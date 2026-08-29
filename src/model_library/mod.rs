//! 模型库核心服务（core）。
//!
//! 分层（不依赖 Tauri runtime state）：
//! - `registry`：RegistryModel 目录（这个模型是什么）
//! - `Installation`：managed 安装目录（`.zapmomo-lib.json`）与 external 注册（settings）
//! - `RuntimeSelection`：复用现有 `model_dir / model_path`（用户选择哪个）
//! - `RuntimeActual`：由 Tauri 层持有的运行时状态，经 `enrich_runtime_status` 注入
//!
//! core 只负责：目录/安装/external 注册、路径解析与安全、settings 读写（带锁）、
//! 模型完整性判断、`set_selected_model`/`restore_selected_model`、下载安装编排。
//! 模型加载/引擎生命周期由各能力模块与 Tauri 层负责，本模块**不复制任何 runtime**。

pub mod catalog;
pub mod install;
pub mod registry;
pub mod storage;
pub mod sysinfo;
pub mod verified;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::config::settings::{self, AppConfig, LocalModel, ModelLibrarySettings};
use crate::kws::model::{DownloadProgress, ModelError, ProgressFn, has_required_files};
use registry::{ModelType, RegistryModel};

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

/// 模型来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Registry,
    Local,
    /// Hugging Face 在线下载安装。
    Hf,
}

/// 文件所有权：由「来源行为」确定，不由路径位置猜测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOwnership {
    /// ZapMomo 自己下载安装到 `~/.zapmomo/models`，拥有文件生命周期管理权
    Managed,
    /// 用户注册，ZapMomo 不拥有文件（移除时绝不删除原始文件）
    External,
}

/// 安装状态（本地文件系统是最终事实来源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    NotInstalled,
    Downloading,
    Installed,
    /// 路径存在但模型不完整 / 注册文件丢失 / GGUF 无效
    Invalid,
}

/// 运行状态：区分「已选择但未运行」与「正在运行旧模型」与「已经是当前 runtime」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    /// 模型是 selection，但能力当前没有 runtime（正常，不是错误）
    Inactive,
    /// selected path == RuntimeActual path 且能力正在运行
    Active,
    /// LLM 正在切换到此 target（runtime 加载中）
    Switching,
    /// RuntimeActual = A、RuntimeSelection = B（下次 start 使用 B）
    PendingRestart,
    /// selection model == last_load_error.model_path 且 RuntimeActual = None
    LoadFailed,
}

/// 一次「设为当前模型」最终发生了什么（set current 的返回值语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAction {
    /// 只更新了 selection（runtime 未运行，下次加载/启动生效）
    None,
    /// 切换并成功 reload 到新模型
    Reloaded,
    /// KWS/ASR 正在监听：已更新 selection，需下次启动生效
    RestartRequired,
    /// LLM：B 加载失败，已恢复 A 且 A 重新加载成功
    ReloadFailedRolledBack,
    /// LLM：B 加载失败，恢复 A 后 A 也加载失败
    ReloadFailedRollbackFailed,
}

// ---------------------------------------------------------------------------
// 对外数据结构
// ---------------------------------------------------------------------------

/// `set_current_model` 的返回结果（camelCase 直供前端，UI 据此 Toast）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCurrentResult {
    pub model_type: ModelType,
    pub model_id: String,
    pub path: String,
    pub runtime_action: RuntimeAction,
    pub effective_immediately: bool,
    pub message: String,
}

/// 系统资源（独立命令，不阻塞模型列表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemResources {
    /// 字节
    pub total_memory: u64,
    /// 字节
    pub available_memory: u64,
    /// 字节（模型目录所在挂载点）
    pub disk_total: u64,
    /// 字节（模型目录所在挂载点）
    pub disk_available: u64,
    /// 0..=100，瞬时采样
    pub cpu_usage: f32,
}

/// 模型库列表中的一条模型（camelCase 直供前端）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryModel {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub model_type: ModelType,
    pub runtime: String,
    pub format: String,
    pub description: String,
    pub languages: Vec<String>,
    pub tags: Vec<String>,
    pub parameter_count: Option<String>,
    pub quantization: Option<String>,
    pub version: String,
    pub size_bytes: Option<u64>,
    pub homepage: Option<String>,
    /// 是否有内置下载源（false = LLM 需导入 GGUF）
    pub downloadable: bool,
    pub source: ModelSource,
    pub ownership: StorageOwnership,
    pub install_state: InstallState,
    /// 是否为该能力当前选择的模型（RuntimeSelection）
    pub current: bool,
    /// 运行状态（仅 current 模型有意义；非 current 恒 Inactive）
    pub runtime_status: RuntimeStatus,
    /// 已安装/已注册的本地路径
    pub local_path: Option<String>,
    pub installed_at: Option<String>,
    /// 稳定安装身份（`set_current_model` / `delete_model` 按此唯一定位具体 Artifact）。
    pub install_id: Option<String>,
    /// HF repo_id（若可映射）。
    pub repo_id: Option<String>,
    /// 兼容性级别字符串（verified/compatible/possible/unsupported；本地模型可空）。
    pub compatibility: Option<String>,
}

/// 四个能力的当前 selection（复用现有 settings，不新增字段）。
#[derive(Debug, Clone, Default)]
pub struct Selections {
    pub kws: Option<PathBuf>,
    pub asr: Option<PathBuf>,
    pub tts: Option<PathBuf>,
    pub llm: Option<PathBuf>,
}

/// 供 `enrich_runtime_status` 注入的 RuntimeActual（来自 Tauri 层 runtime state）。
pub struct RuntimeActuals<'a> {
    pub kws: Option<&'a Path>,
    pub asr: Option<&'a Path>,
    /// TTS 无常驻引擎：actual = 当前 selection（与 current 判定同源）
    pub tts: Option<&'a Path>,
    /// 是否有合成线程在跑（在飞合成用旧配置完成，下次合成读新配置）
    pub tts_active: bool,
    pub llm: Option<&'a Path>,
    pub llm_switching: bool,
    pub llm_switch_target: Option<&'a Path>,
    pub llm_load_error_path: Option<&'a Path>,
}

/// managed 安装元数据（`.zapmomo-lib.json`）。只记录安装信息，不含 current/enabled。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedMeta {
    pub schema_version: u32,
    pub registry_id: String,
    pub version: String,
    #[serde(default)]
    pub installed_at: Option<String>,
    pub managed: bool,
}

// ---------------------------------------------------------------------------
// Settings 读写（带锁，仅保护模型库自身操作）
// ---------------------------------------------------------------------------

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

/// 带锁的 settings 更新：load → mutate → save。模型库自身的 RMW 操作互不覆盖。
pub fn update_settings<F>(f: F) -> Result<(), String>
where
    F: FnOnce(&mut AppConfig),
{
    let _guard = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = settings::load_settings()?.unwrap_or_default();
    f(&mut cfg);
    settings::save_settings(&cfg)
}

/// 读取所有 external 注册记录。
pub fn get_local_models() -> Vec<LocalModel> {
    settings::load_settings()
        .ok()
        .flatten()
        .and_then(|s| s.model_library)
        .map(|lib| lib.local_models)
        .unwrap_or_default()
}

/// 新增/更新 external 注册；同一 `registry_id` 只保留一条绑定。
pub fn add_local_model_record(m: LocalModel) -> Result<(), String> {
    update_settings(|cfg| {
        let lib = cfg
            .model_library
            .get_or_insert_with(ModelLibrarySettings::default);
        if let Some(rid) = &m.registry_id {
            lib.local_models
                .retain(|x| x.registry_id.as_deref() != Some(rid.as_str()));
        }
        if let Some(existing) = lib.local_models.iter_mut().find(|x| x.id == m.id) {
            *existing = m;
        } else {
            lib.local_models.push(m);
        }
    })
}

/// 移除 external 注册（不删任何用户文件）。
pub fn remove_local_model_record(id: &str) -> Result<(), String> {
    update_settings(|cfg| {
        if let Some(lib) = cfg.model_library.as_mut() {
            lib.local_models.retain(|x| x.id != id);
        }
    })
}

/// 若模型通过 external 注册存在（registry 绑定或 standalone），返回需移除的注册 id；
/// managed 安装返回 `None`（命令层据此决定「移除注册」还是「删除文件」）。
pub fn external_binding_to_remove(model_id: &str) -> Option<String> {
    let locals = get_local_models();
    if let Some(l) = locals
        .iter()
        .find(|l| l.registry_id.as_deref() == Some(model_id))
    {
        return Some(l.id.clone());
    }
    if locals.iter().any(|l| l.id == model_id) {
        return Some(model_id.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// 路径工具
// ---------------------------------------------------------------------------

/// 规范化路径：优先 canonicalize，失败回退 absolute（Windows 大小写不敏感）。
pub fn normalize_path(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        c
    } else {
        std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// 稳定路径比较（跨平台 / symlink / 相对路径）。
pub fn paths_equal(a: &Path, b: &Path) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// 当前 selection
// ---------------------------------------------------------------------------

pub fn current_selections() -> Selections {
    let s = settings::load_settings().ok().flatten();
    Selections {
        kws: s
            .as_ref()
            .and_then(|c| crate::kws::config::resolve(c.kws.as_ref(), None).ok())
            .map(|c| c.model_dir),
        asr: s
            .as_ref()
            .and_then(|c| crate::asr::config::resolve(c.asr.as_ref(), None).ok())
            .map(|c| c.model_dir),
        tts: s
            .as_ref()
            .and_then(|c| crate::tts::config::resolve(c.tts.as_ref(), None).ok())
            .map(|c| c.model_dir),
        // LLM 已改为远程连接（无本地模型路径选择），恒 None
        llm: None,
    }
}

/// 指定能力的当前 selection 路径。
pub fn selection_path(mt: ModelType) -> Option<PathBuf> {
    let s = current_selections();
    match mt {
        ModelType::Kws => s.kws,
        ModelType::Asr => s.asr,
        ModelType::Tts => s.tts,
        ModelType::Llm => s.llm,
    }
}

/// GGUF 文件有效性检查（扩展名 + 魔数）。
///
/// 原实现来自已移除的 llama 后端（`llm::local::llama::is_gguf_file`）；
/// 本地 LLM 推理移除后此处仅服务于「用户历史注册的 external LLM 条目」
/// 的完整性探测。
fn is_gguf_file(path: &Path) -> bool {
    use std::io::Read;
    if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
        return false;
    }
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).is_ok() && &magic == b"GGUF"
}

/// 路径是否为该能力当前 selection。
pub fn is_path_current(mt: ModelType, path: &Path) -> bool {
    selection_path(mt).is_some_and(|p| paths_equal(&p, path))
}

/// 设置当前模型（写 `model_dir` / `model_path` 并重置文件级覆盖，绝不写 enabled）。
pub fn set_selected_model(mt: ModelType, path: &Path) -> Result<(), String> {
    let path_str = path.to_string_lossy().to_string();
    update_settings(|cfg| match mt {
        ModelType::Kws => {
            let kws = cfg.kws.get_or_insert_with(Default::default);
            kws.model_dir = Some(path_str);
            // 切换模型目录时重置文件级覆盖：旧模型的手写覆盖（如 zh-en 的
            // epoch-13 文件名）会污染新模型的文件探测，交回 resolve 自动探测。
            kws.encoder = None;
            kws.decoder = None;
            kws.joiner = None;
            kws.tokens = None;
            kws.keywords_file = None;
        }
        ModelType::Asr => {
            let asr = cfg.asr.get_or_insert_with(Default::default);
            asr.model_dir = Some(path_str);
            // 切换时同步持久化模型类型与推理后端：managed 安装目录名 == registry
            // `name`，据此推导 sensevoice/whisper/qwen3_asr 与 audiocpp（runtime
            // 字段）。streaming zipformer 条目与 external/local 目录无权威 kind，
            // 先复位为 None 交回 resolve 按目录内容探测——残留旧族 model_type 会
            // 用旧探针校验新目录，误报「模型文件缺失」（qwen3 → zipformer 实测）。
            asr.model_type = None;
            let mut matched_registry = false;
            if let Some(name) = path.file_name() {
                let base = name.to_string_lossy().to_string();
                if let Some(entry) = crate::model_library::registry::all_models()
                    .iter()
                    .filter(|m| m.model_type == ModelType::Asr)
                    .find(|m| m.name == base)
                {
                    matched_registry = true;
                    if let Some(kind) = entry.asr_kind {
                        asr.model_type = Some(kind);
                    }
                    // audiocpp 条目写 backend；sherpa 条目写 None 复位（保证从
                    // audiocpp 切回 sherpa 时后端归位缺省）。
                    asr.backend = (entry.runtime == "audiocpp").then(|| "audiocpp".to_string());
                    asr.engine_path = None;
                }
            }
            if !matched_registry {
                // external/local 目录（registry 未收录）：目录内含 audiocpp 族 GGUF
                // 则自动识别 audiocpp 后端，否则复位缺省（残留 audiocpp 会拦住
                // sherpa 模型的识别）。
                let gguf = crate::audiocpp::asr_families::detect_gguf_in_dir(path);
                asr.backend = gguf.map(|_| "audiocpp".to_string());
                asr.engine_path = None;
            }
            // audiocpp 无热词能力：切到 audiocpp 时清空热词，避免残留配置误导
            // （patch 层也会过滤，双保险）；切回 sherpa 时用户重新配置。
            if asr.backend.as_deref() == Some("audiocpp") {
                asr.hotwords = None;
            }
            // 切换模型目录时重置文件级覆盖：旧模型的手写覆盖（encoder/decoder/joiner/
            // tokens）与族专属参数（language/use_itn）会污染新模型，交回 resolve 自动探测
            // （与 KWS/TTS 分支同款取舍）。
            asr.encoder = None;
            asr.decoder = None;
            asr.joiner = None;
            asr.tokens = None;
            asr.language = None;
            asr.use_itn = None;
        }
        ModelType::Tts => {
            let old_kind = cfg.tts.as_ref().and_then(|t| t.model_type);
            let tts = cfg.tts.get_or_insert_with(Default::default);
            tts.model_dir = Some(path_str);
            // 切换时同步持久化模型类型与推理后端：managed 安装目录名 == registry
            // `name`，据此推导 zipvoice 与 audiocpp（runtime 字段）；
            // external/local 目录探测不到时保持原值。
            let mut new_kind = old_kind;
            if let Some(name) = path.file_name() {
                let base = name.to_string_lossy().to_string();
                if let Some(entry) = crate::model_library::registry::all_models()
                    .iter()
                    .filter(|m| m.model_type == ModelType::Tts)
                    .find(|m| m.name == base)
                {
                    if let Some(kind) = entry.tts_kind {
                        tts.model_type = Some(kind);
                        new_kind = Some(kind);
                    }
                    // audiocpp 条目写 backend；sherpa 条目写 None 复位（保证从
                    // audiocpp 切回 sherpa 时后端归位缺省）。
                    tts.backend = (entry.runtime == "audiocpp").then(|| "audiocpp".to_string());
                    tts.engine_path = None;
                } else {
                    // external/local 目录（registry 未收录）：复位缺省后端——
                    // 外部目录当前只可能是 sherpa 模型，残留 audiocpp 会拦住合成。
                    tts.backend = None;
                    tts.engine_path = None;
                }
            }
            // 模型族变化时清空默认音色：zipvoice 音色 id（leijun-1 等参考音色）与
            // omnivoice 具名音色互为无效值，残留会让切换后的首次合成报「未找到音色」。
            if old_kind.is_some() && old_kind != new_kind {
                tts.voice = None;
            }
            // 切换模型目录时重置文件级覆盖：旧模型的手写覆盖（encoder/vocoder 等）
            // 会污染新模型的文件探测，交回 resolve 自动探测（与 KWS/ASR 分支同款取舍）。
            // reference_wav/text 指向旧模型目录内的参考音频，一并重置回默认音色；
            // enabled / num_steps / speed 等用户偏好不重置。
            tts.encoder = None;
            tts.decoder = None;
            tts.vocoder = None;
            tts.tokens = None;
            tts.lexicon = None;
            tts.data_dir = None;
            tts.reference_wav = None;
            tts.reference_text = None;
        }
        ModelType::Llm => {
            // LLM 已改为远程连接：无本地模型路径可写，切换不产生配置副作用
        }
    })
}

/// 恢复之前的选择（回滚用）。`old` 为 `None` 表示恢复为「未配置」。
pub fn restore_selected_model(mt: ModelType, old: Option<String>) -> Result<(), String> {
    update_settings(|cfg| match mt {
        ModelType::Kws => cfg.kws.get_or_insert_with(Default::default).model_dir = old,
        ModelType::Asr => cfg.asr.get_or_insert_with(Default::default).model_dir = old,
        ModelType::Tts => cfg.tts.get_or_insert_with(Default::default).model_dir = old,
        ModelType::Llm => {}
    })
}

/// 恢复 TTS 整段配置（热切换构造失败时的回滚）。比 [`restore_selected_model`]
/// 的单字段恢复更完整：`set_selected_model` 切 TTS 时会同步写 model_type/backend、
/// 清 voice/engine_path 与文件级覆盖，单恢复 model_dir 会留下半新半旧状态。
pub fn restore_tts_settings(
    old: Option<crate::config::settings::TtsSettings>,
) -> Result<(), String> {
    update_settings(|cfg| cfg.tts = old)
}

// ---------------------------------------------------------------------------
// RuntimeStatus 纯函数（可单测，不依赖 runtime state）
// ---------------------------------------------------------------------------

pub fn runtime_status(
    model_path: Option<&Path>,
    actual: Option<&Path>,
    capability_active: bool,
    switching_for_this: bool,
    load_failed_for_this: bool,
) -> RuntimeStatus {
    if switching_for_this {
        return RuntimeStatus::Switching;
    }
    match actual {
        Some(a) => {
            let same = model_path.is_some_and(|m| paths_equal(m, a));
            if same {
                if capability_active {
                    RuntimeStatus::Active
                } else {
                    RuntimeStatus::Inactive
                }
            } else if capability_active {
                RuntimeStatus::PendingRestart
            } else {
                RuntimeStatus::Inactive
            }
        }
        None => {
            if load_failed_for_this {
                RuntimeStatus::LoadFailed
            } else {
                RuntimeStatus::Inactive
            }
        }
    }
}

/// 用 RuntimeActual 批量填充每个模型的 `runtime_status`（core 不依赖 Tauri state）。
pub fn enrich_runtime_status(models: &mut [LibraryModel], a: &RuntimeActuals) {
    for m in models.iter_mut() {
        if !m.current {
            m.runtime_status = RuntimeStatus::Inactive;
            continue;
        }
        let mp = m.local_path.as_deref().map(Path::new);
        let (actual, active) = match m.model_type {
            ModelType::Kws => (a.kws, a.kws.is_some()),
            ModelType::Asr => (a.asr, a.asr.is_some()),
            ModelType::Llm => (a.llm, a.llm.is_some()),
            ModelType::Tts => (a.tts, a.tts_active),
        };
        let switching = m.model_type == ModelType::Llm
            && a.llm_switching
            && mp.is_some_and(|p| a.llm_switch_target.is_some_and(|t| paths_equal(p, t)));
        let load_failed = m.model_type == ModelType::Llm
            && mp.is_some_and(|p| a.llm_load_error_path.is_some_and(|e| paths_equal(p, e)));
        m.runtime_status = runtime_status(mp, actual, active, switching, load_failed);
    }
}

// ---------------------------------------------------------------------------
// 列表构建
// ---------------------------------------------------------------------------

/// 构建模型库列表（registry 精选 + standalone external + HF 已安装）。
///
/// installed inventory 的唯一事实来源是 `ModelStorage` 扫描 + external 注册；
/// registry 精选卡片用于展示内置模型（含其下载/导入动作）。
pub fn list_models() -> Vec<LibraryModel> {
    let sel = current_selections();
    let locals = get_local_models();
    let mut out = Vec::new();
    // 平台受限条目（如仅 Metal 平台的 omnivoice）在此过滤：不可见即不可下载
    for reg in registry::models_for_current_platform() {
        out.push(build_registry_model(reg, &sel, &locals));
    }
    for lm in locals.iter().filter(|l| l.registry_id.is_none()) {
        out.push(build_local_model(lm, &sel));
    }
    // HF 在线下载安装（scan `.zapmomo-lib.json`）
    for (dir, meta) in crate::model_library::install::ModelStorage::scan_installs() {
        if meta.source == "hf"
            && let Some(m) = build_installed_model(&dir, &meta)
        {
            out.push(m);
        }
    }
    out
}

/// 按 `id` 或 `install_id` 解析（Current/Delete 能唯一定位具体安装实例）。
pub fn resolve_model(id: &str) -> Option<LibraryModel> {
    list_models()
        .into_iter()
        .find(|m| m.id == id || m.install_id.as_deref() == Some(id))
}

/// 从 HF 安装元数据构建 LibraryModel。
fn build_installed_model(
    dir: &std::path::Path,
    meta: &crate::model_library::install::InstallMeta,
) -> Option<LibraryModel> {
    use crate::model_library::catalog::CompatibilityLevel;

    let mt = ModelType::from_str_value(&meta.model_type).unwrap_or(ModelType::Llm);
    let (runtime_path, install_state) = if mt == ModelType::Llm {
        let gguf = first_gguf_in(dir);
        match gguf {
            Some(p) if is_gguf_file(&p) => (p, InstallState::Installed),
            Some(p) => (p, InstallState::Invalid),
            None => (dir.to_path_buf(), InstallState::Invalid),
        }
    } else {
        let ok = match mt {
            ModelType::Kws => crate::kws::config::kws_files_present(dir),
            ModelType::Asr => crate::asr::is_installed(dir),
            ModelType::Tts => crate::tts::is_installed(dir),
            ModelType::Llm => false,
        };
        (
            dir.to_path_buf(),
            if ok {
                InstallState::Installed
            } else {
                InstallState::Invalid
            },
        )
    };
    let current = is_path_current(mt, &runtime_path);
    let display_name = meta
        .model_id
        .rsplit('/')
        .next()
        .unwrap_or(&meta.model_id)
        .to_string();
    let verified = crate::model_library::verified::VerifiedRegistry::builtin()
        .is_verified_repo(meta.repo_id.as_deref().unwrap_or(""));
    let install_id = Some(meta.install_id.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| meta.registry_id.clone());
    let id = install_id.clone().unwrap_or_else(|| meta.model_id.clone());
    Some(LibraryModel {
        id,
        name: meta.model_id.clone(),
        display_name,
        model_type: mt,
        runtime: if mt == ModelType::Llm {
            "llama.cpp"
        } else {
            "sherpa-onnx"
        }
        .to_string(),
        format: if mt == ModelType::Llm { "GGUF" } else { "ONNX" }.to_string(),
        description: "Hugging Face 在线下载".to_string(),
        languages: Vec::new(),
        tags: Vec::new(),
        parameter_count: None,
        quantization: meta.variant.clone(),
        version: meta.revision.clone().unwrap_or_default(),
        size_bytes: None,
        homepage: None,
        downloadable: true,
        source: ModelSource::Hf,
        ownership: StorageOwnership::Managed,
        install_state,
        current,
        runtime_status: RuntimeStatus::Inactive,
        local_path: Some(runtime_path.display().to_string()),
        installed_at: Some(meta.installed_at.clone()),
        install_id,
        repo_id: meta.repo_id.clone(),
        compatibility: if verified {
            Some(CompatibilityLevel::Verified.as_str().to_string())
        } else {
            None
        },
    })
}

/// 目录内第一个 .gguf 文件（LLM runtime_path）。
fn first_gguf_in(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("gguf") {
            return Some(p);
        }
    }
    None
}

fn build_registry_model(
    reg: &RegistryModel,
    sel: &Selections,
    locals: &[LocalModel],
) -> LibraryModel {
    let root = crate::config::settings::get_models_dir();
    if reg.is_llm() {
        let binding = locals
            .iter()
            .find(|l| l.registry_id.as_deref() == Some(reg.id.as_str()));
        // 双根定位：模型可能装在新根或旧默认根（双根兼容），都不在时保持主根路径展示
        let managed_dir = locate_managed_dir(&reg.name);
        let canonical = match (&managed_dir, reg.file_name.as_ref()) {
            (Some(dir), Some(f)) => Some(dir.join(f)),
            _ => None,
        };
        let (local_path, ownership, install_state) = if let Some(b) = binding {
            let p = PathBuf::from(&b.path);
            let state = if p.is_file() && is_gguf_file(&p) {
                InstallState::Installed
            } else {
                InstallState::Invalid
            };
            (Some(p), StorageOwnership::External, state)
        } else if let Some(c) = &canonical {
            if c.is_file() && is_gguf_file(c) {
                ensure_managed_meta(reg, c.parent().unwrap_or(&root));
                (
                    Some(c.clone()),
                    StorageOwnership::Managed,
                    InstallState::Installed,
                )
            } else if c.exists() || c.parent().is_some_and(|d| d.exists()) {
                (
                    Some(c.clone()),
                    StorageOwnership::Managed,
                    InstallState::Invalid,
                )
            } else {
                (None, StorageOwnership::Managed, InstallState::NotInstalled)
            }
        } else {
            (None, StorageOwnership::Managed, InstallState::NotInstalled)
        };
        let current = sel
            .llm
            .as_ref()
            .is_some_and(|s| local_path.as_ref().is_some_and(|p| paths_equal(s, p)));
        let installed_at = binding.as_ref().map(|b| b.added_at.clone()).or_else(|| {
            local_path
                .as_deref()
                .and_then(|p| read_managed_installed_at(Path::new(p).parent().unwrap_or(&root)))
        });
        let verified_entry =
            crate::model_library::verified::VerifiedRegistry::builtin().entry_for_model(&reg.id);
        let install_id = if matches!(
            install_state,
            InstallState::Installed | InstallState::Invalid
        ) {
            Some(reg.id.clone())
        } else {
            None
        };
        return LibraryModel {
            id: reg.id.clone(),
            name: reg.name.clone(),
            display_name: reg.display_name.clone(),
            model_type: reg.model_type,
            runtime: reg.runtime.clone(),
            format: reg.format.clone(),
            description: reg.description.clone(),
            languages: reg.languages.clone(),
            tags: reg.tags.clone(),
            parameter_count: reg.parameter_count.clone(),
            quantization: reg.quantization.clone(),
            version: reg.version.clone(),
            size_bytes: reg.size_bytes,
            homepage: reg.homepage.clone(),
            downloadable: reg.download.is_some(),
            source: ModelSource::Registry,
            ownership,
            install_state,
            current,
            runtime_status: RuntimeStatus::Inactive,
            local_path: local_path.map(|p| p.display().to_string()),
            installed_at,
            install_id,
            repo_id: verified_entry.and_then(|e| e.repo_id.clone()),
            compatibility: verified_entry.map(|_| {
                crate::model_library::catalog::CompatibilityLevel::Verified
                    .as_str()
                    .to_string()
            }),
        };
    }

    // sherpa managed 模型：双根定位（旧根存量仍识别为已安装）
    let dest = locate_managed_dir(&reg.name).unwrap_or_else(|| root.join(&reg.name));
    let required: Vec<&str> = reg
        .required_assets
        .iter()
        .flat_map(|r| registry::required_files_for_role(r).iter().copied())
        .collect();
    let install_state = if has_required_files(&dest, &required) {
        ensure_managed_meta(reg, &dest);
        InstallState::Installed
    } else if dest.exists() {
        InstallState::Invalid
    } else {
        InstallState::NotInstalled
    };
    let sel_path = match reg.model_type {
        ModelType::Kws => sel.kws.as_ref(),
        ModelType::Asr => sel.asr.as_ref(),
        ModelType::Tts => sel.tts.as_ref(),
        ModelType::Llm => sel.llm.as_ref(),
    };
    let current = sel_path.is_some_and(|s| paths_equal(s, &dest));
    let verified_entry =
        crate::model_library::verified::VerifiedRegistry::builtin().entry_for_model(&reg.id);
    let install_id = if matches!(
        install_state,
        InstallState::Installed | InstallState::Invalid
    ) {
        Some(reg.id.clone())
    } else {
        None
    };
    LibraryModel {
        id: reg.id.clone(),
        name: reg.name.clone(),
        display_name: reg.display_name.clone(),
        model_type: reg.model_type,
        runtime: reg.runtime.clone(),
        format: reg.format.clone(),
        description: reg.description.clone(),
        languages: reg.languages.clone(),
        tags: reg.tags.clone(),
        parameter_count: reg.parameter_count.clone(),
        quantization: reg.quantization.clone(),
        version: reg.version.clone(),
        size_bytes: reg.size_bytes,
        homepage: reg.homepage.clone(),
        downloadable: reg.download.is_some(),
        source: ModelSource::Registry,
        ownership: StorageOwnership::Managed,
        install_state,
        current,
        runtime_status: RuntimeStatus::Inactive,
        local_path: if dest.exists() {
            Some(dest.display().to_string())
        } else {
            None
        },
        installed_at: read_managed_installed_at(&dest),
        install_id,
        repo_id: verified_entry.and_then(|e| e.repo_id.clone()),
        compatibility: verified_entry.map(|_| {
            crate::model_library::catalog::CompatibilityLevel::Verified
                .as_str()
                .to_string()
        }),
    }
}

fn build_local_model(lm: &LocalModel, sel: &Selections) -> LibraryModel {
    let mt = ModelType::from_str_value(&lm.model_type).unwrap_or(ModelType::Llm);
    let path = PathBuf::from(&lm.path);
    let install_state = if !path.exists() {
        InstallState::Invalid
    } else {
        match mt {
            ModelType::Llm => {
                if is_gguf_file(&path) {
                    InstallState::Installed
                } else {
                    InstallState::Invalid
                }
            }
            ModelType::Kws => {
                if crate::kws::config::kws_files_present(&path) {
                    InstallState::Installed
                } else {
                    InstallState::Invalid
                }
            }
            ModelType::Asr => {
                if crate::asr::is_installed(&path) {
                    InstallState::Installed
                } else {
                    InstallState::Invalid
                }
            }
            ModelType::Tts => {
                if crate::tts::is_installed(&path) {
                    InstallState::Installed
                } else {
                    InstallState::Invalid
                }
            }
        }
    };
    let sel_path = match mt {
        ModelType::Kws => sel.kws.as_ref(),
        ModelType::Asr => sel.asr.as_ref(),
        ModelType::Tts => sel.tts.as_ref(),
        ModelType::Llm => sel.llm.as_ref(),
    };
    let current = sel_path.is_some_and(|s| paths_equal(s, &path));
    let (runtime, format) = if mt == ModelType::Llm {
        ("llama.cpp".to_string(), "GGUF".to_string())
    } else {
        ("sherpa-onnx".to_string(), "ONNX".to_string())
    };
    LibraryModel {
        id: lm.id.clone(),
        name: lm.name.clone(),
        display_name: lm.name.clone(),
        model_type: mt,
        runtime,
        format,
        description: "本地模型".to_string(),
        languages: Vec::new(),
        tags: Vec::new(),
        parameter_count: None,
        quantization: None,
        version: String::new(),
        size_bytes: None,
        homepage: None,
        downloadable: false,
        source: ModelSource::Local,
        ownership: StorageOwnership::External,
        install_state,
        current,
        runtime_status: RuntimeStatus::Inactive,
        local_path: Some(lm.path.clone()),
        installed_at: Some(lm.added_at.clone()),
        install_id: Some(lm.id.clone()),
        repo_id: None,
        compatibility: None,
    }
}

fn managed_meta_path(dir: &Path) -> PathBuf {
    dir.join(".zapmomo-lib.json")
}

fn read_managed_installed_at(dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(managed_meta_path(dir)).ok()?;
    let meta: ManagedMeta = serde_json::from_str(&content).ok()?;
    meta.installed_at
}

/// legacy managed 识别：完整目录无 metadata 时 best-effort 补写，失败不影响 Installed。
fn ensure_managed_meta(reg: &RegistryModel, dest: &Path) {
    let meta_path = managed_meta_path(dest);
    if meta_path.is_file() {
        return;
    }
    let meta = ManagedMeta {
        schema_version: 1,
        registry_id: reg.id.clone(),
        version: reg.version.clone(),
        installed_at: None,
        managed: true,
    };
    if let Ok(json) = serde_json::to_string_pretty(&meta) {
        let _ = std::fs::write(&meta_path, json);
    }
}

// ---------------------------------------------------------------------------
// 安装（managed，model-level staging）
// ---------------------------------------------------------------------------

/// managed 模型的标准安装目录。
pub fn managed_install_dir(model: &RegistryModel) -> PathBuf {
    crate::config::settings::get_models_dir().join(&model.name)
}

/// 定位 managed 模型目录：主根（新根）优先，旧默认根兜底（双根兼容）。
///
/// 都不存在时返回 `None`（调用方回退主根路径，用于 NotInstalled 展示/安装目标）。
pub fn locate_managed_dir(name: &str) -> Option<PathBuf> {
    crate::model_library::install::ModelStorage::roots()
        .into_iter()
        .map(|root| root.join(name))
        .find(|dir| dir.exists())
}

/// 删除前路径安全校验并删除（目标必须在模型根之一内，且不能是根目录本身）。
///
/// 自定义 `data_dir` 后旧默认根 `~/.zapmomo/models` 下的存量安装同样可删。
pub fn delete_managed_dir(dir: &Path) -> Result<(), String> {
    let real = dir
        .canonicalize()
        .map_err(|e| format!("无法访问模型目录：{e}"))?;
    let mut allowed = false;
    for root in crate::model_library::install::ModelStorage::roots() {
        let real_root = root.canonicalize().unwrap_or(root);
        if real == real_root {
            return Err("拒绝删除：不能删除模型根目录本身".to_string());
        }
        if real.starts_with(&real_root) {
            allowed = true;
        }
    }
    if !allowed {
        return Err("拒绝删除：模型目录不在 ZapMomo 管理目录内".to_string());
    }
    if dir.is_dir() {
        std::fs::remove_dir_all(dir)
    } else {
        std::fs::remove_file(dir)
    }
    .map_err(|e| format!("删除模型文件失败：{e}"))
}

/// 按目录删除（供 CLI/测试复用）。
pub fn remove_dir_tree_checked(dir: &Path) -> Result<(), String> {
    delete_managed_dir(dir)
}

/// 由运行时路径推导安装目录：文件（LLM gguf）取父目录，目录（sherpa 模型目录）原样。
///
/// 供删除等命令把 `local_path`（双根定位后的实际位置）换算成安装目录。
pub fn runtime_to_install_dir(p: &Path) -> PathBuf {
    if p.is_dir() {
        p.to_path_buf()
    } else {
        p.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| p.to_path_buf())
    }
}

/// 删除 HF 安装目录（**只删该 artifact 的文件**），并清理空父目录（不误删同 repo 其他 variant）。
pub fn delete_hf_install_dir(dir: &Path) -> Result<(), String> {
    delete_managed_dir(dir)?;
    // 向上清理空目录：storageKey 层 → category 层
    let mut p = dir.parent().map(Path::to_path_buf);
    while let Some(d) = p {
        if is_empty_dir(&d) {
            let _ = std::fs::remove_dir(&d);
        } else {
            break;
        }
        p = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

fn is_empty_dir(p: &Path) -> bool {
    std::fs::read_dir(p)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false)
}

/// 安装 managed 模型：全部 required assets → staging → 整体校验 → commit → 写 metadata。
///
/// 原子提交单位是整个模型；任何 required asset 失败/校验失败/取消都会删除 staging，
/// 正式目录绝不出现半安装状态。optional assets（如 punctuation）在 commit 后 best-effort 安装。
pub fn install_managed_model(
    model: &RegistryModel,
    on_progress: &mut ProgressFn,
    cancel: Option<&AtomicBool>,
) -> Result<PathBuf, ModelError> {
    if model.download.is_none() {
        return Err(ModelError::Download("该模型没有内置下载源".to_string()));
    }
    if cancelled(cancel) {
        return Err(ModelError::Cancelled);
    }

    let (assets, total_bytes) = staged_assets(model)?;
    let final_dir = stage_and_commit(model, &assets, total_bytes, on_progress, cancel)?;

    // optional assets best-effort（独立目录，失败仅 warn，不回滚主模型）
    let mut progress = on_progress;
    for role in &model.optional_assets {
        let asset = match crate::kws::model::asset_by_role(role) {
            Some(a) => a,
            None => continue,
        };
        let dest = if role == "punctuation" {
            crate::kws::model::punctuation_user_model_dir()
        } else {
            final_dir.clone()
        };
        let required = registry::required_files_for_role(role);
        if let Err(e) = crate::kws::model::install_asset_to_cancellable(
            asset,
            &dest,
            false,
            &mut progress,
            required,
            cancel,
        ) {
            tracing::warn!("安装可选组件 {role} 失败（不影响主模型）: {e}");
        }
    }

    Ok(final_dir)
}

/// staging 安装清单条目：（资产, 安装完成所需文件名清单）。
type StagedAsset = (
    &'static crate::kws::model::ModelAsset,
    &'static [&'static str],
);

/// 收集 staging 安装清单与总字节：required 资产进清单，optional 资产只计入字节。
///
/// optional（如 ASR 的 punctuation）是独立目录的 tar.bz2，绝不能进 staging——
/// `extract_and_place` 的原子落位是「目标已存在先移除」，第二个 tar.bz2 落到同一
/// staging 目录会摧毁先解压的主模型文件，导致安装后完整性校验必然失败（ASR 模型库
/// 下载必失败的根因）。optional 的实际安装由 [`install_managed_model`] commit 后的
/// best-effort 循环装到各自独立目录。
fn staged_assets(model: &RegistryModel) -> Result<(Vec<StagedAsset>, u64), ModelError> {
    let mut total_bytes: u64 = 0;
    let mut assets: Vec<StagedAsset> = Vec::new();
    for role in model
        .required_assets
        .iter()
        .chain(model.optional_assets.iter())
    {
        let asset = crate::kws::model::asset_by_role(role)
            .ok_or_else(|| ModelError::Download(format!("未知资产 role：{role}")))?;
        total_bytes += asset.size_bytes;
        if model.required_assets.contains(role) {
            assets.push((asset, registry::required_files_for_role(role)));
        }
    }
    Ok((assets, total_bytes))
}

/// staging + 整体校验 + commit 的可测试核心：给定具体资产（可注入本地测试服务器）。
fn stage_and_commit<'a>(
    model: &RegistryModel,
    assets: &[(&'a crate::kws::model::ModelAsset, &'a [&'a str])],
    total_bytes: u64,
    on_progress: &mut ProgressFn,
    cancel: Option<&AtomicBool>,
) -> Result<PathBuf, ModelError> {
    let root = crate::config::settings::get_models_dir();
    let final_dir = root.join(&model.name);

    let install_root = root.join(".install");
    std::fs::create_dir_all(&install_root)?;
    let staging = install_root.join(format!("{}-{}", model.id, unique_suffix()));
    let staging_model = staging.join(&model.name);

    let done_bytes = Arc::new(AtomicU64::new(0));
    let mut progress = {
        let done_bytes = done_bytes.clone();
        let total = total_bytes;
        move |p: DownloadProgress| {
            let cur = done_bytes.load(Ordering::Relaxed) + p.bytes_downloaded;
            let overall = if total > 0 {
                ((cur as f64 / total as f64) * 100.0).min(100.0)
            } else {
                p.percent
            };
            on_progress(DownloadProgress {
                stage: p.stage,
                percent: overall,
                bytes_downloaded: cur,
                total_bytes: total,
                message: p.message,
            });
        }
    };

    // 1. required assets 安装到 staging
    let install_result = (|| -> Result<(), ModelError> {
        for (asset, required) in assets {
            if asset.is_raw() {
                let dest = staging_model.join(&asset.archive);
                crate::kws::model::install_raw_file_to_cancellable(
                    asset,
                    &dest,
                    false,
                    &mut progress,
                    cancel,
                )?;
            } else {
                crate::kws::model::install_asset_to_cancellable(
                    asset,
                    &staging_model,
                    false,
                    &mut progress,
                    required,
                    cancel,
                )?;
            }
            done_bytes.fetch_add(asset.size_bytes, Ordering::Relaxed);
        }
        // 2. 整体完整性校验（staging）
        // LLM 是 raw 单文件，必需文件 = `file_name`（避免为每个 LLM role 维护静态表）
        let required_files: Vec<&str> = if model.is_llm() {
            vec![model.file_name.as_deref().unwrap_or_default()]
        } else {
            model
                .required_assets
                .iter()
                .flat_map(|r| registry::required_files_for_role(r).iter().copied())
                .collect()
        };
        if !has_required_files(&staging_model, &required_files) {
            return Err(ModelError::Download("安装后完整性校验失败".to_string()));
        }
        Ok(())
    })();

    if let Err(e) = install_result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if cancelled(cancel) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ModelError::Cancelled);
    }

    // 3. commit：staging_model → final_dir（path-safety 后移除旧 Invalid 目录）
    if final_dir.exists() {
        delete_managed_dir(&final_dir).map_err(|e| ModelError::Io(std::io::Error::other(e)))?;
    }
    std::fs::rename(&staging_model, &final_dir)?;
    let _ = std::fs::remove_dir_all(&staging);

    // 4. 写 managed 元数据
    ensure_managed_meta(model, &final_dir);

    Ok(final_dir)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    #[test]
    fn test_runtime_status_matrix() {
        let a = Path::new("/models/A");
        let b = Path::new("/models/B");
        // KWS 正在监听 A、selected=A → Active（不 Pending）
        assert_eq!(
            runtime_status(Some(a), Some(a), true, false, false),
            RuntimeStatus::Active
        );
        // selected=B, actual=A, running → PendingRestart
        assert_eq!(
            runtime_status(Some(b), Some(a), true, false, false),
            RuntimeStatus::PendingRestart
        );
        // selected=B, actual=None → Inactive（不是 Pending）
        assert_eq!(
            runtime_status(Some(b), None, false, false, false),
            RuntimeStatus::Inactive
        );
        // actual 同 selected 但 running=false → Inactive（异常态不显示 Active）
        assert_eq!(
            runtime_status(Some(a), Some(a), false, false, false),
            RuntimeStatus::Inactive
        );
        // LLM selected=B, loaded=None + load_failed → LoadFailed
        assert_eq!(
            runtime_status(Some(b), None, false, false, true),
            RuntimeStatus::LoadFailed
        );
        // Switching
        assert_eq!(
            runtime_status(Some(b), None, false, true, false),
            RuntimeStatus::Switching
        );
    }

    /// 测试用最小 TTS LibraryModel（enrich 只读 current / model_type / local_path）。
    fn tts_library_model(current: bool) -> LibraryModel {
        LibraryModel {
            id: "tts-zipvoice-distill-int8".to_string(),
            name: "sherpa-onnx-zipvoice-distill-int8-zh-en-emilia".to_string(),
            display_name: "ZipVoice TTS zh-en".to_string(),
            model_type: ModelType::Tts,
            runtime: "sherpa-onnx".to_string(),
            format: "ONNX".to_string(),
            description: String::new(),
            languages: vec![],
            tags: vec![],
            parameter_count: None,
            quantization: None,
            version: "distill-int8".to_string(),
            size_bytes: None,
            homepage: None,
            downloadable: true,
            source: ModelSource::Registry,
            ownership: StorageOwnership::Managed,
            install_state: InstallState::Installed,
            current,
            runtime_status: RuntimeStatus::Inactive,
            local_path: Some("/models/zipvoice".to_string()),
            installed_at: None,
            install_id: None,
            repo_id: None,
            compatibility: None,
        }
    }

    #[test]
    fn test_enrich_runtime_status_tts() {
        let dir = Path::new("/models/zipvoice");
        // 合成中：当前模型 Active、非当前恒 Inactive
        let mut models = vec![tts_library_model(true), tts_library_model(false)];
        let actuals = RuntimeActuals {
            kws: None,
            asr: None,
            tts: Some(dir),
            tts_active: true,
            llm: None,
            llm_switching: false,
            llm_switch_target: None,
            llm_load_error_path: None,
        };
        enrich_runtime_status(&mut models, &actuals);
        assert_eq!(models[0].runtime_status, RuntimeStatus::Active);
        assert_eq!(models[1].runtime_status, RuntimeStatus::Inactive);

        // 空闲（无合成线程）：当前模型 Inactive
        let mut models = vec![tts_library_model(true)];
        let actuals = RuntimeActuals {
            tts_active: false,
            ..actuals
        };
        enrich_runtime_status(&mut models, &actuals);
        assert_eq!(models[0].runtime_status, RuntimeStatus::Inactive);
    }

    #[test]
    fn test_current_selection_and_set_restore() {
        run_with_temp_home(|home| {
            // set/restore KWS selection（LLM 已改远程连接，无本地 selection 语义）
            // 用临时 HOME 下的绝对路径（不要硬编码 Unix 风格 /tmp，Windows 上 is_absolute 语义不同）。
            let x = home.join("models/x-kws");
            set_selected_model(ModelType::Kws, &x).unwrap();
            let s = current_selections();
            assert!(paths_equal(s.kws.as_ref().unwrap(), &x));
            assert!(!s.kws.as_ref().unwrap().is_dir(), "不应要求目录存在");

            let y = home.join("models/y-kws");
            restore_selected_model(ModelType::Kws, Some(y.display().to_string())).unwrap();
            let s2 = current_selections();
            assert!(paths_equal(s2.kws.as_ref().unwrap(), &y));

            let cfg = settings::load_settings().unwrap().unwrap();
            // set/restore 不触碰 enabled
            assert_eq!(cfg.kws.unwrap().enabled, None);
            assert!(cfg.tts.is_none());
        });
    }

    #[test]
    fn test_set_selected_kws_resets_file_overrides() {
        run_with_temp_home(|home| {
            // 预写旧模型的文件级覆盖（模拟 zh-en 时代的手写配置）
            update_settings(|cfg| {
                let kws = cfg.kws.get_or_insert_with(Default::default);
                kws.model_dir = Some("old-model".to_string());
                kws.encoder = Some("old-encoder.onnx".to_string());
                kws.decoder = Some("old-decoder.onnx".to_string());
                kws.joiner = Some("old-joiner.onnx".to_string());
                kws.tokens = Some("old-tokens.txt".to_string());
                kws.keywords_file = Some("old-keywords.txt".to_string());
                kws.enabled = Some(true);
            })
            .unwrap();

            // 切换到新模型目录
            let new_dir = home.join("models/new-kws");
            set_selected_model(ModelType::Kws, &new_dir).unwrap();

            let cfg = settings::load_settings().unwrap().unwrap();
            let kws = cfg.kws.as_ref().expect("kws 段应存在");
            assert_eq!(
                kws.model_dir,
                Some(new_dir.to_string_lossy().to_string()),
                "model_dir 应更新"
            );
            // 文件级覆盖全部重置：交回 resolve 按目录探测
            assert_eq!(kws.encoder, None);
            assert_eq!(kws.decoder, None);
            assert_eq!(kws.joiner, None);
            assert_eq!(kws.tokens, None);
            assert_eq!(kws.keywords_file, None);
            // enabled 不受切换影响
            assert_eq!(kws.enabled, Some(true));
        });
    }

    #[test]
    fn test_set_selected_asr_resets_file_overrides() {
        run_with_temp_home(|home| {
            // 预写旧模型的文件级覆盖（模拟双语时代的手写配置）
            update_settings(|cfg| {
                let asr = cfg.asr.get_or_insert_with(Default::default);
                asr.model_dir = Some("old-model".to_string());
                asr.encoder = Some("old-encoder.onnx".to_string());
                asr.decoder = Some("old-decoder.onnx".to_string());
                asr.joiner = Some("old-joiner.onnx".to_string());
                asr.tokens = Some("old-tokens.txt".to_string());
                asr.enabled = Some(true);
            })
            .unwrap();

            // 切换到新模型目录
            let new_dir = home.join("models/zh-14m");
            set_selected_model(ModelType::Asr, &new_dir).unwrap();

            let cfg = settings::load_settings().unwrap().unwrap();
            let asr = cfg.asr.as_ref().expect("asr 段应存在");
            assert_eq!(
                asr.model_dir,
                Some(new_dir.to_string_lossy().to_string()),
                "model_dir 应更新"
            );
            // 文件级覆盖全部重置：交回 resolve 按目录探测
            assert_eq!(asr.encoder, None);
            assert_eq!(asr.decoder, None);
            assert_eq!(asr.joiner, None);
            assert_eq!(asr.tokens, None);
            // enabled 不受切换影响
            assert_eq!(asr.enabled, Some(true));
        });
    }

    #[test]
    fn test_set_selected_tts_resets_file_overrides() {
        run_with_temp_home(|home| {
            // 预写旧模型的文件级覆盖（模拟手工改过的配置）
            update_settings(|cfg| {
                let tts = cfg.tts.get_or_insert_with(Default::default);
                tts.model_dir = Some("old-model".to_string());
                tts.encoder = Some("old-encoder.onnx".to_string());
                tts.decoder = Some("old-decoder.onnx".to_string());
                tts.vocoder = Some("old-vocoder.onnx".to_string());
                tts.tokens = Some("old-tokens.txt".to_string());
                tts.lexicon = Some("old-lexicon.txt".to_string());
                tts.data_dir = Some("old-espeak-ng-data".to_string());
                tts.reference_wav = Some("old-ref.wav".to_string());
                tts.reference_text = Some("旧参考文本".to_string());
                tts.enabled = Some(true);
                tts.voice = Some("leijun-1".to_string());
            })
            .unwrap();

            // 切换到新模型目录
            let new_dir = home.join("models/zipvoice");
            set_selected_model(ModelType::Tts, &new_dir).unwrap();

            let cfg = settings::load_settings().unwrap().unwrap();
            let tts = cfg.tts.as_ref().expect("tts 段应存在");
            assert_eq!(
                tts.model_dir,
                Some(new_dir.to_string_lossy().to_string()),
                "model_dir 应更新"
            );
            // 文件级覆盖全部重置：交回 resolve 按目录探测
            assert_eq!(tts.encoder, None);
            assert_eq!(tts.decoder, None);
            assert_eq!(tts.vocoder, None);
            assert_eq!(tts.tokens, None);
            assert_eq!(tts.lexicon, None);
            assert_eq!(tts.data_dir, None);
            // reference_wav/text 是旧模型目录内的参考音频，一并重置回默认音色
            assert_eq!(tts.reference_wav, None);
            assert_eq!(tts.reference_text, None);
            // enabled / 音色偏好 / 参数不受切换影响
            assert_eq!(tts.enabled, Some(true));
            assert_eq!(tts.voice, Some("leijun-1".to_string()));
        });
    }

    #[test]
    fn test_set_selected_tts_persists_model_type_from_registry_name() {
        run_with_temp_home(|home| {
            // managed 安装目录名 == registry `name` → 切换时推导并持久化对应 kind
            let omni = home.join("models/omnivoice-audiocpp");
            set_selected_model(ModelType::Tts, &omni).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.tts.as_ref().and_then(|t| t.model_type),
                Some(crate::tts::config::TtsModelKind::Omnivoice)
            );

            let zip = home.join("models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia");
            set_selected_model(ModelType::Tts, &zip).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.tts.as_ref().and_then(|t| t.model_type),
                Some(crate::tts::config::TtsModelKind::Zipvoice)
            );

            // 非 registry 目录名：不写错 kind（保持上一次推导值）
            let unknown = home.join("models/my-local-model");
            set_selected_model(ModelType::Tts, &unknown).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.tts.as_ref().and_then(|t| t.model_type),
                Some(crate::tts::config::TtsModelKind::Zipvoice),
                "未知目录不应覆盖已推导的 kind"
            );
        });
    }

    #[test]
    fn test_set_selected_tts_persists_backend_from_registry_runtime() {
        run_with_temp_home(|home| {
            // audiocpp managed 目录 → backend = audiocpp + model_type = omnivoice
            let omni = home.join("models/omnivoice-audiocpp");
            set_selected_model(ModelType::Tts, &omni).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            let tts = cfg.tts.as_ref().expect("tts 段应存在");
            assert_eq!(tts.backend.as_deref(), Some("audiocpp"));
            assert_eq!(
                tts.model_type,
                Some(crate::tts::config::TtsModelKind::Omnivoice)
            );

            // 切回 sherpa zipvoice → backend 复位缺省（None）
            let zip = home.join("models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia");
            set_selected_model(ModelType::Tts, &zip).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(cfg.tts.as_ref().and_then(|t| t.backend.clone()), None);

            // audiocpp → external/local 目录 → backend 同样复位（不残留 audiocpp 拦合成）
            set_selected_model(ModelType::Tts, &omni).unwrap();
            let unknown = home.join("models/my-local-model");
            set_selected_model(ModelType::Tts, &unknown).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(cfg.tts.as_ref().and_then(|t| t.backend.clone()), None);
        });
    }

    #[test]
    fn test_set_selected_tts_kind_switch_clears_voice() {
        run_with_temp_home(|home| {
            // audiocpp registry 目录名 → tts_kind = omnivoice
            let omni = home.join("models/omnivoice-audiocpp");
            set_selected_model(ModelType::Tts, &omni).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.tts.as_ref().and_then(|t| t.model_type),
                Some(crate::tts::config::TtsModelKind::Omnivoice)
            );
            // 设置一个 omnivoice 音色后切回 zipvoice：音色 id 互为无效，应被清空
            update_settings(|c| {
                c.tts.get_or_insert_with(Default::default).voice = Some("demo_01_man".to_string());
            })
            .unwrap();
            let zip = home.join("models/sherpa-onnx-zipvoice-distill-int8-zh-en-emilia");
            set_selected_model(ModelType::Tts, &zip).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            let tts = cfg.tts.as_ref().unwrap();
            assert_eq!(
                tts.model_type,
                Some(crate::tts::config::TtsModelKind::Zipvoice)
            );
            assert!(
                tts.voice.is_none(),
                "模型族变化时应清空默认音色（旧族音色 id 在新族无效）"
            );
        });
    }

    #[test]
    fn test_set_selected_asr_persists_model_type_from_registry_name() {
        run_with_temp_home(|home| {
            // managed 安装目录名 == registry `name` → 切换时推导并持久化对应 kind
            // （sherpa Qwen3-ASR 已移除，registry 中带 asr_kind 的是 audiocpp 条目）
            let qwen3 = home.join("models/qwen3-asr-0.6b-audiocpp");
            set_selected_model(ModelType::Asr, &qwen3).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.asr.as_ref().and_then(|a| a.model_type),
                Some(crate::asr::config::AsrModelKind::Qwen3Asr)
            );

            // streaming zipformer：asr_kind 缺省 None → 复位 model_type，
            // 交回 resolve 按目录内容探测（残留 qwen3 的 Some 值会用旧探针
            // 校验新目录，误报「模型文件缺失」）
            let zip =
                home.join("models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20");
            set_selected_model(ModelType::Asr, &zip).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.asr.as_ref().and_then(|a| a.model_type),
                None,
                "无 asr_kind 的 streaming 目录应复位 model_type 交回目录探测"
            );

            // 已从模型库移除的模型目录（老用户已装/外部导入）：registry 反查
            // 不到 → model_type 保持 None，同样交回目录探测兜底，识别仍可用
            let removed = home.join("models/sherpa-onnx-whisper-tiny");
            set_selected_model(ModelType::Asr, &removed).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.asr.as_ref().and_then(|a| a.model_type),
                None,
                "已移除的 registry 模型目录不得持久化 model_type"
            );

            // 文件级覆盖与族专属参数全部重置
            let a = cfg.asr.as_ref().unwrap();
            assert_eq!(a.encoder, None);
            assert_eq!(a.decoder, None);
            assert_eq!(a.joiner, None);
            assert_eq!(a.tokens, None);
            assert_eq!(a.language, None);
            assert_eq!(a.use_itn, None);
        });
    }

    /// 回归：qwen3（asr_kind=Some）→ streaming zipformer（asr_kind=None）切换后，
    /// settings 不得残留旧族 model_type——残留会让 resolve 用 qwen3 探针
    /// 校验 zipformer 目录，误报「模型文件缺失」（2026-08-28 用户实测）。
    /// 无权威 kind 时应复位为 None，交回 resolve 按目录内容探测。
    #[test]
    fn test_set_selected_asr_zipformer_from_qwen3_resets_stale_kind() {
        run_with_temp_home(|home| {
            // 前置：切到 audiocpp qwen3（asr_kind=Some(qwen3_asr)、backend=audiocpp 落盘）
            let qwen3 = home.join("models/qwen3-asr-0.6b-audiocpp");
            set_selected_model(ModelType::Asr, &qwen3).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.asr.as_ref().and_then(|a| a.model_type),
                Some(crate::asr::config::AsrModelKind::Qwen3Asr),
                "前置：qwen3 条目应持久化 model_type"
            );

            // 切到真实布局的 streaming zipformer 目录（registry 无 asr_kind）
            let zip =
                home.join("models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20");
            std::fs::create_dir_all(&zip).unwrap();
            for name in crate::asr::config::REQUIRED_FILES {
                std::fs::write(zip.join(name), b"x").unwrap();
            }
            set_selected_model(ModelType::Asr, &zip).unwrap();

            let cfg = settings::load_settings().unwrap().unwrap();
            let asr = cfg.asr.as_ref().unwrap();
            assert_eq!(
                asr.model_type, None,
                "无权威 kind 应复位 model_type 交回目录探测，而非残留 qwen3_asr"
            );
            assert_eq!(asr.backend, None, "从 audiocpp 切回 sherpa 条目后端应归位");

            // 端到端锚点：resolve 后按目录探测出 Zipformer 且 models_present=true
            let resolved = crate::asr::config::resolve(cfg.asr.as_ref(), None).unwrap();
            assert_eq!(
                resolved.model_type,
                crate::asr::config::AsrModelKind::Zipformer
            );
            assert!(
                crate::asr::config::models_present(&resolved),
                "四件套齐全的 zipformer 目录不应误报模型缺失"
            );
        });
    }

    #[test]
    fn test_set_selected_asr_audiocpp_backend_and_hotwords() {
        run_with_temp_home(|home| {
            // 先切默认 zipformer（sherpa 侧）并配上热词
            // （sherpa Qwen3-ASR 已移除，zipformer 是仅存的 sherpa ASR 条目）
            let sherpa =
                home.join("models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20");
            set_selected_model(ModelType::Asr, &sherpa).unwrap();
            update_settings(|cfg| {
                let asr = cfg.asr.get_or_insert_with(Default::default);
                asr.hotwords = Some("甲乙方 违约金".to_string());
            })
            .unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            let a = cfg.asr.as_ref().unwrap();
            assert_eq!(a.model_type, None, "zipformer 条目 asr_kind 缺省应复位");
            assert_eq!(a.backend, None, "sherpa 条目后端归位缺省");

            // 切 audiocpp Qwen3-ASR：backend 写 audiocpp + 热词清空（上游无热词能力）
            let acpp = home.join("models/qwen3-asr-0.6b-audiocpp");
            set_selected_model(ModelType::Asr, &acpp).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            let a = cfg.asr.as_ref().unwrap();
            assert_eq!(
                a.model_type,
                Some(crate::asr::config::AsrModelKind::Qwen3Asr)
            );
            assert_eq!(a.backend.as_deref(), Some("audiocpp"));
            assert_eq!(a.hotwords, None, "切到 audiocpp 应清空热词");
            assert_eq!(a.engine_path, None);

            // 切回 sherpa：backend 复位 None（热词不恢复，用户重新配置）
            set_selected_model(ModelType::Asr, &sherpa).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(cfg.asr.as_ref().and_then(|a| a.backend.as_deref()), None);

            // external 目录：含 audiocpp 族 GGUF → 自动识别 audiocpp 后端
            let ext = home.join("models-local/my-qwen3-asr");
            std::fs::create_dir_all(&ext).unwrap();
            std::fs::write(
                ext.join(crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file),
                b"x",
            )
            .unwrap();
            set_selected_model(ModelType::Asr, &ext).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(
                cfg.asr.as_ref().and_then(|a| a.backend.as_deref()),
                Some("audiocpp"),
                "external 目录含 GGUF 应自动识别 audiocpp"
            );

            // external 目录无 GGUF → 后端复位缺省（残留 audiocpp 会拦住 sherpa 识别）
            let ext2 = home.join("models-local/plain-sherpa");
            std::fs::create_dir_all(&ext2).unwrap();
            set_selected_model(ModelType::Asr, &ext2).unwrap();
            let cfg = settings::load_settings().unwrap().unwrap();
            assert_eq!(cfg.asr.as_ref().and_then(|a| a.backend.as_deref()), None);
        });
    }

    #[test]
    fn test_external_remove_never_deletes_file() {
        run_with_temp_home(|home| {
            let dir = home.join("models-local");
            std::fs::create_dir_all(&dir).unwrap();
            let model_dir = dir.join("keepme-kws");
            std::fs::create_dir_all(&model_dir).unwrap();
            let record = LocalModel {
                id: "local-keepme".to_string(),
                name: "keepme-kws".to_string(),
                model_type: "kws".to_string(),
                path: model_dir.display().to_string(),
                added_at: "2026-08-27T00:00:00Z".to_string(),
                registry_id: None,
            };
            add_local_model_record(record).unwrap();
            remove_local_model_record("local-keepme").unwrap();
            // 原始目录仍在
            assert!(model_dir.is_dir());
            assert!(get_local_models().is_empty());
        });
    }

    #[test]
    fn test_duplicate_registry_binding_replaced() {
        run_with_temp_home(|home| {
            let dir = home.join("models-local");
            std::fs::create_dir_all(&dir).unwrap();
            let a = dir.join("a-kws");
            let b = dir.join("b-kws");
            std::fs::create_dir_all(&a).unwrap();
            std::fs::create_dir_all(&b).unwrap();
            let mk = |dir: &std::path::Path| LocalModel {
                id: format!("local-{}", dir.file_name().unwrap().to_string_lossy()),
                name: "binding".to_string(),
                model_type: "kws".to_string(),
                path: dir.display().to_string(),
                added_at: "2026-08-27T00:00:00Z".to_string(),
                registry_id: Some("kws-zipformer-zh-en-3m".to_string()),
            };
            add_local_model_record(mk(&a)).unwrap();
            // 第二次导入另一个目录 → 重新关联（旧绑定被替换）
            add_local_model_record(mk(&b)).unwrap();
            let records = get_local_models();
            let bindings: Vec<_> = records
                .iter()
                .filter(|l| l.registry_id.as_deref() == Some("kws-zipformer-zh-en-3m"))
                .collect();
            assert_eq!(bindings.len(), 1, "同一 registry_id 只允许一条绑定");
            assert!(bindings[0].path.ends_with("b-kws"));
        });
    }

    #[test]
    fn test_delete_managed_dir_safety() {
        run_with_temp_home(|home| {
            let models = home.join(".zapmomo/models");
            std::fs::create_dir_all(&models).unwrap();
            let inside = models.join("some-model");
            std::fs::create_dir_all(&inside).unwrap();
            delete_managed_dir(&inside).unwrap();
            assert!(!inside.exists());

            // 目录外的路径被拒绝
            let outside = home.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            let err = delete_managed_dir(&outside).unwrap_err();
            assert!(err.contains("管理目录内"));
        });
    }

    /// 回归：optional 资产（如 punctuation）绝不能进 staging 安装清单。
    ///
    /// 进了会因 extract_and_place「目标已存在先移除」的原子落位摧毁主模型文件，
    /// 曾导致模型库下载任何 ASR 模型都在「安装后完整性校验失败」处必败。
    #[test]
    fn test_staged_assets_excludes_optional() {
        // ASR：required 1 个 + optional punctuation → staging 只装 required，字节两者都计
        let asr = registry::model_by_id("asr-streaming-bilingual-zh-en").unwrap();
        let (assets, total) = staged_assets(asr).unwrap();
        assert_eq!(assets.len(), 1, "punctuation 不得进 staging 清单");
        assert_eq!(assets[0].0.role, "asr");
        let req = crate::kws::model::asset_by_role("asr").unwrap().size_bytes;
        let opt = crate::kws::model::asset_by_role("punctuation")
            .unwrap()
            .size_bytes;
        assert_eq!(total, req + opt, "optional 只计字节（进度总量）");

        // KWS（无 optional）单资产；TTS（tts + tts-vocoder 双 required）两资产
        let (kws_assets, _) =
            staged_assets(registry::model_by_id("kws-zipformer-zh-en-3m").unwrap()).unwrap();
        assert_eq!(kws_assets.len(), 1);
        let (tts_assets, _) =
            staged_assets(registry::model_by_id("tts-zipvoice-distill-int8").unwrap()).unwrap();
        assert_eq!(tts_assets.len(), 2);
    }

    fn test_reg_model(name: &str, id: &str) -> RegistryModel {
        RegistryModel {
            id: id.to_string(),
            name: name.to_string(),
            display_name: name.to_string(),
            model_type: ModelType::Kws,
            tts_kind: None,
            asr_kind: None,
            runtime: "sherpa-onnx".into(),
            format: "ONNX".into(),
            description: String::new(),
            languages: Vec::new(),
            tags: Vec::new(),
            parameter_count: None,
            quantization: None,
            file_name: None,
            version: "test".into(),
            size_bytes: None,
            homepage: None,
            required_assets: vec!["wake-word".into()],
            optional_assets: Vec::new(),
            platforms: None,
            download: None,
        }
    }

    /// LLM 预设安装布局：raw 单文件落在 `<models>/<name>/<file_name>`，带 managed 元数据。
    /// `download_llm_model` 的幂等预检（managed_install_dir + file_name）依赖该布局。
    #[test]
    fn test_install_llm_raw_layout() {
        use crate::kws::model::tests::{serve_many, sha256_hex};

        run_with_temp_home(|_| {
            // 与真实预设同构：registry.name = 安装目录名，file_name = manifest archive 文件名
            let bytes = b"GGUF-test-payload".to_vec();
            let asset = crate::kws::model::ModelAsset {
                name: "Qwen3-0.6B".into(),
                role: "llm-test-raw".into(),
                version: "test".into(),
                kind: Some("raw".into()),
                archive: "Qwen3-0.6B-Q4_K_M.gguf".into(),
                source: serve_many(bytes.clone()),
                sha256: sha256_hex(&bytes),
                size_bytes: bytes.len() as u64,
                license: "Apache-2.0".into(),
            };
            let mut reg = test_reg_model("Qwen3-0.6B", "llm-raw-test");
            reg.model_type = ModelType::Llm;
            reg.runtime = "llama.cpp".into();
            reg.format = "GGUF".into();
            reg.file_name = Some("Qwen3-0.6B-Q4_K_M.gguf".into());
            reg.required_assets = vec!["llm-test-raw".into()];

            let final_dir =
                stage_and_commit(&reg, &[(&asset, &[])], asset.size_bytes, &mut |_| {}, None)
                    .unwrap();
            let final_file = final_dir.join("Qwen3-0.6B-Q4_K_M.gguf");
            assert!(
                final_file.is_file(),
                "GGUF 应落在 <models>/<name>/<file_name>"
            );
            assert_eq!(std::fs::read(&final_file).unwrap(), bytes);
            assert!(final_dir.join(".zapmomo-lib.json").is_file());
            // command 幂等预检用 managed_install_dir(model).join(file_name)，必须与落盘位置一致
            assert!(paths_equal(
                &final_file,
                &managed_install_dir(&reg).join("Qwen3-0.6B-Q4_K_M.gguf")
            ));
        });
    }

    /// staging 保证：任一 required asset 失败，正式模型目录不得出现半安装状态。
    #[test]
    fn test_install_staging_failure_leaves_no_partial() {
        use crate::kws::model::tests::{mini_tarbz2, serve_many, sha256_hex};
        use crate::kws::model::{KWS_REQUIRED_FILES, ModelAsset, is_installed};

        run_with_temp_home(|_| {
            let bytes = mini_tarbz2("test-kws-model");
            let url = serve_many(bytes.clone());
            let mk_asset = |sha: String, archive: &str| ModelAsset {
                name: "test-kws-model".into(),
                role: "wake-word".into(),
                version: "test".into(),
                kind: None,
                archive: archive.into(),
                source: url.clone(),
                sha256: sha,
                size_bytes: bytes.len() as u64,
                license: "Apache-2.0".into(),
            };

            // 成功：完整安装 + 写 metadata
            let good = mk_asset(sha256_hex(&bytes), "mini.tar.bz2");
            let reg_ok = test_reg_model("test-kws-model", "kws-test-ok");
            let dest = stage_and_commit(
                &reg_ok,
                &[(&good, &KWS_REQUIRED_FILES)],
                good.size_bytes,
                &mut |_| {},
                None,
            )
            .unwrap();
            assert!(is_installed(&dest));
            assert!(dest.join(".zapmomo-lib.json").is_file());

            // 失败：sha 不匹配 → 正式目录绝不能出现，staging 被清理
            let bad = mk_asset("0".repeat(64), "mini.tar.bz2");
            let reg_bad = test_reg_model("test-kws-model-bad", "kws-test-bad");
            let err = stage_and_commit(
                &reg_bad,
                &[(&bad, &KWS_REQUIRED_FILES)],
                bad.size_bytes,
                &mut |_| {},
                None,
            )
            .unwrap_err();
            assert!(matches!(err, ModelError::Sha256Mismatch { .. }));
            let final_bad = crate::config::settings::get_models_dir().join("test-kws-model-bad");
            assert!(!final_bad.exists(), "失败不得留下正式目录");
            let install_root = crate::config::settings::get_models_dir().join(".install");
            let leftovers = std::fs::read_dir(&install_root)
                .map(|it| it.filter_map(Result::ok).count())
                .unwrap_or(0);
            assert_eq!(leftovers, 0, "staging 应被清理干净");
        });
    }

    /// legacy managed 识别：完整目录无 metadata → Installed；metadata 补写失败仍 Installed。
    #[test]
    fn test_legacy_managed_recognition_and_metadata_best_effort() {
        run_with_temp_home(|home| {
            // 用 KWS 资产字段摆一个「完整但无 metadata」的旧目录
            use crate::kws::config::{
                DEFAULT_DECODER, DEFAULT_ENCODER, DEFAULT_JOINER, DEFAULT_KEYWORDS_REL,
                DEFAULT_TOKENS,
            };
            let models = home.join(".zapmomo/models");
            let dest = models.join("sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20");
            std::fs::create_dir_all(dest.join("test_wavs")).unwrap();
            for f in [
                DEFAULT_ENCODER,
                DEFAULT_DECODER,
                DEFAULT_JOINER,
                DEFAULT_TOKENS,
            ] {
                std::fs::write(dest.join(f), b"x").unwrap();
            }
            std::fs::write(dest.join(DEFAULT_KEYWORDS_REL), b"k").unwrap();

            let models_list = list_models();
            let m = models_list
                .iter()
                .find(|m| m.id == "kws-zipformer-zh-en-3m")
                .unwrap();
            assert_eq!(
                m.install_state,
                InstallState::Installed,
                "legacy 完整目录应识别为已安装"
            );
            // 补写 metadata 成功（best-effort）
            assert!(dest.join(".zapmomo-lib.json").is_file());
        });
    }

    // ---- 双根兼容（data_dir 自定义后旧根存量可见可删）----

    /// 设置自定义 data_dir，返回数据目录。
    fn set_custom_data_dir(home: &Path) -> PathBuf {
        let data = home.join("zapdata");
        let mut config = crate::config::settings::AppConfig::default();
        config.data_dir = Some(data.display().to_string());
        crate::config::settings::save_settings(&config).unwrap();
        data
    }

    /// 在指定目录造一个带 meta 的 HF 安装（install_id 可控）。
    fn make_hf_install_at(dir: &Path, install_id: &str, model_id: &str) -> PathBuf {
        use crate::model_library::install::{InstallMeta, META_SCHEMA_VERSION, ModelStorage};
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("model.gguf"), b"GGUFxxxxx").unwrap();
        let meta = InstallMeta {
            schema_version: META_SCHEMA_VERSION,
            install_id: install_id.into(),
            source: "hf".into(),
            model_id: model_id.into(),
            repo_id: Some(model_id.into()),
            revision: Some("main".into()),
            model_type: "llm".into(),
            artifact_id: "a".into(),
            variant: None,
            architecture: Some("llama-cpp-gguf".into()),
            installed_at: "2026-08-17T00:00:00Z".into(),
            registry_id: None,
            version: None,
            managed: Some(true),
        };
        ModelStorage::write_meta(dir, &meta).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn test_scan_dual_root_dedup_prefers_new_root() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            // 双根同 install_id（用户手动复制场景）：只显示新根一份
            let legacy = home.join(".zapmomo/models/llm/k--m/a");
            make_hf_install_at(&legacy, "install-dup", "m1");
            let new = data.join("models/llm/k--m/a");
            make_hf_install_at(&new, "install-dup", "m1");
            let installs = crate::model_library::install::ModelStorage::scan_installs();
            assert_eq!(installs.len(), 1, "同 install_id 双根只保留一份");
            assert_eq!(installs[0].0, new, "新根优先");
        });
    }

    #[test]
    fn test_scan_finds_legacy_root_only_installs() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let legacy = home.join(".zapmomo/models/llm/k--m/b");
            make_hf_install_at(&legacy, "install-legacy-only", "m2");
            let installs = crate::model_library::install::ModelStorage::scan_installs();
            assert_eq!(installs.len(), 1, "旧根存量应被扫描到");
            assert_eq!(installs[0].0, legacy);
        });
    }

    #[test]
    fn test_delete_accepts_legacy_root_when_custom() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let legacy = home.join(".zapmomo/models/legacy-model");
            std::fs::create_dir_all(&legacy).unwrap();
            std::fs::write(legacy.join("f.onnx"), b"x").unwrap();
            delete_managed_dir(&legacy).unwrap();
            assert!(!legacy.exists());
        });
    }

    #[test]
    fn test_delete_rejects_models_root_itself() {
        run_with_temp_home(|home| {
            std::fs::create_dir_all(home.join(".zapmomo/models")).unwrap();
            let root = crate::config::settings::get_models_dir();
            let err = delete_managed_dir(&root).unwrap_err();
            assert!(err.contains("拒绝"), "删除根目录本身必须被拒绝：{err}");
        });
    }

    #[test]
    fn test_locate_managed_dir_prefers_new_root_then_legacy() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            // 只在旧根 → 定位到旧根
            let legacy_dir = home.join(".zapmomo/models/reg-model-a");
            std::fs::create_dir_all(&legacy_dir).unwrap();
            assert_eq!(locate_managed_dir("reg-model-a"), Some(legacy_dir.clone()));
            // 新根出现 → 定位切到新根
            let new_dir = data.join("models/reg-model-a");
            std::fs::create_dir_all(&new_dir).unwrap();
            assert_eq!(locate_managed_dir("reg-model-a"), Some(new_dir));
            // 都没有 → None
            assert_eq!(locate_managed_dir("reg-model-none"), None);
        });
    }

    #[test]
    fn test_runtime_to_install_dir() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            // 文件 → 父目录（LLM gguf 场景）
            let f = dir.join("model.gguf");
            std::fs::write(&f, b"x").unwrap();
            assert_eq!(runtime_to_install_dir(&f), dir.clone());
            // 目录 → 原样（sherpa 模型目录场景）
            assert_eq!(runtime_to_install_dir(&dir), dir.clone());
            // 不存在的文件 → 按文件分支取父目录
            let nf = dir.join("none.gguf");
            assert_eq!(runtime_to_install_dir(&nf), dir);
        });
    }

    #[test]
    fn test_registry_model_in_legacy_root_listed_installed() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            // 旧版默认根下摆一个完整 KWS 目录（同 legacy 识别测试的摆法）
            use crate::kws::config::{
                DEFAULT_DECODER, DEFAULT_ENCODER, DEFAULT_JOINER, DEFAULT_KEYWORDS_REL,
                DEFAULT_TOKENS,
            };
            let dest = home.join(".zapmomo/models/sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20");
            std::fs::create_dir_all(dest.join("test_wavs")).unwrap();
            for f in [
                DEFAULT_ENCODER,
                DEFAULT_DECODER,
                DEFAULT_JOINER,
                DEFAULT_TOKENS,
            ] {
                std::fs::write(dest.join(f), b"x").unwrap();
            }
            std::fs::write(dest.join(DEFAULT_KEYWORDS_REL), b"k").unwrap();

            let models_list = list_models();
            let m = models_list
                .iter()
                .find(|m| m.id == "kws-zipformer-zh-en-3m")
                .unwrap();
            assert_eq!(
                m.install_state,
                InstallState::Installed,
                "旧根存量应识别为已安装"
            );
            assert!(
                m.local_path
                    .as_deref()
                    .is_some_and(|p| p.contains(".zapmomo")),
                "local_path 应指向旧根实际位置"
            );
        });
    }

    /// current 判定可用于 external：设置为 current 后 `is_path_current` 为真。
    #[test]
    fn test_is_path_current_for_external() {
        run_with_temp_home(|home| {
            let dir = home.join("models-local");
            std::fs::create_dir_all(&dir).unwrap();
            let model_dir = dir.join("current-kws");
            std::fs::create_dir_all(&model_dir).unwrap();
            add_local_model_record(LocalModel {
                id: "local-current".to_string(),
                name: "current-kws".to_string(),
                model_type: "kws".to_string(),
                path: model_dir.display().to_string(),
                added_at: "2026-08-27T00:00:00Z".to_string(),
                registry_id: None,
            })
            .unwrap();
            set_selected_model(ModelType::Kws, &model_dir).unwrap();
            assert!(is_path_current(ModelType::Kws, &model_dir));
            // 切换走后不再是 current（供 command 层「移除前需先切换」判定使用）
            let other = home.join("models-local/other-kws");
            set_selected_model(ModelType::Kws, &other).unwrap();
            assert!(!is_path_current(ModelType::Kws, &model_dir));
        });
    }

    /// 造一个 HF 安装（meta v2 + gguf 文件），返回 (install_dir, runtime_path, install_id)。
    fn make_hf_install(repo: &str, variant: &str, gguf_name: &str) -> (PathBuf, PathBuf, String) {
        use crate::model_library::catalog::ModelCategory;
        use crate::model_library::install::ArtifactSource;
        use crate::model_library::install::{
            InstallMeta, META_SCHEMA_VERSION, ModelStorage, derive_install_id,
        };

        let artifact_id = format!("{repo}-{variant}");
        let dir = ModelStorage::install_dir("hf", repo, ModelCategory::Llm, &artifact_id);
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = dir.join(gguf_name);
        std::fs::write(&runtime, b"GGUFxxxxx").unwrap();
        let install_id = derive_install_id(
            &ArtifactSource::HuggingFace,
            repo,
            &artifact_id,
            Some(variant),
        );
        let meta = InstallMeta {
            schema_version: META_SCHEMA_VERSION,
            install_id: install_id.clone(),
            source: "hf".into(),
            model_id: repo.into(),
            repo_id: Some(repo.into()),
            revision: Some("main".into()),
            model_type: "llm".into(),
            artifact_id: artifact_id.clone(),
            variant: Some(variant.into()),
            architecture: Some("llama-cpp-gguf".into()),
            installed_at: "2026-08-17T00:00:00Z".into(),
            registry_id: None,
            version: None,
            managed: Some(true),
        };
        ModelStorage::write_meta(&dir, &meta).unwrap();
        (dir, runtime, install_id)
    }

    /// 同 repo 多 artifact 并存（ASR HF 安装）：list_models 与 summary 正确表达，
    /// current 从 selection 派生（is_current 不持久化）。
    #[test]
    fn test_hf_multi_variant_install_and_current_derived() {
        run_with_temp_home(|_| {
            use crate::model_library::catalog::ModelCategory;
            use crate::model_library::install::ArtifactSource;
            use crate::model_library::install::{
                InstallMeta, META_SCHEMA_VERSION, ModelStorage, derive_install_id,
            };

            // 造一个 sherpa HF 安装（meta v2 + 占位文件），返回 (install_dir, install_id)
            let make_hf_asr_install = |repo: &str, artifact: &str| -> (PathBuf, String) {
                let dir = ModelStorage::install_dir("hf", repo, ModelCategory::Asr, artifact);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("model.onnx"), b"onnx").unwrap();
                let install_id =
                    derive_install_id(&ArtifactSource::HuggingFace, repo, artifact, None);
                let meta = InstallMeta {
                    schema_version: META_SCHEMA_VERSION,
                    install_id: install_id.clone(),
                    source: "hf".into(),
                    model_id: repo.into(),
                    repo_id: Some(repo.into()),
                    revision: Some("main".into()),
                    model_type: "asr".into(),
                    artifact_id: artifact.into(),
                    variant: None,
                    architecture: Some("sherpa-asr-streaming-zipformer".into()),
                    installed_at: "2026-08-17T00:00:00Z".into(),
                    registry_id: None,
                    version: None,
                    managed: Some(true),
                };
                ModelStorage::write_meta(&dir, &meta).unwrap();
                (dir, install_id)
            };

            let repo = "k2-fsa/sherpa-onnx-test";
            let (a_dir, a_install) = make_hf_asr_install(repo, "artifact-a");
            let (_b_dir, b_install) = make_hf_asr_install(repo, "artifact-b");

            let models = list_models();
            let hf: Vec<_> = models
                .iter()
                .filter(|m| m.source == ModelSource::Hf)
                .collect();
            assert_eq!(hf.len(), 2, "两个 artifact 各自一条 HF 安装");
            // install_id 稳定且唯一
            let ids: Vec<_> = hf
                .iter()
                .map(|m| m.install_id.as_deref().unwrap())
                .collect();
            assert!(ids.contains(&a_install.as_str()));
            assert!(ids.contains(&b_install.as_str()));
            assert_ne!(a_install, b_install);

            // 设为 A current → 仅 A is_current（is_current 不持久化，从 settings 派生）
            set_selected_model(ModelType::Asr, &a_dir).unwrap();
            let models2 = list_models();
            let a = models2
                .iter()
                .find(|m| m.install_id.as_deref() == Some(&a_install))
                .unwrap();
            let b = models2
                .iter()
                .find(|m| m.install_id.as_deref() == Some(&b_install))
                .unwrap();
            assert!(a.current);
            assert!(!b.current);
        });
    }

    /// 删除具体 variant 不误删同 repo 其他 variant。
    #[test]
    fn test_delete_hf_install_only_removes_one_variant() {
        run_with_temp_home(|_| {
            let repo = "Qwen/Qwen3-4B-GGUF";
            let (q4_dir, _, q4_install) = make_hf_install(repo, "Q4_K_M", "Qwen3-4B-Q4_K_M.gguf");
            let (q5_dir, _, q5_install) = make_hf_install(repo, "Q5_K_M", "Qwen3-4B-Q5_K_M.gguf");

            // 按 installId 定位 Q5 目录并删除
            let models = list_models();
            let q5 = models
                .iter()
                .find(|m| m.install_id.as_deref() == Some(&q5_install))
                .unwrap();
            let p = Path::new(q5.local_path.as_ref().unwrap());
            let dir = p.parent().unwrap();
            delete_hf_install_dir(dir).unwrap();

            assert!(!q5_dir.exists(), "Q5 应被删除");
            assert!(q4_dir.exists(), "Q4 不得误删");
            // 删除后再列出只剩 Q4
            let models2 = list_models();
            let hf_ids: Vec<_> = models2
                .iter()
                .filter(|m| m.source == ModelSource::Hf)
                .map(|m| m.install_id.clone())
                .collect();
            assert_eq!(hf_ids, vec![Some(q4_install.clone())]);
        });
    }

    /// settings 不保存 installed inventory（HF 安装只来自 scan）。
    #[test]
    fn test_settings_has_no_installed_inventory() {
        run_with_temp_home(|_| {
            let repo = "Qwen/Qwen3-4B-GGUF";
            make_hf_install(repo, "Q4_K_M", "Qwen3-4B-Q4_K_M.gguf");
            let cfg = crate::config::settings::load_settings()
                .unwrap()
                .unwrap_or_default();
            let locals = cfg.model_library.map(|m| m.local_models.len()).unwrap_or(0);
            assert_eq!(locals, 0, "Settings 不保存 HF 安装 inventory");
            // scan 是唯一来源
            assert_eq!(
                crate::model_library::install::ModelStorage::scan_installs().len(),
                1
            );
        });
    }
}
