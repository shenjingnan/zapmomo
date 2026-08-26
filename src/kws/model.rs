//! 模型资产清单解析与下载安装。
//!
//! 模型元数据编译期嵌入（`include_str!`），运行时从用户目录
//! `~/.zapmomo/models/<name>` 安装/查找，供 CLI（`kws install-model`）与
//! GUI（下载按钮）复用。流程与 `scripts/download-kws-model.sh` 一致：
//! 下载 → sha256 校验 → 临时目录解压 → 原子落位，幂等可重跑。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::config::settings::get_models_dir;
use crate::kws::config::{
    DEFAULT_DECODER, DEFAULT_ENCODER, DEFAULT_JOINER, DEFAULT_KEYWORDS_REL, DEFAULT_TOKENS,
};

/// `models/manifest.json` 的顶层结构。
#[derive(Debug, Clone, Deserialize)]
pub struct ModelManifest {
    #[serde(rename = "schema_version")]
    pub schema_version: u32,
    pub assets: Vec<ModelAsset>,
}

/// 单个模型资产。
#[derive(Debug, Clone, Deserialize)]
pub struct ModelAsset {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub version: String,
    /// 资产类型：`archive`（默认，tar.bz2 解压落位）或 `raw`（单文件直接落位）。
    #[serde(default)]
    pub kind: Option<String>,
    pub archive: String,
    pub source: String,
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub license: String,
}

impl ModelAsset {
    /// 是否为「裸文件」资产（单文件下载，无需解压）。
    pub fn is_raw(&self) -> bool {
        self.kind.as_deref() == Some("raw")
    }
}

/// 编译期嵌入的清单 JSON（随仓库入库，打包后不依赖外部文件）。
const MANIFEST_JSON: &str = include_str!("../../models/manifest.json");

/// 解析一次并缓存。
fn manifest() -> &'static ModelManifest {
    static CACHE: OnceLock<ModelManifest> = OnceLock::new();
    CACHE.get_or_init(|| serde_json::from_str(MANIFEST_JSON).expect("内嵌模型清单无效"))
}

/// 按 role 查找模型资产（如 "wake-word" / "asr"）。
pub fn asset_by_role(role: &str) -> Option<&'static ModelAsset> {
    manifest().assets.iter().find(|a| a.role == role)
}

/// 默认唤醒词模型资产（清单中第一个 `role == "wake-word"` 的资产，找不到则取首个）。
pub fn default_asset() -> &'static ModelAsset {
    asset_by_role("wake-word")
        .or_else(|| manifest().assets.first())
        .expect("模型清单为空")
}

/// ASR 模型资产（清单中 `role == "asr"` 的资产）。
pub fn asr_asset() -> &'static ModelAsset {
    asset_by_role("asr").expect("模型清单缺少 asr 资产")
}

/// 标点恢复模型资产（清单中 `role == "punctuation"` 的资产）。
pub fn punctuation_asset() -> &'static ModelAsset {
    asset_by_role("punctuation").expect("模型清单缺少 punctuation 资产")
}

/// TTS 主模型资产（清单中 `role == "tts"` 的资产，tar.bz2 归档）。
pub fn tts_asset() -> &'static ModelAsset {
    asset_by_role("tts").expect("模型清单缺少 tts 资产")
}

/// TTS 声码器资产（清单中 `role == "tts-vocoder"` 的资产，裸 .onnx 单文件）。
pub fn tts_vocoder_asset() -> &'static ModelAsset {
    asset_by_role("tts-vocoder").expect("模型清单缺少 tts-vocoder 资产")
}

/// 用户模型根目录：`~/.zapmomo/models`
pub fn user_models_dir() -> PathBuf {
    get_models_dir()
}

/// 默认 KWS 模型安装目录：`~/.zapmomo/models/<name>`
pub fn user_model_dir() -> PathBuf {
    get_models_dir().join(&default_asset().name)
}

/// 默认 ASR 模型安装目录：`~/.zapmomo/models/<name>`
pub fn asr_user_model_dir() -> PathBuf {
    get_models_dir().join(&asr_asset().name)
}

/// 默认标点模型安装目录：`~/.zapmomo/models/<name>`
pub fn punctuation_user_model_dir() -> PathBuf {
    get_models_dir().join(&punctuation_asset().name)
}

/// 默认 TTS 模型安装目录：`~/.zapmomo/models/<name>`
pub fn tts_user_model_dir() -> PathBuf {
    get_models_dir().join(&tts_asset().name)
}

/// Silero VAD 资产（清单中 `role == "asr-vad"` 的资产，裸 .onnx 单文件，听写分段用）。
pub fn asr_vad_asset() -> &'static ModelAsset {
    asset_by_role("asr-vad").expect("模型清单缺少 asr-vad 资产")
}

/// Silero VAD 模型文件路径：`~/.zapmomo/models/silero-vad/silero_vad.onnx`。
pub fn asr_vad_user_model_path() -> PathBuf {
    get_models_dir()
        .join(&asr_vad_asset().name)
        .join(&asr_vad_asset().archive)
}

/// 下载/安装阶段（CLI 打日志 / GUI 推事件共用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStage {
    Downloading,
    Verifying,
    Extracting,
    Done,
}

/// 下载进度回调载荷。
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub stage: DownloadStage,
    /// 下载阶段 0..=100；其它阶段为 `-1`（不确定进度）。
    pub percent: f64,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub message: String,
}

pub type ProgressFn<'a> = dyn FnMut(DownloadProgress) + Send + 'a;

/// 模型安装错误。
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("HTTP 请求失败: {0}")]
    Http(String),
    #[error("下载失败（重试后仍失败）: {0}")]
    Download(String),
    #[error("sha256 校验失败（期望 {expected}，实际 {actual}）")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("解压失败: {0}")]
    Extract(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("下载已取消")]
    Cancelled,
}

/// KWS 模型安装完成所需的文件（相对目标目录）。
pub const KWS_REQUIRED_FILES: [&str; 5] = [
    DEFAULT_ENCODER,
    DEFAULT_DECODER,
    DEFAULT_JOINER,
    DEFAULT_TOKENS,
    DEFAULT_KEYWORDS_REL,
];

/// wenetspeech 模型包内文件名（epoch-12 系列，官方文档测试命令同款）。
///
/// 包内另有 epoch-99 系列与 int8 变体，此处固定取官方推荐的 epoch-12 fp32 三件套。
pub const WENETSPEECH_ENCODER: &str = "encoder-epoch-12-avg-2-chunk-16-left-64.onnx";
pub const WENETSPEECH_DECODER: &str = "decoder-epoch-12-avg-2-chunk-16-left-64.onnx";
pub const WENETSPEECH_JOINER: &str = "joiner-epoch-12-avg-2-chunk-16-left-64.onnx";
/// wenetspeech 自带关键词文件（与 zh-en 的 `test_wavs/keywords.txt` 不同名）。
pub const WENETSPEECH_KEYWORDS_REL: &str = "test_wavs/test_keywords.txt";
/// wenetspeech 模型安装完成所需的文件（tokens.txt 与 zh-en 同名共用）。
pub const KWS_WENETSPEECH_REQUIRED_FILES: [&str; 5] = [
    WENETSPEECH_ENCODER,
    WENETSPEECH_DECODER,
    WENETSPEECH_JOINER,
    DEFAULT_TOKENS,
    WENETSPEECH_KEYWORDS_REL,
];

/// gigaspeech 模型安装完成所需的文件。
///
/// 三件套与 wenetspeech 同名（官方包实况，已核实），默认关键词同样取
/// `test_wavs/test_keywords.txt`（包根旧格式 `keywords.txt` 含不在 tokens.txt 的
/// piece，为旧脚本残留，不采用）。`bpe.model` 是自定义英文唤醒词的 BPE 编码依据，
/// 纳入安装完整性校验。
pub const KWS_GIGASPEECH_REQUIRED_FILES: [&str; 6] = [
    WENETSPEECH_ENCODER,
    WENETSPEECH_DECODER,
    WENETSPEECH_JOINER,
    DEFAULT_TOKENS,
    WENETSPEECH_KEYWORDS_REL,
    "bpe.model",
];

/// 目标目录是否已包含给定的一组文件。
pub fn has_required_files(dest_dir: &Path, required: &[&str]) -> bool {
    required.iter().all(|f| dest_dir.join(f).is_file())
}

/// 目标目录是否已装好 KWS 模型（5 个核心文件齐全）。
pub fn is_installed(dest_dir: &Path) -> bool {
    has_required_files(dest_dir, &KWS_REQUIRED_FILES)
}

/// 安装默认唤醒词模型到 `dest_dir`（默认 `~/.zapmomo/models/<name>`）。
///
/// 幂等：已安装且 `force` 为假时直接返回。下载过程中回调进度。
pub fn install_model_to(
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
) -> Result<(), ModelError> {
    install_asset_to(
        default_asset(),
        dest_dir,
        force,
        on_progress,
        &KWS_REQUIRED_FILES,
    )
}

/// 安装标点模型到 `dest_dir`（默认 `~/.zapmomo/models/<标点模型名>`）。
///
/// 幂等：已安装且 `force` 为假时直接返回。`required_files` 用于幂等性判断。
pub fn install_punctuation_model_to(
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
    required_files: &[&str],
) -> Result<(), ModelError> {
    install_asset_to(
        punctuation_asset(),
        dest_dir,
        force,
        on_progress,
        required_files,
    )
}

/// 按指定资产安装（测试/多模型可复用）。`required_files` 用于幂等性判断。
///
/// 等价于 `install_asset_to_cancellable(..., None)`（不可取消）。
pub fn install_asset_to(
    asset: &ModelAsset,
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
    required_files: &[&str],
) -> Result<(), ModelError> {
    install_asset_to_cancellable(asset, dest_dir, force, on_progress, required_files, None)
}

/// 可取消版本的 [`install_asset_to`]。
///
/// `cancel` 为 `Some(&AtomicBool)` 时，下载读循环每轮检查；命中即清理临时文件并返回
/// [`ModelError::Cancelled`]。各阶段前也会再检查一次（下载/校验/解压/落位）。
pub fn install_asset_to_cancellable(
    asset: &ModelAsset,
    dest_dir: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
    required_files: &[&str],
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ModelError> {
    if cancelled(cancel) {
        return Err(ModelError::Cancelled);
    }
    let parent = dest_dir
        .parent()
        .ok_or_else(|| ModelError::Extract("目标目录缺少父目录".to_string()))?;

    if !force && has_required_files(dest_dir, required_files) {
        on_progress(progress(DownloadStage::Done, 100.0, dest_dir, "模型已安装"));
        return Ok(());
    }

    std::fs::create_dir_all(parent)?;
    let tmp_archive = parent.join(format!(".{}.tmp", asset.archive));

    download_to(
        &asset.source,
        &tmp_archive,
        asset.size_bytes,
        on_progress,
        cancel,
    )?;
    if cancelled(cancel) {
        let _ = std::fs::remove_file(&tmp_archive);
        return Err(ModelError::Cancelled);
    }

    on_progress(progress(
        DownloadStage::Verifying,
        -1.0,
        dest_dir,
        "校验 sha256",
    ));
    verify_sha256(&tmp_archive, &asset.sha256)?;

    on_progress(progress(
        DownloadStage::Extracting,
        -1.0,
        dest_dir,
        "解压中",
    ));
    extract_and_place(&tmp_archive, dest_dir)?;

    on_progress(progress(
        DownloadStage::Done,
        100.0,
        dest_dir,
        "模型安装完成",
    ));
    Ok(())
}

/// 取消标志是否已置位。
fn cancelled(cancel: Option<&std::sync::atomic::AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
}

/// 安装「裸文件」资产（单文件，无解压）到 `dest_path`。
///
/// 用于 TTS 声码器这类与主包分离发布的独立 .onnx 文件。流程：
/// 幂等检查 → 下载到临时文件 → sha256 校验 → 原子落位（无解压阶段）。
pub fn install_raw_file_to(
    asset: &ModelAsset,
    dest_path: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
) -> Result<(), ModelError> {
    install_raw_file_to_cancellable(asset, dest_path, force, on_progress, None)
}

/// 可取消版本的 [`install_raw_file_to`]。
pub fn install_raw_file_to_cancellable(
    asset: &ModelAsset,
    dest_path: &Path,
    force: bool,
    on_progress: &mut ProgressFn,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ModelError> {
    if cancelled(cancel) {
        return Err(ModelError::Cancelled);
    }
    let parent = dest_path
        .parent()
        .ok_or_else(|| ModelError::Extract("目标文件缺少父目录".to_string()))?;

    if !force && dest_path.is_file() {
        on_progress(progress(
            DownloadStage::Done,
            100.0,
            dest_path,
            "模型已安装",
        ));
        return Ok(());
    }

    std::fs::create_dir_all(parent)?;
    // tmp 名只取 archive 的文件名部分：archive 可能含子目录相对路径
    // （如 `embeddings/alba.safetensors`），直接拼接会落到未创建的子目录里
    let file_stem = std::path::Path::new(&asset.archive)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.archive.clone());
    let tmp = parent.join(format!(".{file_stem}.tmp"));

    download_to(&asset.source, &tmp, asset.size_bytes, on_progress, cancel)?;
    if cancelled(cancel) {
        let _ = std::fs::remove_file(&tmp);
        return Err(ModelError::Cancelled);
    }

    on_progress(progress(
        DownloadStage::Verifying,
        -1.0,
        dest_path,
        "校验 sha256",
    ));
    verify_sha256(&tmp, &asset.sha256)?;

    // 原子落位：目标已存在先移除（Windows 上 rename 覆盖文件可能失败）。
    if dest_path.exists() {
        std::fs::remove_file(dest_path)?;
    }
    std::fs::rename(&tmp, dest_path)?;

    on_progress(progress(
        DownloadStage::Done,
        100.0,
        dest_path,
        "模型安装完成",
    ));
    Ok(())
}

fn progress(
    stage: DownloadStage,
    percent: f64,
    _dest_dir: &Path,
    message: &str,
) -> DownloadProgress {
    DownloadProgress {
        stage,
        percent,
        bytes_downloaded: 0,
        total_bytes: 0,
        message: message.to_string(),
    }
}

/// 流式下载到临时文件，带进度回调；失败重试 3 次（退避等待）。
///
/// `cancel` 命中时立即返回 [`ModelError::Cancelled`]（不重试），并删除临时文件。
/// `pub(crate)`：模型库下载（download.rs）复用同一流式核心。
pub(crate) fn download_to(
    url: &str,
    tmp_archive: &Path,
    manifest_total: u64,
    on_progress: &mut ProgressFn,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ModelError> {
    let mut last_err: Option<ModelError> = None;
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (1 << attempt)));
        }
        match try_download_once(url, tmp_archive, manifest_total, on_progress, cancel) {
            Ok(()) => return Ok(()),
            Err(e) => {
                if matches!(e, ModelError::Cancelled) {
                    let _ = std::fs::remove_file(tmp_archive);
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.map_or_else(
        || ModelError::Download("未知错误".to_string()),
        |e| ModelError::Download(e.to_string()),
    ))
}

fn try_download_once(
    url: &str,
    tmp_archive: &Path,
    manifest_total: u64,
    on_progress: &mut ProgressFn,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ModelError> {
    try_download_once_core(url, None, tmp_archive, manifest_total, on_progress, cancel)
}

fn try_download_once_core(
    url: &str,
    token: Option<&str>,
    tmp_archive: &Path,
    manifest_total: u64,
    on_progress: &mut ProgressFn,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ModelError> {
    let mut req = ureq::get(url);
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().map_err(|e| ModelError::Http(e.to_string()))?;
    let total = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(manifest_total);

    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(tmp_archive)?;
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        if cancelled(cancel) {
            let _ = std::fs::remove_file(tmp_archive);
            return Err(ModelError::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        let percent = if total > 0 {
            ((done as f64 / total as f64) * 100.0).min(100.0)
        } else {
            -1.0
        };
        on_progress(DownloadProgress {
            stage: DownloadStage::Downloading,
            percent,
            bytes_downloaded: done,
            total_bytes: total,
            message: format!("下载中 {:.1}%", percent.max(0.0)),
        });
    }
    file.flush()?;
    Ok(())
}

/// 对临时压缩包整包计算 sha256 并比对；不匹配则删除损坏文件并报错。
/// `pub(crate)`：HF 下载文件完整性校验复用。
pub(crate) fn verify_sha256(path: &Path, expected: &str) -> Result<(), ModelError> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex::encode(hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(path);
        return Err(ModelError::Sha256Mismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// 解压 tar.bz2 到同父目录临时目录，再把顶层模型目录原子移到目标位置。
fn extract_and_place(tmp_archive: &Path, dest_dir: &Path) -> Result<(), ModelError> {
    let parent = dest_dir
        .parent()
        .ok_or_else(|| ModelError::Extract("目标目录缺少父目录".to_string()))?;
    let name = dest_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let tmp_extract = parent.join(format!(".{name}.extract"));
    std::fs::create_dir_all(&tmp_extract)?;

    let file = std::fs::File::open(tmp_archive)?;
    let bz = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(bz);
    archive
        .unpack(&tmp_extract)
        .map_err(|e| ModelError::Extract(e.to_string()))?;

    // 定位顶层模型目录：优先 <name>，否则退化为唯一的顶层项（兼容不同包内布局）。
    let src = tmp_extract.join(&name);
    let src = if src.is_dir() {
        src
    } else {
        let mut entries = std::fs::read_dir(&tmp_extract)?.filter_map(Result::ok);
        let top = entries
            .next()
            .map(|e| e.path())
            .ok_or_else(|| ModelError::Extract("压缩包内容为空".to_string()))?;
        if entries.next().is_some() {
            return Err(ModelError::Extract(
                "压缩包顶层存在多个目录，无法确定模型根目录".to_string(),
            ));
        }
        top
    };

    // 原子落位：目标已存在先移除（Windows 上 rename 覆盖目录会失败）。
    if dest_dir.exists() {
        std::fs::remove_dir_all(dest_dir)?;
    }
    std::fs::rename(&src, dest_dir)?;
    std::fs::remove_dir_all(&tmp_extract)?;
    let _ = std::fs::remove_file(tmp_archive);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::net::TcpListener;

    pub(crate) fn mini_tarbz2(prefix: &str) -> Vec<u8> {
        use bzip2::Compression;
        use bzip2::write::BzEncoder;
        let mut bz = BzEncoder::new(Vec::new(), Compression::default());
        {
            let mut ar = tar::Builder::new(&mut bz);
            let base = format!("{prefix}/");
            let mut dir = tar::Header::new_gnu();
            dir.set_entry_type(tar::EntryType::Directory);
            dir.set_size(0);
            dir.set_mode(0o755);
            dir.set_username("test").unwrap();
            dir.set_groupname("test").unwrap();
            dir.set_cksum();
            ar.append_data(&mut dir, &base, std::io::empty()).unwrap();

            let mut f = |rel: &str, bytes: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(bytes.len() as u64);
                h.set_mode(0o644);
                h.set_username("test").unwrap();
                h.set_groupname("test").unwrap();
                h.set_cksum();
                ar.append_data(&mut h, format!("{base}{rel}"), bytes)
                    .unwrap();
            };
            f(DEFAULT_ENCODER, b"enc-onnx-bytes");
            f(DEFAULT_DECODER, b"dec-onnx-bytes");
            f(DEFAULT_JOINER, b"joiner-onnx-bytes");
            f(DEFAULT_TOKENS, b"token symbols");
            f(DEFAULT_KEYWORDS_REL, b"k w @KW\n");
            ar.finish().unwrap();
        }
        bz.finish().unwrap()
    }

    /// 起一个本地 HTTP 服务，每个连接都返回给定字节，返回请求 URL。
    pub(crate) fn serve_many(bytes: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = std::sync::Arc::new(bytes);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { break };
                let payload = payload.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = sock.write_all(head.as_bytes());
                    let _ = sock.write_all(&payload);
                });
            }
        });
        format!("http://{addr}/model.tar.bz2")
    }

    fn asset_for(source: &str, sha256: &str, archive: &str) -> ModelAsset {
        ModelAsset {
            name: "test-kws-model".to_string(),
            role: "wake-word".to_string(),
            version: "test".to_string(),
            kind: None,
            archive: archive.to_string(),
            source: source.to_string(),
            sha256: sha256.to_string(),
            size_bytes: 0,
            license: "Apache-2.0".to_string(),
        }
    }

    pub(crate) fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(data))
    }

    #[test]
    fn test_manifest_default_asset() {
        let a = default_asset();
        assert!(!a.name.is_empty());
        assert!(a.source.starts_with("http"));
        assert_eq!(a.sha256.len(), 64);
        // 默认资产应为清单中 role == "wake-word" 的条目（自洽校验，不依赖模型文件是否已下载）
        let m = manifest();
        assert!(
            m.assets
                .iter()
                .any(|x| x.name == a.name && x.role == "wake-word"),
            "default_asset 不在清单中"
        );
    }

    #[test]
    fn test_manifest_asr_asset() {
        let a = asr_asset();
        assert!(!a.name.is_empty());
        assert!(a.source.starts_with("http"));
        assert_eq!(a.sha256.len(), 64);
        // asr_asset 应为清单中 role == "asr" 的条目（自洽校验，不依赖模型文件是否已下载）
        let m = manifest();
        assert!(
            m.assets.iter().any(|x| x.name == a.name && x.role == "asr"),
            "asr_asset 不在清单中"
        );
        // asset_by_role 应与 asr_asset 一致
        assert_eq!(asset_by_role("asr").unwrap().name, a.name);
    }

    #[test]
    fn test_manifest_punctuation_asset() {
        let a = punctuation_asset();
        assert!(!a.name.is_empty());
        assert!(a.source.starts_with("http"));
        assert_eq!(a.sha256.len(), 64);
        // punctuation_asset 应为清单中 role == "punctuation" 的条目（自洽校验）
        let m = manifest();
        assert!(
            m.assets
                .iter()
                .any(|x| x.name == a.name && x.role == "punctuation"),
            "punctuation_asset 不在清单中"
        );
        assert_eq!(asset_by_role("punctuation").unwrap().name, a.name);
    }

    #[test]
    fn test_manifest_tts_assets() {
        let a = tts_asset();
        assert_eq!(a.role, "tts");
        assert!(!a.is_raw());
        assert_eq!(a.sha256.len(), 64);

        let v = tts_vocoder_asset();
        assert_eq!(v.role, "tts-vocoder");
        assert!(v.is_raw());
        assert_eq!(v.sha256.len(), 64);

        // 声码器与主包落位到同一模型目录
        assert_eq!(tts_asset().name, tts_vocoder_asset().name);
    }

    #[test]
    fn test_manifest_asr_vad_asset() {
        let a = asr_vad_asset();
        assert_eq!(a.role, "asr-vad");
        assert!(a.is_raw(), "VAD 是 raw 单文件");
        assert_eq!(a.archive, "silero_vad.onnx");
        assert!(a.source.starts_with("http"));
        assert_eq!(a.sha256.len(), 64);
        // 自洽：清单中 role == "asr-vad"
        let m = manifest();
        assert!(
            m.assets
                .iter()
                .any(|x| x.role == "asr-vad" && x.name == a.name),
            "asr-vad 不在清单中"
        );
        assert_eq!(asset_by_role("asr-vad").unwrap().name, a.name);
        // 落位目录：~/.zapmomo/models/<name>/silero_vad.onnx
        assert_eq!(
            asr_vad_user_model_path(),
            get_models_dir().join(&a.name).join("silero_vad.onnx")
        );
    }

    #[test]
    fn test_verify_sha256_ok_and_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob");
        std::fs::write(&p, b"hello").unwrap();
        assert!(verify_sha256(&p, &sha256_hex(b"hello")).is_ok());
        // 错误校验值：报错且删除损坏文件
        let p2 = dir.path().join("bad");
        std::fs::write(&p2, b"hello").unwrap();
        let err = verify_sha256(&p2, &"0".repeat(64)).unwrap_err();
        assert!(matches!(err, ModelError::Sha256Mismatch { .. }));
        assert!(!p2.exists());
    }

    #[test]
    fn test_extract_and_place_mini_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("mini.tar.bz2");
        std::fs::write(&archive, mini_tarbz2("test-kws-model")).unwrap();
        let dest = dir.path().join("test-kws-model");
        extract_and_place(&archive, &dest).unwrap();
        assert!(is_installed(&dest));
        assert!(!archive.exists());
        assert!(!dir.path().join(".test-kws-model.extract").exists());
    }

    #[test]
    fn test_install_full_flow_via_local_server() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = mini_tarbz2("test-kws-model");
        let url = serve_many(bytes.clone());
        let archive = "mini.tar.bz2".to_string();
        let asset = asset_for(&url, &sha256_hex(&bytes), &archive);

        let dest = dir.path().join("test-kws-model");
        let mut stages = Vec::new();
        install_asset_to(
            &asset,
            &dest,
            false,
            &mut |p| stages.push(p.stage),
            &KWS_REQUIRED_FILES,
        )
        .unwrap();
        assert!(is_installed(&dest));

        let expected = [
            DownloadStage::Downloading,
            DownloadStage::Verifying,
            DownloadStage::Extracting,
            DownloadStage::Done,
        ];
        assert_eq!(stages, expected);
    }

    #[test]
    fn test_install_idempotent_skips_when_installed() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test-kws-model");
        // 直接摆好核心文件，模拟已安装
        std::fs::create_dir_all(dest.join("test_wavs")).unwrap();
        std::fs::write(dest.join(DEFAULT_ENCODER), b"e").unwrap();
        std::fs::write(dest.join(DEFAULT_DECODER), b"d").unwrap();
        std::fs::write(dest.join(DEFAULT_JOINER), b"j").unwrap();
        std::fs::write(dest.join(DEFAULT_TOKENS), b"t").unwrap();
        std::fs::write(dest.join(DEFAULT_KEYWORDS_REL), b"k").unwrap();

        let mut stages = Vec::new();
        install_asset_to(
            &default_asset(),
            &dest,
            false,
            &mut |p| stages.push(p.stage),
            &KWS_REQUIRED_FILES,
        )
        .unwrap();
        assert_eq!(stages, vec![DownloadStage::Done]);
    }

    #[test]
    fn test_install_raw_file_via_local_server() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"vocos-onnx-bytes".to_vec();
        let url = serve_many(bytes.clone());
        let mut asset = asset_for(&url, &sha256_hex(&bytes), "vocos_24khz.onnx");
        asset.kind = Some("raw".to_string());

        let dest = dir.path().join("vocos_24khz.onnx");
        let mut stages = Vec::new();
        install_raw_file_to(&asset, &dest, false, &mut |p| stages.push(p.stage)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), bytes);

        let expected = [
            DownloadStage::Downloading,
            DownloadStage::Verifying,
            DownloadStage::Done,
        ];
        assert_eq!(stages, expected);

        // 幂等：已装且非 force → 仅 Done
        let mut stages = Vec::new();
        install_raw_file_to(&asset, &dest, false, &mut |p| stages.push(p.stage)).unwrap();
        assert_eq!(stages, vec![DownloadStage::Done]);
    }

    #[test]
    fn test_install_force_reinstalls() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = mini_tarbz2("test-kws-model");
        let url = serve_many(bytes.clone());
        let asset = asset_for(&url, &sha256_hex(&bytes), "mini.tar.bz2");
        let dest = dir.path().join("test-kws-model");

        // 先装好，再 force 重装 → 应重新走完整流程
        install_asset_to(&asset, &dest, false, &mut |_| {}, &KWS_REQUIRED_FILES).unwrap();
        let mut stages = Vec::new();
        install_asset_to(
            &asset,
            &dest,
            true,
            &mut |p| stages.push(p.stage),
            &KWS_REQUIRED_FILES,
        )
        .unwrap();
        assert!(is_installed(&dest));
        assert!(stages.contains(&DownloadStage::Downloading));
    }

    #[test]
    fn test_install_sha256_mismatch_errors() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = mini_tarbz2("test-kws-model");
        let url = serve_many(bytes);
        let asset = asset_for(&url, &"0".repeat(64), "mini.tar.bz2");
        let dest = dir.path().join("test-kws-model");
        let err =
            install_asset_to(&asset, &dest, false, &mut |_| {}, &KWS_REQUIRED_FILES).unwrap_err();
        assert!(matches!(err, ModelError::Sha256Mismatch { .. }));
        assert!(!dest.exists());
    }

    /// 慢速分块下载服务器，便于在下载中途触发取消。
    fn serve_slow(step_ms: u64, payload: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = std::sync::Arc::new(payload);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut sock) = stream else { break };
                let payload = payload.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = sock.write_all(head.as_bytes());
                    let mut sent = 0usize;
                    while sent < payload.len() {
                        let end = (sent + 8192).min(payload.len());
                        let _ = sock.write_all(&payload[sent..end]);
                        sent = end;
                        std::thread::sleep(std::time::Duration::from_millis(step_ms));
                    }
                });
            }
        });
        format!("http://{addr}/slow.tar.bz2")
    }

    #[test]
    fn test_install_cancel_cleans_tmp_and_can_redo() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let payload = vec![0u8; 512 * 1024];
        let url = serve_slow(15, payload);
        let asset = asset_for(&url, &"0".repeat(64), "slow.tar.bz2");
        let dest = dir.path().join("test-kws-model");
        let parent = dest.parent().unwrap();
        let tmp_path = parent.join(".slow.tar.bz2.tmp");

        // 收到第一个中间进度后立即取消（确定性，避免时序竞态）
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();
        let asset_for_thread = asset.clone();
        let dest_for_thread = dest.clone();
        let handle = std::thread::spawn(move || {
            let mut stages = Vec::new();
            let result = install_asset_to_cancellable(
                &asset_for_thread,
                &dest_for_thread,
                false,
                &mut |p| {
                    if p.percent > 0.0 && p.percent < 100.0 {
                        let _ = tx.send(p.percent);
                    }
                    stages.push(p.percent);
                },
                &KWS_REQUIRED_FILES,
                Some(&cancel_clone),
            );
            (result, stages)
        });

        // 等到确实观察到中间进度后才取消
        let _first = rx
            .recv_timeout(std::time::Duration::from_secs(15))
            .expect("慢速服务器应产生中间进度");
        cancel.store(true, Ordering::Relaxed);
        let (result, stages) = handle.join().unwrap();

        assert!(matches!(result, Err(ModelError::Cancelled)));
        // 确实观察到了中间进度
        assert!(stages.iter().any(|&p| p > 0.0 && p < 100.0));
        // 取消后临时文件与正式目录都被清理
        assert!(!tmp_path.exists());
        assert!(!dest.exists());

        // 取消后重新下载（cancel 复位）能正常开始并完成
        cancel.store(false, Ordering::Relaxed);
        let bytes = mini_tarbz2("test-kws-model");
        let url2 = serve_many(bytes.clone());
        let asset2 = asset_for(&url2, &sha256_hex(&bytes), "mini.tar.bz2");
        let mut stages2 = Vec::new();
        install_asset_to_cancellable(
            &asset2,
            &dest,
            false,
            &mut |p| stages2.push(p.stage),
            &KWS_REQUIRED_FILES,
            Some(&cancel),
        )
        .unwrap();
        assert!(is_installed(&dest));
    }

    /// 多资产总体进度聚合：各 asset 下载字节累计后总体百分比单调不减。
    #[test]
    fn test_aggregate_overall_percent_monotonic() {
        // 与 install 聚合公式一致：overall = (累计已完成 + 当前 asset 字节) / 总字节
        let total: u64 = 3000;
        let mut done: u64 = 0;
        let mut prev: f64 = -1.0;
        let mut monotonic = true;
        let mut last_overall: f64 = 0.0;
        for size in [1000u64, 1000, 1000] {
            for &step in &[0u64, 500, size] {
                let cur = done + step;
                let overall = ((cur as f64 / total as f64) * 100.0).min(100.0);
                if overall < prev {
                    monotonic = false;
                }
                prev = overall;
                last_overall = overall;
            }
            done += size;
        }
        assert!(monotonic, "总体进度不能倒退");
        assert!((last_overall - 100.0).abs() < 1e-9);
    }
}
