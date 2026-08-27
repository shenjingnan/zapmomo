/// Settings - TOML 配置管理
///
/// 提供通用的配置读写功能，支持 ${env.VAR} 环境变量引用。
/// 配置文件存储在 `~/.zapmomo/settings.toml`。
use crate::config::shortcuts::ShortcutsSettings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::RwLock;
use std::time::SystemTime;

const PROJECT_DIR: &str = ".zapmomo";
const SETTINGS_FILE: &str = "settings.toml";

/// 获取用户 home 目录（跨平台：macOS/Linux 用 $HOME，Windows 用 %USERPROFILE%）
pub fn get_home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
        .into()
}

/// 获取配置目录路径
pub fn get_settings_dir() -> PathBuf {
    get_home_dir().join(PROJECT_DIR)
}

/// 获取设置文件路径
pub fn get_settings_path() -> PathBuf {
    get_settings_dir().join(SETTINGS_FILE)
}

/// 获取模型目录路径：`<data_dir>/models`（data_dir 未设置时为 `~/.zapmomo/models`）。
///
/// 模型统一安装到用户目录，不随仓库/安装包分发。
pub fn get_models_dir() -> PathBuf {
    get_data_dir()
        .unwrap_or_else(get_settings_dir)
        .join("models")
}

/// 旧版默认模型根 `~/.zapmomo/models`：`data_dir` 指向别处时返回 `Some`，
/// 供双根扫描/默认目录回退/迁移定位存量安装；未自定义时返回 `None`。
pub fn legacy_models_dir() -> Option<PathBuf> {
    let default = get_settings_dir().join("models");
    (get_models_dir() != default).then_some(default)
}

/// 伙伴模型载荷存储目录：`<data_dir>/companions`（未设置时为 `~/.zapmomo/companions`）。
///
/// 注意：`library.json` 清单永远留在 `~/.zapmomo/companions`（见 `companion::get_companions_dir`），
/// 只有模型载荷目录跟随 `data_dir`。
pub fn get_companions_store_dir() -> PathBuf {
    get_data_dir()
        .unwrap_or_else(get_settings_dir)
        .join("companions")
}

/// 旧版默认伙伴载荷目录 `~/.zapmomo/companions`：`data_dir` 指向别处时返回 `Some`。
pub fn legacy_companions_dir() -> Option<PathBuf> {
    let default = get_settings_dir().join("companions");
    (get_companions_store_dir() != default).then_some(default)
}

/// 剥离路径前缀（Windows 大小写不敏感，容忍 `\`/`/` 混合分隔符）。
///
/// `path` 以 `prefix` 开头（大小写忽略）且 `prefix` 结束在组件边界（恰好相等，
/// 或其后紧跟分隔符）时返回剩余相对路径，否则 `None`——防止 `models2` 被
/// `models` 误剥导致迁移改写出错路径。
/// 供迁移时改写 settings/伙伴库中的绝对路径引用使用。
pub fn strip_prefix_ci<'a>(path: &'a Path, prefix: &Path) -> Option<&'a Path> {
    let p = path.as_os_str().to_str()?;
    let q = prefix.as_os_str().to_str()?;
    if cfg!(windows) {
        // 归一化分隔符 + 大小写（`/`↔`\` 一一对应，长度不变），再比较前缀；
        // 前缀尾部多余分隔符先去掉，边界判断才不受影响
        let pl = p.replace('/', "\\").to_lowercase();
        let ql = q
            .replace('/', "\\")
            .to_lowercase()
            .trim_end_matches('\\')
            .to_string();
        // 前缀后必须是分隔符（归一化后）或路径恰好等于前缀
        let boundary =
            pl.len() == ql.len() || pl.get(ql.len()..).is_some_and(|r| r.starts_with('\\'));
        if ql.is_empty() || !pl.starts_with(&ql) || !boundary {
            return None;
        }
        // ql.len() 是归一化后的前缀长度；原始 p 里前缀部分分隔符未变长，get 切安全
        let rest = p.get(ql.len()..)?;
        Some(Path::new(rest.trim_start_matches(['/', '\\'])))
    } else {
        let q = q.trim_end_matches('/');
        p.strip_prefix(q).and_then(|rest| {
            if rest.is_empty() || rest.starts_with('/') {
                Some(Path::new(rest.trim_start_matches('/')))
            } else {
                None
            }
        })
    }
}

/// `data_dir` 解析缓存：`(settings 路径, mtime, len, 解析结果)`。
///
/// `get_models_dir` 调用高频（系统资源 30s 轮询 / 模型列表 / 每次下载安装），
/// 不能每次读 TOML；以 settings.toml 的 mtime + 文件大小为键，手改文件也会自动失效
/// （mtime 同秒精度不足时，大小变化兜底）。应用内写入经 `save_settings` 主动刷新缓存。
type DataDirCacheValue = Option<(PathBuf, Option<SystemTime>, Option<u64>, Option<PathBuf>)>;
static DATA_DIR_CACHE: LazyLock<RwLock<DataDirCacheValue>> = LazyLock::new(|| RwLock::new(None));

/// 读 settings.toml 的 (mtime, len)（文件不存在/读取失败 → `None`）。
fn settings_mtime_len() -> (Option<SystemTime>, Option<u64>) {
    match std::fs::metadata(get_settings_path()) {
        Ok(m) => (m.modified().ok(), Some(m.len())),
        Err(_) => (None, None),
    }
}

/// 解析 `data_dir` 设置（支持 `${env.VAR}` 引用）。
///
/// 未设置 / 空串 / 相对路径 / env 解析失败 → `None`（回退默认根 `~/.zapmomo`，
/// 调用方拿 `PathBuf` 的签名不能 Err，降级并 `warn`）。
pub fn get_data_dir() -> Option<PathBuf> {
    let path = get_settings_path();
    let (mtime, len) = settings_mtime_len();
    // 快路径：路径 + mtime + 大小都未变，直接用缓存
    if let Some((cached_path, cached_mtime, cached_len, cached_value)) =
        &*DATA_DIR_CACHE.read().unwrap_or_else(|e| e.into_inner())
        && *cached_path == path
        && *cached_mtime == mtime
        && *cached_len == len
    {
        return cached_value.clone();
    }
    // 慢路径：读 settings 解析（失败一律回退默认根）
    let resolved = load_settings()
        .ok()
        .flatten()
        .and_then(|cfg| cfg.data_dir)
        .and_then(|raw| match resolve_env_ref(&raw) {
            Ok(dir) if dir.trim().is_empty() => None,
            Ok(dir) => {
                let p = PathBuf::from(&dir);
                if p.is_absolute() {
                    Some(p)
                } else {
                    tracing::warn!("data_dir 需为绝对路径，当前值 {dir:?}，回退默认目录");
                    None
                }
            }
            Err(e) => {
                tracing::warn!("data_dir 解析失败，回退默认目录：{e}");
                None
            }
        });
    *DATA_DIR_CACHE.write().unwrap_or_else(|e| e.into_inner()) =
        Some((path, mtime, len, resolved.clone()));
    resolved
}

/// 清空 `data_dir` 缓存：写入 data_dir 后调用，确保后续读取立即可见（不等 mtime）。
pub fn refresh_data_dir_cache() {
    *DATA_DIR_CACHE.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 测试专用：重置 data_dir 缓存，避免跨用例污染。
#[cfg(test)]
pub(crate) fn reset_data_dir_cache_for_test() {
    refresh_data_dir_cache();
}

/// 获取 TTS 合成音频输出目录：`~/.zapmomo/tts`（供前端 asset 协议播放）。
pub fn get_tts_output_dir() -> PathBuf {
    get_settings_dir().join("tts")
}

/// 解析 ${env.VAR} 引用
///
/// - "${env.MY_VAR}" → 从环境变量 MY_VAR 读取
/// - "plain-value" → 原样返回
pub fn resolve_env_ref(value: &str) -> Result<String, String> {
    if let Some(captures) = value
        .strip_prefix("${env.")
        .and_then(|s| s.strip_suffix('}'))
    {
        let env_var = captures;
        if env_var.is_empty() {
            return Err("环境变量名称为空".to_string());
        }
        match std::env::var(env_var) {
            Ok(resolved) => Ok(resolved),
            Err(_) => Err(format!(
                "环境变量 {env_var} 未设置。请在 {SETTINGS_FILE} 中配置或设置环境变量 {env_var}。"
            )),
        }
    } else {
        Ok(value.to_string())
    }
}

/// 应用配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    /// 调试模式
    #[serde(default)]
    pub debug: bool,
    /// 日志级别
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 是否在 macOS Dock / Cmd+Tab 中隐藏应用图标（Accessory 模式），缺省 false 展示
    #[serde(default)]
    pub hide_dock_icon: bool,
    /// 自定义配置项（示例）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<std::collections::HashMap<String, String>>,
    /// 全局默认麦克风输入设备名（空 = 系统默认），KWS / ASR 共用；重启后免重选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone: Option<String>,
    /// 自定义数据目录（绝对路径，支持 ${env.VAR}）：模型与伙伴载荷存放在
    /// `<data_dir>/models` 与 `<data_dir>/companions`；settings/日志等小文件
    /// 仍留在 `~/.zapmomo`。缺省 = `~/.zapmomo`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// 唤醒词检测（KWS）配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kws: Option<KwsSettings>,
    /// 语音识别（ASR）配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr: Option<AsrSettings>,
    /// 文本转语音（TTS）配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<TtsSettings>,
    /// Live2D 角色配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live2d: Option<Live2dSettings>,
    /// 本地 LLM 配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmSettings>,
    /// 语音会话（KWS→ASR→LLM→TTS 全链路）配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceSettings>,
    /// 模型库配置（用户通过「添加本地模型」注册的 external 模型等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_library: Option<ModelLibrarySettings>,
    /// 全局快捷键配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<ShortcutsSettings>,
    /// dsh 桥配置（接收 deepseek-harness 插件推送的任务事件）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsh: Option<DshSettings>,
    /// 文字输入条窗口配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatbox: Option<ChatboxSettings>,
}

/// 用户「添加本地模型」注册的模型（external）。
///
/// 只保存注册路径，**不复制/不管理用户文件**；移除时只删除本条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LocalModel {
    /// 稳定 id（`local-` + sha256(规范化绝对路径) 前 12 位）
    pub id: String,
    /// 目录/文件基名（展示用）
    pub name: String,
    /// 能力类型：kws | asr | llm | tts
    pub model_type: String,
    /// 绝对路径（LLM 必须是具体 .gguf 文件路径）
    pub path: String,
    /// 注册时间（RFC3339）
    pub added_at: String,
    /// 显式关联的 Registry 模型 id（从 Registry 卡片导入时携带；顶部添加本地模型为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
}

/// 模型库配置段。
///
/// 只保存**用户配置**（本地注册），不保存 installed inventory。
/// "电脑上装了哪些模型" 的唯一事实来源是 `~/.zapmomo/models/**/.zapmomo-lib.json` 扫描。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelLibrarySettings {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_models: Vec<LocalModel>,
}

/// 唤醒词检测（KWS）配置。
///
/// 全部字段可缺省：未配置的项在解析时回退到 `kws::config` 的内置默认值，
/// 因此这里用 `Option` 以区分「未配置」与「配置了」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct KwsSettings {
    /// 是否启用 KWS（打开开关即持久化；启动时自动监听的前提），缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 会话级自定义唤醒词（原始字符串，多个用 / 分隔；持久化后启动自动监听也使用），缺省 None = 模型内置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_keywords: Option<String>,
    /// 模型目录（支持 ${env.VAR} 引用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
    /// encoder onnx 文件名（缺省用模型目录下 chunk-16 变体）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    /// decoder onnx 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// joiner onnx 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joiner: Option<String>,
    /// tokens.txt 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<String>,
    /// 关键词文件路径（缺省 = <model_dir>/test_wavs/keywords.txt）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords_file: Option<String>,
    /// 推理后端，缺省 "cpu"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 推理线程数，缺省 2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_threads: Option<i32>,
    /// 每次喂给模型的采样数（@16k），缺省 3200（0.2s）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<usize>,
    /// 模型输入采样率，缺省 16000
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    /// 关键词 boosting 分数，缺省 1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords_score: Option<f32>,
    /// 触发阈值，缺省 0.25（越大越不容易误触发）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords_threshold: Option<f32>,
    /// 调试输出，缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

/// 语音识别（ASR）配置。
///
/// 全部字段可缺省：未配置的项在解析时回退到 `asr::config` 的内置默认值，
/// 因此这里用 `Option` 以区分「未配置」与「配置了」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AsrSettings {
    /// 是否启用 ASR（语音会话「能识别」的前提），缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 模型类型（sherpa-onnx 分支：zipformer/paraformer/sensevoice/whisper/qwen3_asr；
    /// 缺省按模型目录内容探测）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_type: Option<crate::asr::config::AsrModelKind>,
    /// 转写语言（SenseVoice/Whisper；缺省自动检测）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// SenseVoice 反向文本正则化（数字/标点，缺省 true）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_itn: Option<bool>,
    /// 模型目录（支持 ${env.VAR} 引用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
    /// encoder onnx 文件名（缺省用 int8 变体）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    /// decoder onnx 文件名（缺省用 fp32 变体，官方 int8 配方）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// joiner onnx 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joiner: Option<String>,
    /// tokens.txt 文件名（Qwen3-ASR 为 tokenizer 目录名，缺省 "tokenizer"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<String>,
    /// 推理后端，缺省 "cpu"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 推理线程数，缺省 2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_threads: Option<i32>,
    /// 每次喂给模型的采样数（@16k），缺省 3200（0.2s）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<usize>,
    /// 模型输入采样率，缺省 16000
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    /// 解码方式：greedy_search | modified_beam_search，缺省 greedy_search
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoding_method: Option<String>,
    /// 端点检测（静音自动断句），缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_endpoint: Option<bool>,
    /// 规则 1 最小尾随静音（秒），缺省 2.4
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule1_min_trailing_silence: Option<f32>,
    /// 规则 2 最小尾随静音（秒），缺省 1.2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule2_min_trailing_silence: Option<f32>,
    /// 规则 3 最小句长（秒），缺省 20.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule3_min_utterance_length: Option<f32>,
    /// 空白符惩罚，缺省 0.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blank_penalty: Option<f32>,
    /// 热词（空格分隔，中文直接写），缺省无（zipformer 走 context graph、
    /// Qwen3-ASR 转逗号格式嵌提示词、paraformer 不支持）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotwords: Option<String>,
    /// 是否对最终结果自动加标点，缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_punctuation: Option<bool>,
    /// 标点模型 onnx 路径（相对路径锚定标点模型目录）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub punctuation_model: Option<String>,
    /// 调试输出，缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

/// 文本转语音（TTS）配置。
///
/// 全部字段可缺省：未配置的项在解析时回退到 `tts::config` 的内置默认值，
/// 因此这里用 `Option` 以区分「未配置」与「配置了」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TtsSettings {
    /// 是否启用语音合成，缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 模型类型（sherpa-onnx 分支：zipvoice/vits/matcha/...；缺省按模型目录内容探测）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_type: Option<crate::tts::config::TtsModelKind>,
    /// 模型目录（支持 ${env.VAR} 引用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
    /// encoder onnx 文件名（缺省 int8 变体）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    /// decoder onnx 文件名（缺省 int8 变体）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    /// 声码器 vocoder onnx 文件名（缺省 vocos_24khz.onnx）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocoder: Option<String>,
    /// tokens.txt 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<String>,
    /// lexicon.txt 文件名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lexicon: Option<String>,
    /// espeak-ng 数据目录名（缺省 espeak-ng-data）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    /// 参考音频 wav 路径（相对模型目录；缺省 test_wavs/leijun-1.wav）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_wav: Option<String>,
    /// 参考音频的逐字转写文本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_text: Option<String>,
    /// 默认音色 id（如 `leijun-1` / 自定义音色 id；缺省 None = 用 reference_wav 即 leijun）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// 扩散解码步数（质量/速度权衡），缺省 4
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_steps: Option<i32>,
    /// 语速，缺省 1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// 推理后端，缺省 "cpu"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 推理线程数，缺省 2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_threads: Option<i32>,
    /// 调试输出，缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    /// TTS 引擎后端：sherpa（进程内，缺省）| audiocpp（audio.cpp sidecar 进程）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// audiocpp 引擎二进制覆盖路径（开发/调试用；缺省由 locator 自动定位）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_path: Option<String>,
}

/// 角色窗口位置（逻辑像素）。
///
/// `None` 表示未记录 → 首次启动时定位到屏幕右下角；记录后用于恢复手动拖动的位置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CompanionWindowPosition {
    /// 窗口左上角 x 坐标（逻辑像素）。
    pub x: i32,
    /// 窗口左上角 y 坐标（逻辑像素）。
    pub y: i32,
}

/// 角色窗口显示层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionWindowLayer {
    /// 置顶：悬浮在所有应用窗口之上（默认，现状）。
    #[default]
    Front,
    /// 置底：作为背景装饰，沉到所有应用窗口之下并完全点穿。
    Back,
}

/// 角色窗口拖拽模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompanionDragMode {
    /// 直接拖动：按住左键即可移动窗口（默认，现状）。
    #[default]
    Direct,
    /// 修饰键拖动：需按住 cmd（macOS）/ Ctrl（Windows、Linux）才能拖动。
    Modifier,
}

/// Live2D 角色配置。
///
/// 字段可缺省：未配置时回退到 `live2d::config` 的默认目录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Live2dSettings {
    /// 模型目录（支持 ${env.VAR} 引用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<String>,
    /// 角色窗口位置（逻辑像素；缺省表示未记录）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_position: Option<CompanionWindowPosition>,
    /// 角色窗口缩放比例（1.0 = 100%；缺省视为 1.0）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_scale: Option<f64>,
    /// 角色窗口透明度（1.0 = 不透明；缺省视为 1.0）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_opacity: Option<f64>,
    /// 角色窗口点击穿透（true = 鼠标事件全部穿透到身后窗口；缺省视为 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_through: Option<bool>,
    /// 角色窗口显示层级（置顶/置底；缺省视为置顶）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_layer: Option<CompanionWindowLayer>,
    /// 角色窗口位置锁定（true = 禁止拖动窗口；滚轮缩放与右键菜单保留；缺省视为 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
    /// 角色窗口拖拽模式（direct = 左键直接拖动；modifier = 需按住 cmd/Ctrl；缺省视为 direct）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drag_mode: Option<CompanionDragMode>,
}

/// 本地 LLM 配置。
///
/// 全部字段可缺省：未配置的项在解析时回退到 `llm::config` 的内置默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LlmSettings {
    /// 是否启用 LLM，缺省 false
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// provider 标识，缺省 "openai"（OpenAI 兼容 API）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// 角色 system prompt，缺省用内置默认
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 最多生成 token 数，缺省 512
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    /// 温度，缺省 0.7
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// top_p 采样，缺省 0.8
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// top_k 采样，缺省 20
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// min_p 采样，缺省 0.05
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    /// 重复惩罚，缺省 1.05
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    /// 随机种子，缺省 0（随机）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// HTTP provider 的 base URL（如 https://open.bigmodel.cn/api/paas/v4 或 Ollama 地址）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// HTTP provider 的 API key（本地 server / Ollama 可留空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// HTTP provider 的模型名（如 glm-4.7-flash / gpt-4o-mini）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// 语音会话配置（`voice run` 的会话级参数）。
///
/// 全部字段可缺省：未配置的项回退到 `voice::config` 的内置默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct VoiceSettings {
    /// 是否在应用启动时自动启动语音会话（进入待唤醒），缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 会话唤醒词（原始字符串，多个用 / 分隔），缺省 None = KWS 模型内置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// TTS 音色 id（如 leijun-1 / news-female / 自定义音色 id），缺省 None = 配置默认
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// TTS 语速，缺省 1.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// 最多对话轮数，缺省 None = 无限（Ctrl-C 退出）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// 传给 LLM 的历史消息条数上限，缺省 12
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_max: Option<usize>,
    /// 播报/思考中唤醒词打断，缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in: Option<bool>,
    /// 回复播完后自动进入 ASR 聆听（免唤醒续聊；空识别保持聆听不回待唤醒），缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up: Option<bool>,
    /// 打断用 KWS 触发阈值（高于监听阈值，缓解回声误触发），缺省 0.5
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barge_in_threshold: Option<f32>,
    /// 唤醒后的欢迎语文本（TTS 用当前音色合成播放），缺省 "你好，我在。"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_text: Option<String>,
    /// 「真正说话」RMS 音量阈值，缺省 0.02
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad_silence_threshold: Option<f32>,
    /// ASR 阶段连续静音多久判定说完（秒），缺省 3.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_max_trailing_silence: Option<f32>,
    /// 欢迎语后等用户真正说话的超时（秒），超时回待唤醒，缺省 8.0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_wait_timeout: Option<f32>,
}

/// dsh 桥配置。
///
/// 全部字段可缺省：未配置的项在解析时回退到 `dsh::config` 的内置默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DshSettings {
    /// 是否启用桥服务（loopback HTTP 监听），缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 监听端口，0 = 随机端口（默认，避免冲突），缺省 0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// 事件是否语音播报（voice 会话运行中只出气泡），缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_enabled: Option<bool>,
    /// 事件是否写入对话记录，缺省 true
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_to_history: Option<bool>,
}

fn default_log_level() -> String {
    "info".to_string()
}

/// 文字输入条窗口配置。
///
/// 字段可缺省：未配置时回退内置默认（隐藏 + 桌宠正上方/屏幕底部居中）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ChatboxSettings {
    /// 输入条是否显示（缺省视为 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// 输入条窗口位置（逻辑像素；缺省表示未记录 → 定位到桌宠正上方）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_position: Option<CompanionWindowPosition>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            debug: false,
            log_level: default_log_level(),
            hide_dock_icon: false,
            custom: None,
            microphone: None,
            data_dir: None,
            kws: None,
            asr: None,
            tts: None,
            live2d: None,
            llm: None,
            voice: None,
            model_library: None,
            shortcuts: None,
            dsh: None,
            chatbox: None,
        }
    }
}

/// 加载 ~/.zapmomo/settings.toml
///
/// 文件不存在时返回 None，不报错。
pub fn load_settings() -> Result<Option<AppConfig>, String> {
    let file_path = get_settings_path();

    let content = match std::fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };

    let config: AppConfig = toml::from_str(&content).map_err(|e| format!("TOML 格式错误: {e}"))?;

    Ok(Some(config))
}

/// 保存配置到 `~/.zapmomo/settings.toml`（自动创建父目录）。
///
/// 采用「临时文件 + 替换」的安全写：先把完整内容写入带 pid 后缀的临时文件，
/// 再 rename 到正式路径。POSIX 上 rename 同文件系统是原子的（直接覆盖）；Windows
/// 上 rename 无法覆盖已存在目标，先移除旧文件再 rename（存在短暂窗口）。若替换失败
/// 会保留临时文件便于恢复，并返回明确错误——**不做严格 atomic replace 的承诺**。
pub fn save_settings(config: &AppConfig) -> Result<(), String> {
    let file_path = get_settings_path();
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let content = toml::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {e}"))?;
    let tmp = file_path.with_file_name(format!("settings.toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &content).map_err(|e| format!("写入临时配置失败: {e}"))?;
    let renamed = match std::fs::rename(&tmp, &file_path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows：目标存在时 rename 可能失败，先移除再重试；失败则保留 tmp 便于恢复。
            if file_path.exists() {
                std::fs::remove_file(&file_path).map_err(|e| format!("移除旧配置失败: {e}"))?;
            }
            std::fs::rename(&tmp, &file_path).map_err(|e| format!("替换配置失败: {e}"))
        }
    };
    if renamed.is_ok() {
        // 应用内写入立即刷新 data_dir 缓存（mtime 同秒精度不足，不能只靠文件时间戳）
        refresh_data_dir_cache();
    }
    renamed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn write_toml_settings(home: &std::path::Path, content: &str) {
        let settings_dir = home.join(PROJECT_DIR);
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(settings_dir.join(SETTINGS_FILE), content).unwrap();
    }

    #[test]
    fn test_get_settings_path() {
        run_with_temp_home(|home| {
            let path = get_settings_path();
            assert_eq!(path, home.join(".zapmomo/settings.toml"));
        });
    }

    #[test]
    fn test_get_settings_dir() {
        run_with_temp_home(|home| {
            let dir = get_settings_dir();
            assert_eq!(dir, home.join(".zapmomo"));
        });
    }

    #[test]
    fn test_resolve_env_ref_plain_value() {
        assert_eq!(resolve_env_ref("plain-value").unwrap(), "plain-value");
        assert_eq!(
            resolve_env_ref("https://example.com").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn test_resolve_env_ref_from_env() {
        unsafe {
            std::env::set_var("TEST_MY_VAR", "test-resolved-value");
        }
        assert_eq!(
            resolve_env_ref("${env.TEST_MY_VAR}").unwrap(),
            "test-resolved-value"
        );
        unsafe {
            std::env::remove_var("TEST_MY_VAR");
        }
    }

    #[test]
    fn test_resolve_env_ref_missing_var() {
        let result = resolve_env_ref("${env.NONEXISTENT_VAR_XYZ}");
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("NONEXISTENT_VAR_XYZ"));
    }

    #[test]
    fn test_resolve_env_ref_empty() {
        assert_eq!(resolve_env_ref("").unwrap(), "");
    }

    #[test]
    fn test_resolve_env_ref_empty_env_var_name() {
        let result = resolve_env_ref("${env.}");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_settings_file_not_found() {
        run_with_temp_home(|_| {
            let result = load_settings().unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn test_load_settings_invalid_toml() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "{invalid}");
            let result = load_settings();
            assert!(result.is_err());
            assert!(result.err().unwrap().contains("TOML 格式错误"));
        });
    }

    #[test]
    fn test_load_settings_empty() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "");
            let result = load_settings().unwrap().unwrap();
            assert!(!result.debug);
            assert_eq!(result.log_level, "info");
            assert!(result.custom.is_none());
        });
    }

    #[test]
    fn test_load_settings_full() {
        run_with_temp_home(|home| {
            write_toml_settings(
                home,
                "debug = true\nlog_level = \"debug\"\n\n[custom]\nkey1 = \"value1\"\n",
            );
            let result = load_settings().unwrap().unwrap();
            assert!(result.debug);
            assert_eq!(result.log_level, "debug");
            assert_eq!(result.custom.unwrap().get("key1").unwrap(), "value1");
        });
    }

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert!(!config.debug);
        assert_eq!(config.log_level, "info");
        assert!(config.custom.is_none());
        assert!(config.microphone.is_none());
    }

    #[test]
    fn test_app_config_serde_roundtrip() {
        let config = AppConfig {
            debug: true,
            log_level: "warn".to_string(),
            hide_dock_icon: true,
            custom: Some(std::collections::HashMap::new()),
            microphone: Some("内置麦克风".to_string()),
            data_dir: None,
            kws: None,
            asr: None,
            tts: None,
            live2d: None,
            llm: None,
            voice: None,
            model_library: None,
            shortcuts: None,
            dsh: None,
            chatbox: None,
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized);
        // 缺省字段应被反序列化为 false；microphone 应被序列化
        assert!(toml_str.contains("hide_dock_icon"));
        assert!(toml_str.contains("microphone"));
    }

    #[test]
    fn test_load_settings_without_hide_dock_icon_defaults_false() {
        // 旧配置文件没有 hide_dock_icon 字段时，应回退为 false（默认展示图标）。
        run_with_temp_home(|home| {
            write_toml_settings(home, "debug = true\n");
            let result = load_settings().unwrap().unwrap();
            assert!(!result.hide_dock_icon);
        });
    }

    #[test]
    fn test_load_settings_with_hide_dock_icon() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "hide_dock_icon = true\n");
            let result = load_settings().unwrap().unwrap();
            assert!(result.hide_dock_icon);
        });
    }

    #[test]
    fn test_load_settings_with_microphone() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "microphone = \"内置麦克风\"\n");
            let result = load_settings().unwrap().unwrap();
            assert_eq!(result.microphone.as_deref(), Some("内置麦克风"));
        });
    }

    #[test]
    fn test_load_settings_with_asr_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[asr]\nnum_threads = 4\nenable_endpoint = false\n");
            let result = load_settings().unwrap().unwrap();
            let asr = result.asr.unwrap();
            assert_eq!(asr.num_threads, Some(4));
            assert_eq!(asr.enable_endpoint, Some(false));
            // 未配置的字段保持 None
            assert_eq!(asr.model_dir, None);
            assert_eq!(asr.decoding_method, None);
        });
    }

    #[test]
    fn test_load_settings_without_asr_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "debug = true\n");
            let result = load_settings().unwrap().unwrap();
            assert!(result.asr.is_none());
        });
    }

    #[test]
    fn test_load_settings_with_kws_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[kws]\nnum_threads = 4\nchunk_size = 1600\n");
            let result = load_settings().unwrap().unwrap();
            let kws = result.kws.unwrap();
            assert_eq!(kws.num_threads, Some(4));
            assert_eq!(kws.chunk_size, Some(1600));
            // 未配置的字段保持 None
            assert_eq!(kws.model_dir, None);
            assert_eq!(kws.keywords_threshold, None);
        });
    }

    #[test]
    fn test_load_settings_without_kws_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "debug = true\n");
            let result = load_settings().unwrap().unwrap();
            assert!(result.kws.is_none());
        });
    }

    #[test]
    fn test_load_settings_with_tts_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[tts]\nnum_threads = 4\nspeed = 1.2\n");
            let result = load_settings().unwrap().unwrap();
            let tts = result.tts.unwrap();
            assert_eq!(tts.num_threads, Some(4));
            assert_eq!(tts.speed, Some(1.2));
            // 未配置的字段保持 None
            assert_eq!(tts.model_dir, None);
            assert_eq!(tts.num_steps, None);
        });
    }

    #[test]
    fn test_load_settings_without_tts_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "debug = true\n");
            let result = load_settings().unwrap().unwrap();
            assert!(result.tts.is_none());
        });
    }

    #[test]
    fn test_load_settings_with_tts_enabled_false() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[tts]\nenabled = false\n");
            let result = load_settings().unwrap().unwrap();
            let tts = result.tts.unwrap();
            assert_eq!(tts.enabled, Some(false));
        });
    }

    #[test]
    fn test_tts_settings_serde_roundtrip() {
        let tts = TtsSettings {
            enabled: Some(false),
            model_type: Some(crate::tts::config::TtsModelKind::Zipvoice),
            model_dir: Some("${env.TTS_MODEL_DIR}".to_string()),
            encoder: Some("encoder.int8.onnx".to_string()),
            decoder: None,
            vocoder: Some("vocos_24khz.onnx".to_string()),
            tokens: None,
            lexicon: None,
            data_dir: None,
            reference_wav: Some("test_wavs/leijun-1.wav".to_string()),
            reference_text: None,
            voice: Some("custom-voice".to_string()),
            num_steps: Some(4),
            speed: Some(1.0),
            provider: Some("cpu".to_string()),
            num_threads: Some(2),
            debug: Some(false),
            backend: Some("audiocpp".to_string()),
            engine_path: None,
        };
        let toml_str = toml::to_string(&tts).unwrap();
        let deserialized: TtsSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(tts, deserialized);
        // 未配置字段应被 skip_serializing_if 忽略
        assert!(!toml_str.contains("decoder"));
        assert!(!toml_str.contains("engine_path"));
        assert!(toml_str.contains("backend = \"audiocpp\""));
    }

    #[test]
    fn test_get_tts_output_dir() {
        run_with_temp_home(|home| {
            assert_eq!(get_tts_output_dir(), home.join(".zapmomo/tts"));
        });
    }

    #[test]
    fn test_kws_settings_serde_roundtrip() {
        let kws = KwsSettings {
            enabled: Some(false),
            custom_keywords: Some("你好小智".to_string()),
            model_dir: Some("${env.KWS_MODEL_DIR}".to_string()),
            encoder: Some("encoder.onnx".to_string()),
            decoder: None,
            joiner: None,
            tokens: None,
            keywords_file: Some("kw.txt".to_string()),
            provider: Some("cpu".to_string()),
            num_threads: Some(4),
            chunk_size: Some(1600),
            sample_rate: Some(16000),
            keywords_score: Some(1.0),
            keywords_threshold: Some(0.3),
            debug: Some(false),
        };
        let toml_str = toml::to_string(&kws).unwrap();
        let deserialized: KwsSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(kws, deserialized);
        // 未配置字段应被 skip_serializing_if 忽略
        assert!(!toml_str.contains("decoder"));
    }

    #[test]
    fn test_kws_settings_env_ref_resolution() {
        unsafe {
            std::env::set_var("KWS_MODEL_DIR", "/tmp/kws-model");
        }
        let kws = KwsSettings {
            model_dir: Some("${env.KWS_MODEL_DIR}".to_string()),
            ..KwsSettings::default()
        };
        assert_eq!(
            resolve_env_ref(kws.model_dir.as_ref().unwrap()).unwrap(),
            "/tmp/kws-model"
        );
        unsafe {
            std::env::remove_var("KWS_MODEL_DIR");
        }
    }

    #[test]
    fn test_live2d_settings_serde_roundtrip() {
        let live2d = Live2dSettings {
            model_dir: Some("/tmp/some-model".to_string()),
            window_position: Some(CompanionWindowPosition { x: 120, y: 800 }),
            window_scale: Some(1.5),
            window_opacity: Some(0.6),
            click_through: Some(true),
            window_layer: Some(CompanionWindowLayer::Back),
            locked: Some(true),
            drag_mode: Some(CompanionDragMode::Modifier),
        };
        let toml_str = toml::to_string(&live2d).unwrap();
        assert!(toml_str.contains("click_through = true"));
        assert!(toml_str.contains("window_layer = \"back\""));
        assert!(toml_str.contains("locked = true"));
        assert!(toml_str.contains("drag_mode = \"modifier\""));
        let deserialized: Live2dSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(live2d, deserialized);
        assert_eq!(deserialized.window_layer, Some(CompanionWindowLayer::Back));
        assert_eq!(deserialized.drag_mode, Some(CompanionDragMode::Modifier));
        // 未记录位置/比例/穿透/层级时字段应被 skip_serializing_if 忽略
        let none_pos = Live2dSettings {
            model_dir: Some("/tmp/some-model".to_string()),
            window_position: None,
            window_scale: None,
            window_opacity: None,
            click_through: None,
            window_layer: None,
            locked: None,
            drag_mode: None,
        };
        let none_toml = toml::to_string(&none_pos).unwrap();
        assert!(!none_toml.contains("window_position"));
        assert!(!none_toml.contains("window_scale"));
        assert!(!none_toml.contains("window_opacity"));
        assert!(!none_toml.contains("click_through"));
        assert!(!none_toml.contains("window_layer"));
        assert!(!none_toml.contains("locked"));
        assert!(!none_toml.contains("drag_mode"));
        // 缺省层级为置顶
        assert_eq!(CompanionWindowLayer::default(), CompanionWindowLayer::Front);
        // 缺省拖拽模式为直接拖动
        assert_eq!(CompanionDragMode::default(), CompanionDragMode::Direct);
    }

    #[test]
    fn test_load_settings_with_live2d_table() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[live2d]\nmodel_dir = \"/tmp/model-dir\"\n");
            let result = load_settings().unwrap().unwrap();
            let live2d = result.live2d.unwrap();
            assert_eq!(live2d.model_dir.as_deref(), Some("/tmp/model-dir"));
            // 旧版配置无 click_through / locked / drag_mode 字段 → 反序列化回退 None（视为关闭/直拖）。
            assert_eq!(live2d.click_through, None);
            assert_eq!(live2d.locked, None);
            assert_eq!(live2d.drag_mode, None);
        });
    }

    #[test]
    fn test_live2d_drag_mode_invalid_value_rejected() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "[live2d]\ndrag_mode = \"bogus\"\n");
            let err = load_settings().unwrap_err();
            assert!(err.to_string().contains("drag_mode"));
        });
    }

    #[test]
    fn test_save_settings_roundtrip() {
        run_with_temp_home(|home| {
            let config = AppConfig {
                debug: true,
                log_level: "debug".to_string(),
                hide_dock_icon: false,
                custom: None,
                microphone: None,
                data_dir: None,
                kws: None,
                asr: None,
                tts: None,
                live2d: Some(Live2dSettings {
                    model_dir: Some("/tmp/model-dir".to_string()),
                    ..Default::default()
                }),
                llm: None,
                voice: None,
                model_library: None,
                shortcuts: None,
                dsh: None,
                chatbox: None,
            };
            save_settings(&config).unwrap();
            let loaded = load_settings().unwrap().unwrap();
            assert_eq!(loaded, config);
            // 文件确实写到了 HOME 下
            assert!(home.join(".zapmomo/settings.toml").is_file());
        });
    }

    // ---- data_dir（自定义数据目录）----

    /// 用 AppConfig + save_settings 写 data_dir（TOML 序列化器正确转义 Windows 反斜杠）。
    fn write_data_dir_settings(data_dir: Option<&str>) {
        let mut config = AppConfig::default();
        config.data_dir = data_dir.map(|s| s.to_string());
        save_settings(&config).unwrap();
    }

    #[test]
    fn test_data_dir_serde_roundtrip() {
        run_with_temp_home(|home| {
            write_data_dir_settings(Some("D:\\zapdata"));
            let loaded = load_settings().unwrap().unwrap();
            assert_eq!(loaded.data_dir.as_deref(), Some("D:\\zapdata"));
            // 未设置时字段不序列化
            let toml_str = toml::to_string(&AppConfig::default()).unwrap();
            assert!(!toml_str.contains("data_dir"));
            assert!(home.join(".zapmomo/settings.toml").is_file());
        });
    }

    #[test]
    fn test_get_models_dir_default_unchanged() {
        run_with_temp_home(|home| {
            assert_eq!(get_models_dir(), home.join(".zapmomo/models"));
            assert_eq!(legacy_models_dir(), None);
            assert_eq!(get_companions_store_dir(), home.join(".zapmomo/companions"));
            assert_eq!(legacy_companions_dir(), None);
        });
    }

    #[test]
    fn test_get_models_dir_custom_data_dir() {
        run_with_temp_home(|home| {
            let data = home.join("zapdata");
            write_data_dir_settings(Some(&data.display().to_string()));
            assert_eq!(get_data_dir(), Some(data.clone()));
            assert_eq!(get_models_dir(), data.join("models"));
            assert_eq!(get_companions_store_dir(), data.join("companions"));
            // 旧根指向默认位置（供双根扫描/迁移定位存量）
            assert_eq!(legacy_models_dir(), Some(home.join(".zapmomo/models")));
            assert_eq!(
                legacy_companions_dir(),
                Some(home.join(".zapmomo/companions"))
            );
        });
    }

    #[test]
    fn test_get_data_dir_env_ref_resolution() {
        run_with_temp_home(|home| {
            let env_dir = home.join("envdata");
            unsafe {
                std::env::set_var("TEST_ZM_DATA_DIR", &env_dir);
            }
            write_data_dir_settings(Some("${env.TEST_ZM_DATA_DIR}"));
            assert_eq!(get_data_dir(), Some(env_dir.clone()));
            assert_eq!(get_models_dir(), env_dir.join("models"));
            unsafe {
                std::env::remove_var("TEST_ZM_DATA_DIR");
            }
        });
    }

    #[test]
    fn test_get_data_dir_invalid_env_falls_back() {
        run_with_temp_home(|home| {
            write_data_dir_settings(Some("${env.NONEXISTENT_DATA_DIR_XYZ}"));
            assert_eq!(get_data_dir(), None);
            assert_eq!(get_models_dir(), home.join(".zapmomo/models"));
            assert_eq!(legacy_models_dir(), None);
        });
    }

    #[test]
    fn test_strip_prefix_ci() {
        if cfg!(windows) {
            // Windows：大小写不敏感 + 分隔符宽容，剥离前缀后返回相对路径
            let prefix = std::path::Path::new("C:\\Users\\Admin\\zapdata\\models");
            let path = std::path::Path::new("c:\\users\\admin\\zapdata\\models\\llm\\model.gguf");
            let rest = strip_prefix_ci(path, prefix).unwrap();
            assert_eq!(rest, std::path::Path::new("llm\\model.gguf"));
            // 不在前缀下 → None
            let other = std::path::Path::new("D:\\other\\x");
            assert!(strip_prefix_ci(other, prefix).is_none());
            // 前缀自身 → 返回空
            let exact = std::path::Path::new("C:\\Users\\Admin\\zapdata\\models");
            assert_eq!(
                strip_prefix_ci(exact, prefix).unwrap(),
                std::path::Path::new("")
            );
            // 部分段重合（models2）不算前缀，防迁移误改写
            let sibling = std::path::Path::new("C:\\Users\\Admin\\zapdata\\models2\\x.gguf");
            assert!(strip_prefix_ci(sibling, prefix).is_none());
        } else {
            // Unix：大小写敏感、仅 `/` 为分隔符
            let prefix = std::path::Path::new("/home/user/zapdata/models");
            let path = std::path::Path::new("/home/user/zapdata/models/llm/model.gguf");
            assert_eq!(
                strip_prefix_ci(path, prefix).unwrap(),
                std::path::Path::new("llm/model.gguf")
            );
            // 大小写不同 → None
            let mixed = std::path::Path::new("/Home/User/zapdata/models/m.gguf");
            assert!(strip_prefix_ci(mixed, prefix).is_none());
            // 不在前缀下 → None
            let other = std::path::Path::new("/opt/other/x");
            assert!(strip_prefix_ci(other, prefix).is_none());
            // 前缀自身 → 返回空
            let exact = std::path::Path::new("/home/user/zapdata/models");
            assert_eq!(
                strip_prefix_ci(exact, prefix).unwrap(),
                std::path::Path::new("")
            );
            // 部分段重合（models2）不算前缀，防迁移误改写
            let sibling = std::path::Path::new("/home/user/zapdata/models2/x.gguf");
            assert!(strip_prefix_ci(sibling, prefix).is_none());
        }
    }

    #[test]
    fn test_get_data_dir_relative_falls_back() {
        run_with_temp_home(|_| {
            write_data_dir_settings(Some("relative/dir"));
            assert_eq!(get_data_dir(), None);
        });
    }

    #[test]
    fn test_data_dir_cache_mtime_invalidation() {
        run_with_temp_home(|home| {
            let d1 = home.join("d1");
            let d2 = home.join("d2");
            write_data_dir_settings(Some(&d1.display().to_string()));
            assert_eq!(get_data_dir(), Some(d1.clone()));
            // 直接改文件（不经 refresh_data_dir_cache）：mtime 变化应自动失效缓存
            write_data_dir_settings(Some(&d2.display().to_string()));
            assert_eq!(get_data_dir(), Some(d2.clone()));
            // 显式刷新后同样正确
            write_data_dir_settings(Some(&d1.display().to_string()));
            refresh_data_dir_cache();
            assert_eq!(get_data_dir(), Some(d1));
        });
    }

    #[test]
    fn test_save_settings_safe_replace_and_tmp_cleanup() {
        run_with_temp_home(|home| {
            let config = AppConfig {
                log_level: "debug".to_string(),
                ..Default::default()
            };
            save_settings(&config).unwrap();
            // 正式文件存在
            assert!(home.join(".zapmomo/settings.toml").is_file());
            // 临时文件被清理（rename 成功）
            let tmp = home.join(format!(".zapmomo/settings.toml.tmp.{}", std::process::id()));
            assert!(!tmp.exists());
            // 覆盖保存仍成功且内容完整
            let config2 = AppConfig {
                log_level: "warn".to_string(),
                ..Default::default()
            };
            save_settings(&config2).unwrap();
            let loaded = load_settings().unwrap().unwrap();
            assert_eq!(loaded.log_level, "warn");
        });
    }

    // ---- dsh（dsh 桥配置段）----

    #[test]
    fn test_parse_dsh_section() {
        run_with_temp_home(|_| {
            std::fs::create_dir_all(get_settings_dir()).unwrap();
            std::fs::write(
                get_settings_path(),
                "[dsh]\nenabled = false\nport = 47800\nvoice_enabled = false\n",
            )
            .unwrap();
            let cfg = load_settings().unwrap().unwrap();
            let dsh = cfg.dsh.expect("[dsh] 段应解析");
            assert_eq!(dsh.enabled, Some(false));
            assert_eq!(dsh.port, Some(47800));
            assert_eq!(dsh.voice_enabled, Some(false));
            assert_eq!(dsh.record_to_history, None);
        });
    }

    #[test]
    fn test_dsh_section_absent_defaults_none() {
        run_with_temp_home(|home| {
            write_toml_settings(home, "debug = true\n");
            let cfg = load_settings().unwrap().unwrap();
            assert!(cfg.dsh.is_none());
        });
    }
}
