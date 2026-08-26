//! 模型下载：DownloadTask / DownloadManager（顺序队列）/ ModelDownloadResolver / 流式下载。
//!
//! - **taskId 独立（UUID 风格）**：同一 repo 多 variant 可同时排队；绝不把 repoId 当 taskId。
//! - **ArtifactSource 分派**：RegistryManifest 走 manifest URL（不经 HF DownloadEndpoint）；
//!   HuggingFace 走 repoId+revision+path + DownloadEndpoint；LocalImport 无远程下载。
//! - **断点续传**：本轮不实现，`DownloadFileSpec.urls` 预留 `Range` 注释。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::catalog::{CatalogError, ModelCategory, RemoteModelFile};
use super::install::{
    InstallMeta, META_SCHEMA_VERSION, ModelStorage, derive_install_id, resolve_runtime_path,
};
use super::verified::VerifiedRegistry;

// ---------------------------------------------------------------------------
// ArtifactSource / DownloadEndpoint（四个概念之二）
// ---------------------------------------------------------------------------

/// Artifact 的安装规格/文件从哪里来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSource {
    /// 内置精选（manifest.json 下载源）。
    RegistryManifest,
    /// Hugging Face。
    HuggingFace,
    /// 本地导入。
    LocalImport,
}

impl ArtifactSource {
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "registry" | "registry_manifest" => Some(ArtifactSource::RegistryManifest),
            "hf" | "huggingface" => Some(ArtifactSource::HuggingFace),
            "local" | "local_import" => Some(ArtifactSource::LocalImport),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DownloadFileSpec / Config / Resolver
// ---------------------------------------------------------------------------

/// 单个下载文件（URL 已在 enqueue 时解析完成；`urls` 按顺序 fallback）。
#[derive(Debug, Clone)]
pub struct DownloadFileSpec {
    /// 相对 install_dir 的路径（保留原始 filename，不重命名）。
    pub path: String,
    /// 候选 URL（如 [primary, mirror]）。
    pub urls: Vec<String>,
    pub size: u64,
    /// LFS/非 LFS sha256（HF 提供时）。
    pub sha256: Option<String>,
}

/// 下载配置（来自 settings，不含 token 泄露到 View）。
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    pub catalog_base: String,
    /// auto | huggingface | mirror。
    pub download_source: String,
    /// 镜像 base URL（可自定义，如 https://hf-mirror.com）。
    pub mirror_url: String,
}

/// URL 解析（**按 ArtifactSource 分派**，RegistryManifest 不经 HF DownloadEndpoint）。
pub struct ModelDownloadResolver {
    cfg: DownloadConfig,
}

fn resolve_path(repo_id: &str, revision: &str, path: &str) -> String {
    let enc = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    out.push(b as char);
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    };
    format!("{}/resolve/{}/{}", enc(repo_id), enc(revision), enc(path))
}

impl ModelDownloadResolver {
    pub fn new(cfg: DownloadConfig) -> Self {
        Self { cfg }
    }

    /// 解析 HF artifact 文件集为 DownloadFileSpec。
    pub fn resolve_hf(
        &self,
        repo_id: &str,
        revision: Option<&str>,
        files: &[RemoteModelFile],
    ) -> Result<Vec<DownloadFileSpec>, CatalogError> {
        let rev = revision.unwrap_or("main");
        let base = self.cfg.catalog_base.trim_end_matches('/').to_string();
        let mut out = Vec::with_capacity(files.len());
        for f in files {
            if f.file_type != "file" {
                continue;
            }
            let rel = resolve_path(repo_id, rev, &f.path);
            let primary = format!("{base}/{rel}");
            let mirror_base = self.cfg.mirror_url.trim_end_matches('/').to_string();
            let mirror = format!("{mirror_base}/{rel}");
            let urls = match self.cfg.download_source.as_str() {
                "mirror" | "hf-mirror" => vec![mirror],
                "huggingface" => vec![primary],
                _ => vec![primary, mirror],
            };
            let size = f.lfs.as_ref().map(|l| l.size).or(f.size).unwrap_or(0);
            let sha256 = f
                .lfs
                .as_ref()
                .map(|l| l.sha256.clone())
                .or_else(|| f.sha256.clone());
            out.push(DownloadFileSpec {
                path: f.path.clone(),
                urls,
                size,
                sha256,
            });
        }
        if out.is_empty() {
            return Err(CatalogError::Endpoint("artifact 没有可下载文件".into()));
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 下载状态 / 任务
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Downloading,
    Verifying,
    Extracting,
    Done,
    Failed,
    Cancelled,
}

impl DownloadState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadState::Queued => "queued",
            DownloadState::Downloading => "downloading",
            DownloadState::Verifying => "verifying",
            DownloadState::Extracting => "extracting",
            DownloadState::Done => "done",
            DownloadState::Failed => "failed",
            DownloadState::Cancelled => "cancelled",
        }
    }
}

/// 内部下载任务（独立 taskId）。
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub task_id: String,
    pub install_id: String,
    pub model_id: String,
    pub artifact_id: String,
    pub variant: Option<String>,
    pub artifact_source: ArtifactSource,
    pub repo_id: Option<String>,
    pub revision: Option<String>,
    pub model_type: Option<ModelCategory>,
    pub install_dir: PathBuf,
    pub runtime_path: PathBuf,
    pub files: Vec<DownloadFileSpec>,
    pub state: DownloadState,
    pub current_file: Option<String>,
    pub file_index: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub created_at: String,
}

/// 前端视图（camelCase）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTaskView {
    pub task_id: String,
    pub model_id: String,
    pub artifact_id: String,
    pub variant: Option<String>,
    pub artifact_source: String,
    pub state: String,
    pub current_file: Option<String>,
    pub file_index: usize,
    pub file_total: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub progress: f64,
    pub queue_position: usize,
    pub queue_length: usize,
}

impl DownloadTask {
    fn view(&self, queue_position: usize, queue_length: usize) -> DownloadTaskView {
        let progress = if self.total_bytes > 0 {
            ((self.bytes_downloaded as f64 / self.total_bytes as f64) * 100.0).min(100.0)
        } else {
            match self.state {
                DownloadState::Done => 100.0,
                _ => 0.0,
            }
        };
        DownloadTaskView {
            task_id: self.task_id.clone(),
            model_id: self.model_id.clone(),
            artifact_id: self.artifact_id.clone(),
            variant: self.variant.clone(),
            artifact_source: match self.artifact_source {
                ArtifactSource::RegistryManifest => "registry_manifest",
                ArtifactSource::HuggingFace => "huggingface",
                ArtifactSource::LocalImport => "local_import",
            }
            .into(),
            state: self.state.as_str().into(),
            current_file: self.current_file.clone(),
            file_index: self.file_index,
            file_total: self.files.len(),
            bytes_downloaded: self.bytes_downloaded,
            total_bytes: self.total_bytes,
            progress,
            queue_position,
            queue_length,
        }
    }
}

/// 下载任务请求（来自前端，camelCase）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadArtifactRequest {
    pub model_id: String,
    pub artifact_id: String,
    pub variant: Option<String>,
    /// "huggingface"（第一版仅支持 HF 在线下载）。
    pub artifact_source: String,
    pub repo_id: Option<String>,
    pub revision: Option<String>,
    pub files: Vec<RemoteModelFile>,
    pub model_type: Option<String>,
}

// ---------------------------------------------------------------------------
// FileDownloader（可注入，测试不联网）
// ---------------------------------------------------------------------------

/// 单文件下载（流式 + 取消）。`progress(bytes_downloaded, total_bytes)`。
pub trait FileDownloader: Send + Sync {
    fn download(
        &self,
        spec: &DownloadFileSpec,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<(), String>;
}

/// 真实下载器：ureq 流式，URL fallback，LFS sha256 校验。
pub struct UreqFileDownloader {
    token: Option<String>,
    /// 仅对该 host 的 URL 加 Authorization（镜像不加）。
    hf_host: String,
}

impl UreqFileDownloader {
    pub fn new(token: Option<String>, catalog_base: String) -> Self {
        let host = catalog_base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        Self {
            token,
            hf_host: host,
        }
    }
}

impl FileDownloader for UreqFileDownloader {
    fn download(
        &self,
        spec: &DownloadFileSpec,
        dest: &Path,
        cancel: &AtomicBool,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<(), String> {
        let mut last_err = None;
        for url in &spec.urls {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            if dest.exists() {
                let _ = std::fs::remove_file(dest);
            }
            let mut on_progress = |p: crate::kws::model::DownloadProgress| {
                let total = p.total_bytes.max(spec.size);
                progress(p.bytes_downloaded, total);
            };
            // 镜像（hf-mirror）不需要 token；仅在 HF host 上加 Authorization。
            let apply_token = url.contains(&self.hf_host) && self.token.is_some();
            let result = if apply_token {
                with_bearer(
                    url,
                    self.token.as_deref().unwrap(),
                    &mut on_progress,
                    cancel,
                    dest,
                    spec.size,
                )
            } else {
                crate::kws::model::download_to(url, dest, spec.size, &mut on_progress, Some(cancel))
            };
            match result {
                Ok(()) => {
                    if let Some(sha) = &spec.sha256 {
                        crate::kws::model::verify_sha256(dest, sha).map_err(|e| e.to_string())?;
                    }
                    return Ok(());
                }
                Err(crate::kws::model::ModelError::Cancelled) => {
                    return Err("cancelled".to_string());
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        Err(last_err.unwrap_or_else(|| "所有下载源失败".to_string()))
    }
}

/// 带 Authorization 的下载（镜像不需要 token）。
fn with_bearer(
    url: &str,
    token: &str,
    progress: &mut (dyn FnMut(crate::kws::model::DownloadProgress) + Send),
    cancel: &AtomicBool,
    dest: &Path,
    size: u64,
) -> Result<(), crate::kws::model::ModelError> {
    // 复用 kws::model::download_to 的流式逻辑；这里额外带 header。
    // 实现：临时走 ureq 手动请求（同 try_download_once 逻辑）。
    crate::kws::model::download_to_with_auth(url, token, dest, size, progress, Some(cancel))
}

// ---------------------------------------------------------------------------
// 事件 sink
// ---------------------------------------------------------------------------

pub trait DownloadEventSink: Send + Sync {
    fn on_update(&self, view: &DownloadTaskView);
}

// ---------------------------------------------------------------------------
// DownloadManager（顺序队列）
// ---------------------------------------------------------------------------

pub struct DownloadManager {
    inner: Mutex<Inner>,
    active: AtomicBool,
    downloader: Mutex<Arc<dyn FileDownloader>>,
    sink: Mutex<Option<Arc<dyn DownloadEventSink>>>,
}

struct Inner {
    tasks: Vec<DownloadTask>,
    cancel: HashMap<String, Arc<AtomicBool>>,
}

impl DownloadManager {
    pub fn new(downloader: Arc<dyn FileDownloader>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                tasks: Vec::new(),
                cancel: HashMap::new(),
            }),
            active: AtomicBool::new(false),
            downloader: Mutex::new(downloader),
            sink: Mutex::new(None),
        }
    }

    pub fn set_sink(&self, sink: Arc<dyn DownloadEventSink>) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    pub fn set_downloader(&self, downloader: Arc<dyn FileDownloader>) {
        *self.downloader.lock().unwrap_or_else(|e| e.into_inner()) = downloader;
    }

    /// 入队（同一 repo 多 variant 各得独立 taskId）。
    pub fn enqueue(
        self: &Arc<Self>,
        req: &DownloadArtifactRequest,
        cfg: &DownloadConfig,
    ) -> Result<DownloadTaskView, String> {
        let source = ArtifactSource::from_str_value(&req.artifact_source)
            .ok_or_else(|| format!("未知的 artifact 来源：{}", req.artifact_source))?;
        if source != ArtifactSource::HuggingFace {
            return Err("该模型使用内置下载流程（不在线下载队列）".to_string());
        }
        let repo_id = req
            .repo_id
            .as_deref()
            .ok_or_else(|| "缺少 repo_id".to_string())?;

        let files = ModelDownloadResolver::new(cfg.clone())
            .resolve_hf(repo_id, req.revision.as_deref(), &req.files)
            .map_err(|e| e.to_string())?;

        let model_type = req
            .model_type
            .as_deref()
            .and_then(ModelCategory::from_str_value)
            .or_else(|| infer_model_type(&req.files));
        let category = model_type.unwrap_or(ModelCategory::Llm);

        let install_dir =
            ModelStorage::install_dir("hf", &req.model_id, category, &req.artifact_id);
        let file_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let runtime_path = resolve_runtime_path(&install_dir, category, &file_paths);
        let install_id = derive_install_id(
            &source,
            &req.model_id,
            &req.artifact_id,
            req.variant.as_deref(),
        );

        let total_bytes: u64 = files.iter().map(|f| f.size).sum();
        let task = DownloadTask {
            task_id: new_task_id(&req.model_id, &req.artifact_id, req.variant.as_deref()),
            install_id,
            model_id: req.model_id.clone(),
            artifact_id: req.artifact_id.clone(),
            variant: req.variant.clone(),
            artifact_source: source,
            repo_id: Some(repo_id.to_string()),
            revision: req.revision.clone(),
            model_type,
            install_dir,
            runtime_path,
            files,
            state: DownloadState::Queued,
            current_file: None,
            file_index: 0,
            bytes_downloaded: 0,
            total_bytes,
            created_at: crate::datetime::iso_timestamp_now(),
        };
        let task_id = task.task_id.clone();
        // 在锁内直接构造入队时刻的 view：若放到 spawn worker 之后重读共享状态，
        // worker 可能已把 Queued 推进到 Downloading，返回值将依赖线程调度。
        let view = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let idx = inner.tasks.len();
            inner.tasks.push(task);
            inner
                .cancel
                .insert(task_id.clone(), Arc::new(AtomicBool::new(false)));
            inner.tasks[idx].view(idx, inner.tasks.len())
        };
        self.emit(&task_id);
        // 启动 worker（若尚无活跃任务）
        if !self.active.swap(true, Ordering::SeqCst) {
            let mgr = self.clone();
            std::thread::spawn(move || mgr.worker_loop());
        }
        Ok(view)
    }

    /// 取消：Queued 直接移除；Downloading 置 cancel 标志。
    pub fn cancel(&self, task_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(idx) = inner.tasks.iter().position(|t| t.task_id == task_id) else {
            return Err("未找到该下载任务".to_string());
        };
        match inner.tasks[idx].state {
            DownloadState::Queued => {
                inner.tasks.remove(idx);
                inner.cancel.remove(task_id);
                drop(inner);
                self.emit(task_id);
                Ok(())
            }
            DownloadState::Downloading => {
                if let Some(flag) = inner.cancel.get(task_id) {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            _ => Err("该任务已结束，无法取消".to_string()),
        }
    }

    pub fn snapshot(&self) -> Vec<DownloadTaskView> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let len = inner.tasks.len();
        inner
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| t.view(i, len))
            .collect()
    }

    fn view_of(&self, task_id: &str) -> (DownloadTaskView, bool) {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let len = inner.tasks.len();
        if let Some((i, t)) = inner
            .tasks
            .iter()
            .enumerate()
            .find(|(_, t)| t.task_id == task_id)
        {
            (t.view(i, len), true)
        } else {
            (
                DownloadTaskView {
                    task_id: task_id.into(),
                    model_id: String::new(),
                    artifact_id: String::new(),
                    variant: None,
                    artifact_source: String::new(),
                    state: DownloadState::Cancelled.as_str().into(),
                    current_file: None,
                    file_index: 0,
                    file_total: 0,
                    bytes_downloaded: 0,
                    total_bytes: 0,
                    progress: 0.0,
                    queue_position: 0,
                    queue_length: 0,
                },
                false,
            )
        }
    }

    fn emit(&self, task_id: &str) {
        let (view, _) = self.view_of(task_id);
        if let Some(sink) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            sink.on_update(&view);
        }
    }

    /// worker：顺序处理队列直到空。
    fn worker_loop(self: &Arc<Self>) {
        loop {
            let task = {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                let Some(idx) = inner
                    .tasks
                    .iter()
                    .position(|t| t.state == DownloadState::Queued)
                else {
                    self.active.store(false, Ordering::SeqCst);
                    return;
                };
                inner.tasks[idx].state = DownloadState::Downloading;
                let task = inner.tasks[idx].clone();
                drop(inner);
                self.emit(&task.task_id);
                task
            };
            let task_id = task.task_id.clone();
            let cancel = {
                let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                inner.cancel.get(&task_id).cloned().unwrap_or_default()
            };
            let result = self.run_download(&task, &cancel);

            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = inner.tasks.iter_mut().find(|t| t.task_id == task_id) {
                match &result {
                    Ok(()) => {
                        t.state = DownloadState::Done;
                        t.bytes_downloaded = t.total_bytes;
                    }
                    Err(msg) if msg == "cancelled" => t.state = DownloadState::Cancelled,
                    Err(_) => t.state = DownloadState::Failed,
                }
            }
            inner.cancel.remove(&task_id);
            drop(inner);
            self.emit(&task_id);
        }
    }

    /// 执行一个任务的全部文件下载 + 校验 + 写元数据。
    fn run_download(&self, task: &DownloadTask, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        std::fs::create_dir_all(&task.install_dir).map_err(|e| format!("创建目录失败：{e}"))?;
        let downloader = self
            .downloader
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (i, spec) in task.files.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let dest = task.install_dir.join(&spec.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
            }
            let mut last_pct = -1.0f64;
            let task_id = task.task_id.clone();
            let file_index = i;
            let path = spec.path.clone();
            let mgr = self;
            let mut progress = move |done: u64, total: u64| {
                let pct = if total > 0 {
                    done as f64 / total as f64 * 100.0
                } else {
                    100.0
                };
                if pct - last_pct >= 1.0 || pct >= 100.0 {
                    last_pct = pct;
                    let mut inner = mgr.inner.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(t) = inner.tasks.iter_mut().find(|t| t.task_id == task_id) {
                        t.bytes_downloaded = done;
                        t.total_bytes = total;
                        t.file_index = file_index;
                        t.current_file = Some(path.clone());
                    }
                    drop(inner);
                    mgr.emit(&task_id);
                }
            };
            downloader.download(spec, &dest, cancel, &mut progress)?;
        }
        // 写元数据
        let meta = InstallMeta {
            schema_version: META_SCHEMA_VERSION,
            install_id: task.install_id.clone(),
            source: "hf".into(),
            model_id: task.model_id.clone(),
            repo_id: task.repo_id.clone(),
            revision: task.revision.clone(),
            model_type: task
                .model_type
                .map(|c| c.as_str().to_string())
                .unwrap_or_default(),
            artifact_id: task.artifact_id.clone(),
            variant: task.variant.clone(),
            architecture: VerifiedRegistry::builtin()
                .entry_for_repo(task.repo_id.as_deref().unwrap_or(""))
                .and_then(|e| e.architecture.clone())
                .or_else(|| Some("unknown".to_string())),
            installed_at: crate::datetime::iso_timestamp_now(),
            registry_id: None,
            version: None,
            managed: Some(true),
        };
        ModelStorage::write_meta(&task.install_dir, &meta)?;
        Ok(())
    }
}

/// 从文件集推断模型类型（LLM=gguf；sherpa 由 compat 层推断；默认 LLM）。
fn infer_model_type(files: &[RemoteModelFile]) -> Option<ModelCategory> {
    if files
        .iter()
        .any(|f| f.path.to_lowercase().ends_with(".gguf"))
    {
        return Some(ModelCategory::Llm);
    }
    // 交给 compat 层在 Phase 4 明细；此处用文件名粗略推断
    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let has = |s: &str| names.iter().any(|n| n.contains(s));
    if has("keywords") {
        Some(ModelCategory::Kws)
    } else if has("joiner") {
        Some(ModelCategory::Asr)
    } else if has("vocoder") || has("lexicon") {
        Some(ModelCategory::Tts)
    } else {
        None
    }
}

/// 生成独立 taskId（UUID 风格，不依赖 uuid crate；同会话内唯一）。
fn new_task_id(model_id: &str, artifact_id: &str, variant: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = format!(
        "{model_id}|{artifact_id}|{}|{}-{n}",
        variant.unwrap_or(""),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let hex = hex::encode(Sha256::digest(seed.as_bytes()));
    let h: Vec<char> = hex.chars().collect();
    format!(
        "dl-{}-{}-{}-{}-{}",
        h[0..8].iter().collect::<String>(),
        h[8..12].iter().collect::<String>(),
        h[12..16].iter().collect::<String>(),
        h[16..20].iter().collect::<String>(),
        h[20..32].iter().collect::<String>()
    )
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(source: &str) -> DownloadConfig {
        DownloadConfig {
            catalog_base: "https://huggingface.co".into(),
            download_source: source.into(),
            mirror_url: "https://hf-mirror.com".into(),
        }
    }

    fn hf_file(path: &str) -> RemoteModelFile {
        RemoteModelFile {
            path: path.into(),
            size: Some(100),
            file_type: "file".into(),
            lfs: None,
            sha256: Some("ab".into()),
        }
    }

    #[test]
    fn test_resolve_hf_urls_auto_fallback() {
        let r = ModelDownloadResolver::new(cfg("auto"));
        let specs = r
            .resolve_hf(
                "Qwen/Qwen3-4B-GGUF",
                None,
                &[hf_file("Qwen3-4B-Q4_K_M.gguf")],
            )
            .unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].urls.len(), 2, "auto → [primary, mirror]");
        assert!(
            specs[0].urls[0].starts_with("https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/")
        );
        assert!(
            specs[0].urls[1].starts_with("https://hf-mirror.com/Qwen/Qwen3-4B-GGUF/resolve/main/")
        );
    }

    #[test]
    fn test_resolve_hf_urls_source_override() {
        let r = ModelDownloadResolver::new(cfg("hf-mirror"));
        let specs = r
            .resolve_hf("a/b", Some("v1"), &[hf_file("m.gguf")])
            .unwrap();
        assert_eq!(specs[0].urls.len(), 1);
        assert!(specs[0].urls[0].contains("hf-mirror.com/a/b/resolve/v1/m.gguf"));

        let r = ModelDownloadResolver::new(cfg("huggingface"));
        let specs = r
            .resolve_hf("a/b", Some("v1"), &[hf_file("m.gguf")])
            .unwrap();
        assert!(specs[0].urls[0].starts_with("https://huggingface.co/"));
    }

    #[test]
    fn test_resolve_hf_encodes_path_space() {
        let r = ModelDownloadResolver::new(cfg("huggingface"));
        let specs = r
            .resolve_hf("a/b", None, &[hf_file("my model.gguf")])
            .unwrap();
        assert!(
            specs[0].urls[0].contains("my%20model.gguf"),
            "{}",
            specs[0].urls[0]
        );
    }

    /// 假下载器：写一个文件，立即成功（测试队列状态机，不联网）。
    struct FakeDownloader;
    impl FileDownloader for FakeDownloader {
        fn download(
            &self,
            spec: &DownloadFileSpec,
            dest: &Path,
            _cancel: &AtomicBool,
            progress: &mut (dyn FnMut(u64, u64) + Send),
        ) -> Result<(), String> {
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::write(dest, format!("fake:{}", spec.path)).unwrap();
            progress(spec.size, spec.size);
            Ok(())
        }
    }

    /// 阻塞下载器：等待 cancel 或超时（测取消）。
    struct BlockingDownloader;
    impl FileDownloader for BlockingDownloader {
        fn download(
            &self,
            _spec: &DownloadFileSpec,
            _dest: &Path,
            cancel: &AtomicBool,
            _progress: &mut (dyn FnMut(u64, u64) + Send),
        ) -> Result<(), String> {
            for _ in 0..200 {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(())
        }
    }

    fn req(model: &str) -> DownloadArtifactRequest {
        DownloadArtifactRequest {
            model_id: model.into(),
            artifact_id: "Q4_K_M".into(),
            variant: Some("Q4_K_M".into()),
            artifact_source: "huggingface".into(),
            repo_id: Some(model.into()),
            revision: None,
            files: vec![hf_file("model-Q4_K_M.gguf")],
            model_type: Some("llm".into()),
        }
    }

    #[test]
    fn test_enqueue_and_snapshot() {
        crate::test_util::run_with_temp_home(|_| {
            let mgr = Arc::new(DownloadManager::new(Arc::new(FakeDownloader)));
            let v = mgr
                .enqueue(&req("Qwen/Qwen3-4B-GGUF"), &cfg("huggingface"))
                .unwrap();
            assert!(v.task_id.starts_with("dl-"));
            assert_eq!(v.state, "queued");
            assert_eq!(mgr.snapshot().len(), 1);
            // 等待完成
            for _ in 0..200 {
                let snap = mgr.snapshot();
                if snap.iter().any(|t| t.state == "done") {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let snap = mgr.snapshot();
            assert_eq!(snap[0].state, "done", "下载应完成");
            // 元数据写入
            let task = {
                let inner = mgr.inner.lock().unwrap();
                inner.tasks[0].clone()
            };
            assert!(ModelStorage::read_meta(&task.install_dir).is_some());
        });
    }

    #[test]
    fn test_cancel_queued_removes() {
        crate::test_util::run_with_temp_home(|_| {
            let mgr = Arc::new(DownloadManager::new(Arc::new(BlockingDownloader)));
            let v1 = mgr.enqueue(&req("a/A"), &cfg("huggingface")).unwrap();
            let v2 = mgr.enqueue(&req("a/B"), &cfg("huggingface")).unwrap();
            // v2 是 queued（v1 正在下载）→ cancel 直接移除
            mgr.cancel(&v2.task_id).unwrap();
            let snap = mgr.snapshot();
            assert!(
                !snap.iter().any(|t| t.task_id == v2.task_id),
                "queued 任务应被移除"
            );
            // 取消活跃的 v1
            mgr.cancel(&v1.task_id).unwrap();
            for _ in 0..200 {
                let snap = mgr.snapshot();
                if snap
                    .iter()
                    .all(|t| t.state == "cancelled" || t.state == "done")
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
    }

    #[test]
    fn test_same_repo_multi_variant_distinct_task_ids() {
        crate::test_util::run_with_temp_home(|_| {
            let mgr = Arc::new(DownloadManager::new(Arc::new(FakeDownloader)));
            let mut a = req("Qwen/Qwen3-4B-GGUF");
            a.artifact_id = "Q4_K_M".into();
            let mut b = req("Qwen/Qwen3-4B-GGUF");
            b.artifact_id = "Q5_K_M".into();
            let va = mgr.enqueue(&a, &cfg("huggingface")).unwrap();
            let vb = mgr.enqueue(&b, &cfg("huggingface")).unwrap();
            assert_ne!(va.task_id, vb.task_id, "同 repo 多 variant 必须独立 taskId");
        });
    }
}
