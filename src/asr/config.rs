/// ASR 配置解析与校验。
///
/// 负责把 `settings.toml` 的 `[asr]` 表与 CLI flag 合并成一份已展开、已填默认值的
/// `ResolvedAsrConfig`。优先级：CLI `--model-dir` > settings > 内置默认。
use crate::config::settings::{AsrSettings, resolve_env_ref};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 模型包内默认文件名。
///
/// 采用 sherpa-onnx 官方 int8 配方：int8 encoder/joiner 搭配 fp32 decoder
/// （decoder 体积极小，官方 int8 示例即用 fp32 decoder）。
pub const DEFAULT_ENCODER: &str = "encoder-epoch-99-avg-1.int8.onnx";
pub const DEFAULT_DECODER: &str = "decoder-epoch-99-avg-1.onnx";
pub const DEFAULT_JOINER: &str = "joiner-epoch-99-avg-1.int8.onnx";
pub const DEFAULT_TOKENS: &str = "tokens.txt";

/// 模型安装完成所需的文件（相对目标目录）。
pub const REQUIRED_FILES: [&str; 4] = [
    DEFAULT_ENCODER,
    DEFAULT_DECODER,
    DEFAULT_JOINER,
    DEFAULT_TOKENS,
];

/// 标点模型包内文件名（CT Transformer 单文件）。
pub const DEFAULT_PUNCT_MODEL: &str = "model.onnx";

/// 标点模型安装完成所需的文件（相对目标目录）。
pub const PUNCT_REQUIRED_FILES: [&str; 1] = [DEFAULT_PUNCT_MODEL];

/// ASR 模型类型（sherpa-onnx `OfflineModelConfig` / `OnlineModelConfig` 的分支）。
///
/// 全链路显式传递：`[asr].model_type`（持久化）→ `ResolvedAsrConfig.model_type` →
/// 引擎构造分支（`AsrEngine` 流式 / `offline::OfflineAsrEngine` 离线）。默认 Zipformer
/// （streaming zipformer transducer，现状 6 个注册模型），老配置无该字段时按目录内容探测兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrModelKind {
    /// 流式 zipformer transducer（encoder/decoder/joiner/tokens 四件套）
    #[default]
    Zipformer,
    /// 流式 Paraformer：`encoder.onnx|int8` + `decoder.onnx|int8` + `tokens.txt`
    /// （仅 greedy_search；热词为 transducer 专属，本族忽略）
    Paraformer,
    /// 离线 SenseVoice：单 `model.onnx` + `tokens.txt`（多语言 + 情绪/事件标签）
    #[serde(rename = "sensevoice")]
    SenseVoice,
    /// 离线 Whisper：`<size>-encoder.onnx` + `<size>-decoder.onnx` + `<size>-tokens.txt`
    Whisper,
    /// 离线 Qwen3-ASR：`conv_frontend.onnx` + 裸名 `encoder/decoder.int8.onnx` +
    /// `tokenizer/` 目录（LLM 自回归解码，29 语言自动识别，离线族中唯一支持热词）
    Qwen3Asr,
}

impl AsrModelKind {
    /// snake_case 字符串（配置/JSON 直传）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zipformer => "zipformer",
            Self::Paraformer => "paraformer",
            Self::SenseVoice => "sensevoice",
            Self::Whisper => "whisper",
            Self::Qwen3Asr => "qwen3_asr",
        }
    }

    /// 解析 snake_case 字符串（与 `ModelType::from_str_value` 同款命名）。
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "zipformer" => Some(Self::Zipformer),
            "paraformer" => Some(Self::Paraformer),
            "sensevoice" => Some(Self::SenseVoice),
            "whisper" => Some(Self::Whisper),
            "qwen3_asr" => Some(Self::Qwen3Asr),
            _ => None,
        }
    }

    /// 是否流式（可走实时语音会话 / `AsrEngine`）。
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Zipformer | Self::Paraformer)
    }

    /// 是否离线（仅整段文件转写，走 `offline::OfflineAsrEngine`）。
    pub fn is_offline(&self) -> bool {
        !self.is_streaming()
    }
}

/// ASR 引擎后端（镜像 `crate::tts::config::TtsBackendKind` 的正交语义：
/// kind 表达模型族、backend 表达运行时）。
///
/// - Sherpa（缺省）：sherpa-onnx 进程内引擎（现状全部 5 个族）；
/// - Audiocpp：audio.cpp sidecar 进程（GGUF + Metal），当前仅 Qwen3Asr 可查
///   （见 `crate::audiocpp::asr_families`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrBackendKind {
    /// sherpa-onnx 进程内引擎（缺省，老配置无 backend 字段时的行为）
    #[default]
    Sherpa,
    /// audio.cpp sidecar 进程（audiocpp_server，OpenAI 风格 HTTP）
    Audiocpp,
}

impl AsrBackendKind {
    /// snake_case 字符串（配置/JSON 直传）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sherpa => "sherpa",
            Self::Audiocpp => "audiocpp",
        }
    }

    /// 解析 snake_case 字符串（与 `AsrModelKind::parse_str` 同款命名）。
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "sherpa" => Some(Self::Sherpa),
            "audiocpp" => Some(Self::Audiocpp),
            _ => None,
        }
    }
}

/// SenseVoice 主模型默认文件名（注册 int8 变体
/// `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17`；fp32 包 999MB，桌面伙伴不划算）。
pub const SENSEVOICE_MODEL: &str = "model.int8.onnx";

/// SenseVoice 模型安装完成所需的文件（相对目标目录）。
pub const SENSEVOICE_REQUIRED_FILES: [&str; 2] = [SENSEVOICE_MODEL, DEFAULT_TOKENS];

/// Whisper tiny 模型安装完成所需的文件（sherpa-onnx-whisper-tiny）。
pub const WHISPER_TINY_ENCODER: &str = "tiny-encoder.onnx";
pub const WHISPER_TINY_DECODER: &str = "tiny-decoder.onnx";
pub const WHISPER_TINY_TOKENS: &str = "tiny-tokens.txt";
pub const WHISPER_TINY_REQUIRED_FILES: [&str; 3] = [
    WHISPER_TINY_ENCODER,
    WHISPER_TINY_DECODER,
    WHISPER_TINY_TOKENS,
];

/// Whisper base 模型安装完成所需的文件（sherpa-onnx-whisper-base）。
pub const WHISPER_BASE_ENCODER: &str = "base-encoder.onnx";
pub const WHISPER_BASE_DECODER: &str = "base-decoder.onnx";
pub const WHISPER_BASE_TOKENS: &str = "base-tokens.txt";
pub const WHISPER_BASE_REQUIRED_FILES: [&str; 3] = [
    WHISPER_BASE_ENCODER,
    WHISPER_BASE_DECODER,
    WHISPER_BASE_TOKENS,
];

/// 流式 Paraformer 默认文件名（官方包 int8 变体；fp32 包内同在，运行时不默认消费）。
pub const PARAFORMER_ENCODER: &str = "encoder.int8.onnx";
pub const PARAFORMER_DECODER: &str = "decoder.int8.onnx";

/// 流式 Paraformer 模型安装完成所需的文件（相对模型目录）。
///
/// 与 zipformer 的 `{prefix}-epoch-99-...` 命名不同，官方 paraformer 包是裸名
/// `encoder/decoder`（int8 与 fp32 各一份）；完整性按默认消费的 int8 三件判。
pub const PARAFORMER_REQUIRED_FILES: [&str; 3] =
    [PARAFORMER_ENCODER, PARAFORMER_DECODER, DEFAULT_TOKENS];

/// Qwen3-ASR 卷积前端默认文件名（官方包固定名，无 int8 变体）。
pub const QWEN3_CONV_FRONTEND: &str = "conv_frontend.onnx";

/// Qwen3-ASR tokenizer 目录名（无 tokens.txt，tokenizer 从目录加载）。
pub const QWEN3_TOKENIZER_DIR: &str = "tokenizer";

/// Qwen3-ASR 模型安装完成所需的文件（相对模型目录）。
///
/// 注意：安装完整性判定（`has_required_files`）是 `is_file` 语义，tokenizer
/// 目录不能作条目，以其内部三文件表达（子路径先例：KWS 的
/// `test_wavs/test_keywords.txt`）。
pub const QWEN3_REQUIRED_FILES: [&str; 6] = [
    QWEN3_CONV_FRONTEND,
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "tokenizer/vocab.json",
    "tokenizer/merges.txt",
    "tokenizer/tokenizer_config.json",
];

/// 各模型类型安装完成所需的文件（相对模型目录）。
///
/// Whisper 因 tiny/base 尺寸前缀不同返回空，运行时由目录探测
/// （`detect_whisper_prefix`）兜底；安装完整性按 registry role 精确声明。
pub fn required_files(kind: AsrModelKind) -> &'static [&'static str] {
    match kind {
        AsrModelKind::Zipformer => &REQUIRED_FILES,
        AsrModelKind::Paraformer => &PARAFORMER_REQUIRED_FILES,
        AsrModelKind::SenseVoice => &SENSEVOICE_REQUIRED_FILES,
        AsrModelKind::Whisper => &[],
        AsrModelKind::Qwen3Asr => &QWEN3_REQUIRED_FILES,
    }
}

/// 解析后的完整 ASR 配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAsrConfig {
    /// 是否启用 ASR（语音会话「能识别」的前提），缺省 false
    pub enabled: bool,
    /// 模型类型（决定引擎构造分支；默认 Zipformer，老配置按目录探测兜底）
    pub model_type: AsrModelKind,
    /// 引擎后端（sherpa 进程内 / audiocpp sidecar；默认 Sherpa）
    pub backend: AsrBackendKind,
    /// audiocpp 引擎二进制覆盖路径（开发/调试用；None = locator 自动定位）
    pub engine_path: Option<PathBuf>,
    pub model_dir: PathBuf,
    /// SenseVoice 主模型 `model.onnx` / Qwen3-ASR 卷积前端 `conv_frontend.onnx`
    /// （whisper/zipformer 为 None）
    pub model: Option<PathBuf>,
    /// SenseVoice/Whisper 转写语言（None = 自动检测；Qwen3-ASR 恒 None）
    pub language: Option<String>,
    /// SenseVoice 反向文本正则化（数字/标点，缺省 true；Qwen3-ASR 不适用）
    pub use_itn: Option<bool>,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    /// tokens 文件（zipformer/paraformer/sensevoice/whisper）；
    /// Qwen3-ASR 为 tokenizer 目录（无 tokens.txt，tokenizer 从目录加载）
    pub tokens: PathBuf,
    pub provider: String,
    pub num_threads: i32,
    /// 每次喂给模型的采样数（@sample_rate）
    pub chunk_size: usize,
    pub sample_rate: i32,
    /// 解码方式：greedy_search | modified_beam_search
    pub decoding_method: String,
    /// 端点检测（静音自动断句）
    pub enable_endpoint: bool,
    pub rule1_min_trailing_silence: f32,
    pub rule2_min_trailing_silence: f32,
    pub rule3_min_utterance_length: f32,
    /// transducer 空白符惩罚（通常 0.0）
    pub blank_penalty: f32,
    /// 热词（空格分隔，中文直接写），提升专有名词识别。
    /// zipformer 走 context graph；Qwen3-ASR 在 build 层转逗号格式嵌入提示词；
    /// paraformer 不支持（引擎层忽略）
    pub hotwords: Option<String>,
    /// 是否对最终结果自动加标点
    pub enable_punctuation: bool,
    /// 标点模型 onnx 路径
    pub punctuation_model: PathBuf,
    pub debug: bool,
}

impl Default for ResolvedAsrConfig {
    fn default() -> Self {
        let model_dir = default_model_dir();
        let join = |name: &str| model_dir.join(name);
        Self {
            enabled: false,
            model_type: AsrModelKind::Zipformer,
            backend: AsrBackendKind::Sherpa,
            engine_path: None,
            model: None,
            language: None,
            use_itn: None,
            encoder: join(DEFAULT_ENCODER),
            decoder: join(DEFAULT_DECODER),
            joiner: join(DEFAULT_JOINER),
            tokens: join(DEFAULT_TOKENS),
            model_dir,
            provider: "cpu".to_string(),
            num_threads: 2,
            chunk_size: 3200,
            sample_rate: 16000,
            decoding_method: "greedy_search".to_string(),
            enable_endpoint: true,
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: 1.2,
            rule3_min_utterance_length: 20.0,
            blank_penalty: 0.0,
            hotwords: None,
            enable_punctuation: true,
            punctuation_model: punctuation_default_dir().join(DEFAULT_PUNCT_MODEL),
            debug: false,
        }
    }
}

/// 用户默认模型目录：`~/.zapmomo/models/<模型名>`
pub fn user_default_model_dir() -> PathBuf {
    crate::kws::model::asr_user_model_dir()
}

/// 源码仓库中的模型目录（开发者 `./models/<模型名>`，仅作开发回退）。
fn repo_models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(&crate::kws::model::asr_asset().name)
}

/// 默认模型目录选择：用户已安装 > 旧默认根存量（data_dir 切换后）> 源码仓库已下载（开发便利）> 用户默认。
///
/// 纯决策函数（不访问真实文件系统），便于测试注入路径。
fn choose_default_model_dir(user: &Path, legacy: Option<&Path>, repo: &Path) -> PathBuf {
    if user.join(DEFAULT_TOKENS).is_file() {
        user.to_path_buf()
    } else if legacy.is_some_and(|l| l.join(DEFAULT_TOKENS).is_file()) {
        legacy.unwrap().to_path_buf()
    } else if repo.join(DEFAULT_TOKENS).is_file() {
        repo.to_path_buf()
    } else {
        user.to_path_buf()
    }
}

/// 默认模型目录（运行时解析：优先用户目录，旧根存量兜底，源码开发时回退到仓库 `./models/`）。
pub fn default_model_dir() -> PathBuf {
    // legacy 与 user 层次对等：旧根下对应模型的子目录（user 是 `models/<模型名>`）
    let legacy = crate::config::settings::legacy_models_dir()
        .map(|l| l.join(&crate::kws::model::asr_asset().name));
    choose_default_model_dir(
        &user_default_model_dir(),
        legacy.as_deref(),
        &repo_models_dir(),
    )
}

/// 标点模型默认目录：用户目录（`~/.zapmomo/models/<标点名>`）优先，旧根存量兜底。
fn punctuation_default_dir() -> PathBuf {
    let new = crate::kws::model::punctuation_user_model_dir();
    if new.join(DEFAULT_PUNCT_MODEL).is_file() {
        return new;
    }
    if let Some(legacy) = crate::config::settings::legacy_models_dir() {
        let legacy_dir = legacy.join(new.file_name().unwrap_or_default());
        if legacy_dir.join(DEFAULT_PUNCT_MODEL).is_file() {
            return legacy_dir;
        }
    }
    new
}

/// 展开 settings 中的路径字符串（支持 `${env.VAR}`），未配置时用默认文件名。
/// 返回的路径若为相对路径则拼接在 `model_dir` 下。
fn resolve_file(
    settings_value: Option<&str>,
    default_name: &str,
    model_dir: &Path,
) -> Result<PathBuf, String> {
    match settings_value {
        Some(v) => {
            let expanded = resolve_env_ref(v)?;
            let p = PathBuf::from(&expanded);
            Ok(if p.is_absolute() {
                p
            } else {
                model_dir.join(p)
            })
        }
        None => Ok(model_dir.join(default_name)),
    }
}

/// onnx 默认文件名探测：settings 未显式配置某 onnx 文件时按模型目录内容选择。
///
/// 与 KWS 的 `detect_default_onnx` 规则不同：ASR 官方 int8 配方是 int8 encoder/joiner +
/// fp32 decoder（int8 偏好按组件分方向），且文件名不一定含 chunk-16（双语模型就不含），
/// 故各自维护、注释互引，不做参数化共享。
///
/// 规则（确定性，read_dir 顺序不确定故候选排序）：
/// 1. 默认常量文件名存在 → 直接用（已装注册模型零行为变化，实测 6 个注册包常量文件都在）；
/// 2. 否则收集目录中 `{prefix}-` 开头、`.onnx` 结尾的文件：优先子集 =
///    `prefer_int8` 时含 `.int8`、否则不含 `.int8`，子集内字母序取第一个；
///    优先子集为空 → 全体候选字母序取第一个（如 int8-only 目录的 decoder 取 int8，可运行）；
/// 3. 目录不存在或无候选 → 回退默认常量名（后续预检报「缺少模型文件」，错误路径清晰）。
fn detect_default_onnx(
    model_dir: &Path,
    prefix: &str,
    fallback: &str,
    prefer_int8: bool,
) -> String {
    if model_dir.join(fallback).is_file() {
        return fallback.to_string();
    }
    let Ok(entries) = std::fs::read_dir(model_dir) else {
        return fallback.to_string();
    };
    let mut candidates: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.starts_with(&format!("{prefix}-")) && n.ends_with(".onnx"))
        .collect();
    candidates.sort();
    candidates
        .iter()
        .find(|n| n.contains(".int8") == prefer_int8)
        .or_else(|| candidates.first())
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

/// settings 未显式配置某文件字段时的默认名探测入口（tokens 各模型同名，不探测）。
///
/// int8 偏好按组件分方向：encoder/joiner 偏好 int8（官方量化配方，与默认常量一致），
/// decoder 偏好 fp32（体积小、int8 收益可忽略，官方示例即用 fp32）。
fn detect_default_name(field: &str, model_dir: &Path, fallback: &str) -> String {
    match field {
        "encoder" => detect_default_onnx(model_dir, "encoder", fallback, true),
        "decoder" => detect_default_onnx(model_dir, "decoder", fallback, false),
        "joiner" => detect_default_onnx(model_dir, "joiner", fallback, true),
        _ => fallback.to_string(),
    }
}

/// 目录内匹配 SenseVoice 主模型：`model.onnx` / `model.int8.onnx` / `model-*.onnx`。
///
/// 与 `detect_default_onnx` 的 `{prefix}-` 前缀探测不同：SenseVoice 是 `model.onnx`（无连字符），
/// 用等值/前缀双匹配，候选字母序取第一个。
fn detect_model_onnx(model_dir: &Path) -> Option<String> {
    let Ok(entries) = std::fs::read_dir(model_dir) else {
        return None;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n == "model.onnx" || n == "model.int8.onnx" || n.starts_with("model-"))
        .filter(|n| n.ends_with(".onnx"))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// 从 `*-encoder.onnx` / `*-encoder.int8.onnx` 推断 whisper 尺寸前缀（tiny/base/...）。
///
/// 字母序取第一个（`base` < `tiny`，确定性），无匹配返回 None。
fn detect_whisper_prefix(model_dir: &Path) -> Option<String> {
    let Ok(entries) = std::fs::read_dir(model_dir) else {
        return None;
    };
    let mut prefixes: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter_map(|n| {
            n.strip_suffix("-encoder.onnx")
                .or_else(|| n.strip_suffix("-encoder.int8.onnx"))
                .map(str::to_string)
        })
        .filter(|p| !p.is_empty())
        .collect();
    prefixes.sort();
    prefixes.into_iter().next()
}

/// 裸名组件默认文件探测（Paraformer/Qwen3-ASR 共用）：`<component>.int8.onnx`
/// 存在取 int8，否则回退 `<component>.onnx`（fp32-only 目录可运行；两文件均缺时
/// 回退 int8 名，后续预检报「缺少模型文件」，错误路径清晰）。
///
/// 与 zipformer 的 `{prefix}-` 前缀探测、SenseVoice 的 `model*.onnx` 探测不同：
/// 官方 paraformer/qwen3 包是裸名 `encoder.onnx`/`decoder.onnx`，用精确名二选一。
fn detect_bare_int8_onnx(model_dir: &Path, component: &str) -> String {
    let int8 = format!("{component}.int8.onnx");
    if model_dir.join(&int8).is_file() {
        int8
    } else {
        format!("{component}.onnx")
    }
}

/// 目录是否含一套流式 Paraformer 文件（encoder/decoder 各 int8|fp32 之一 + tokens）。
///
/// 裸名与其它族的文件名形状互斥：whisper 探针需 `-encoder.onnx` 连字符前缀，
/// zipformer 官方包均为 `encoder-epoch-99-...` 前缀命名。
fn paraformer_files_detected(model_dir: &Path) -> bool {
    let encoder =
        model_dir.join("encoder.int8.onnx").is_file() || model_dir.join("encoder.onnx").is_file();
    let decoder =
        model_dir.join("decoder.int8.onnx").is_file() || model_dir.join("decoder.onnx").is_file();
    encoder && decoder && model_dir.join(DEFAULT_TOKENS).is_file()
}

/// 目录是否含一套 Qwen3-ASR 文件（`conv_frontend.onnx` 独有标记 + 裸名
/// int8|fp32 encoder/decoder + `tokenizer/` 目录）。
///
/// `conv_frontend.onnx` 是与其它族文件名形状互斥的独有指纹（paraformer 探针要求
/// tokens.txt，qwen3 无 tokens.txt），放 `detect_kind_from_dir` 首位保确定性。
fn qwen3_files_detected(model_dir: &Path) -> bool {
    let conv = model_dir.join(QWEN3_CONV_FRONTEND).is_file();
    let enc =
        model_dir.join("encoder.int8.onnx").is_file() || model_dir.join("encoder.onnx").is_file();
    let dec =
        model_dir.join("decoder.int8.onnx").is_file() || model_dir.join("decoder.onnx").is_file();
    conv && enc && dec && model_dir.join(QWEN3_TOKENIZER_DIR).is_dir()
}

/// 目录内容探测模型类型（settings 未配置 `model_type` 时的兜底）。
///
/// 文件探针：`conv_frontend.onnx`+裸名 encoder/decoder+`tokenizer/` → Qwen3-ASR；
/// `model.onnx`+tokens → SenseVoice；`*-encoder.onnx` 系 → Whisper；
/// 裸名 `encoder/decoder.onnx`+tokens → Paraformer；
/// 否则 Zipformer（含空目录/不存在）。managed 安装目录名 == registry name 的
/// 权威匹配在 `set_selected_model` 写配置时已保证，此处仅兜底外部/本地目录。
pub fn detect_kind_from_dir(model_dir: &Path) -> AsrModelKind {
    // qwen3 首位：conv_frontend.onnx 是独有指纹（其余族探针都不会命中它），
    // 且 qwen3 无 tokens.txt，若不先判会被 paraformer 之前的探针漏过、最终
    // 落到 Zipformer 兜底。
    if qwen3_files_detected(model_dir) {
        return AsrModelKind::Qwen3Asr;
    }
    if detect_model_onnx(model_dir).is_some() && model_dir.join(DEFAULT_TOKENS).is_file() {
        return AsrModelKind::SenseVoice;
    }
    if let Some(prefix) = detect_whisper_prefix(model_dir)
        && model_dir.join(format!("{prefix}-decoder.onnx")).is_file()
        && model_dir.join(format!("{prefix}-tokens.txt")).is_file()
    {
        return AsrModelKind::Whisper;
    }
    if paraformer_files_detected(model_dir) {
        return AsrModelKind::Paraformer;
    }
    AsrModelKind::Zipformer
}

/// 按模型类型判定目录是否探测得到完整的一套模型文件（供 `asr_files_present` 分派）。
pub fn asr_files_present_for_kind(model_dir: &Path, kind: AsrModelKind) -> bool {
    match kind {
        AsrModelKind::Zipformer => {
            let files = [
                detect_default_onnx(model_dir, "encoder", DEFAULT_ENCODER, true),
                detect_default_onnx(model_dir, "decoder", DEFAULT_DECODER, false),
                detect_default_onnx(model_dir, "joiner", DEFAULT_JOINER, true),
                DEFAULT_TOKENS.to_string(),
            ];
            files.iter().all(|f| model_dir.join(f).is_file())
        }
        AsrModelKind::Paraformer => paraformer_files_detected(model_dir),
        AsrModelKind::SenseVoice => {
            detect_model_onnx(model_dir).is_some() && model_dir.join(DEFAULT_TOKENS).is_file()
        }
        AsrModelKind::Whisper => detect_whisper_prefix(model_dir).is_some_and(|prefix| {
            model_dir.join(format!("{prefix}-encoder.onnx")).is_file()
                && model_dir.join(format!("{prefix}-decoder.onnx")).is_file()
                && model_dir.join(format!("{prefix}-tokens.txt")).is_file()
        }),
        AsrModelKind::Qwen3Asr => qwen3_files_detected(model_dir),
    }
}

/// 目录内是否探测得到完整的一套 ASR 模型文件（模型无关 + 族感知，替代按默认文件名
/// 硬编码的判定，供模型库 external/HF 导入的完整性检查复用；对称 KWS 的 `kws_files_present`）。
pub fn asr_files_present(model_dir: &Path) -> bool {
    asr_files_present_for_kind(model_dir, detect_kind_from_dir(model_dir))
}

/// ASR 就绪预检（backend 感知的单一权威入口，镜像 `tts::config::preflight`）。
///
/// - sherpa：按 `asr_files_present_for_kind` 探测式校验；
/// - audiocpp：按 `audiocpp::asr_families` 描述表的 `required_files`（单 GGUF）
///   逐文件校验，hint 用描述表的 `registry_hint`；sherpa-only kind 配 audiocpp
///   后端的非法组合报错（resolve 已 fail-fast，此处双保险）。
pub fn preflight(cfg: &ResolvedAsrConfig) -> Result<(), String> {
    if models_present(cfg) {
        return Ok(());
    }
    let hint = match cfg.backend {
        AsrBackendKind::Sherpa => "请运行 `zapmomo asr install-model` 下载模型".to_string(),
        AsrBackendKind::Audiocpp => {
            match crate::audiocpp::asr_families::asr_family_desc(cfg.model_type) {
                Some(desc) => format!("请运行 `{hint}` 下载模型", hint = desc.registry_hint),
                None => {
                    return Err(format!(
                        "模型类型 {} 不支持 audiocpp 后端（请检查 [asr].model_type 与 backend 组合）",
                        cfg.model_type.as_str()
                    ));
                }
            }
        }
    };
    Err(format!(
        "缺少模型文件（目录: {}）。{hint}",
        cfg.model_dir.display()
    ))
}

/// 模型文件是否齐备（backend 感知；供 `get_asr_config` 的 models_present 等展示路径）。
pub fn models_present(cfg: &ResolvedAsrConfig) -> bool {
    match cfg.backend {
        AsrBackendKind::Sherpa => asr_files_present_for_kind(&cfg.model_dir, cfg.model_type),
        AsrBackendKind::Audiocpp => {
            match crate::audiocpp::asr_families::asr_family_desc(cfg.model_type) {
                Some(desc) => desc
                    .required_files
                    .iter()
                    .all(|f| cfg.model_dir.join(f).is_file()),
                None => false,
            }
        }
    }
}

/// 解析模型目录：CLI > settings > 默认。
fn resolve_model_dir(
    settings: Option<&AsrSettings>,
    cli_model_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(dir) = cli_model_dir {
        return Ok(dir.to_path_buf());
    }
    if let Some(dir) = settings.and_then(|s| s.model_dir.as_deref()) {
        let expanded = resolve_env_ref(dir)?;
        let p = PathBuf::from(expanded);
        return Ok(if p.is_absolute() {
            p
        } else {
            crate::config::settings::get_settings_dir().join(p)
        });
    }
    Ok(default_model_dir())
}

/// 合并配置并填充默认值。
pub fn resolve(
    settings: Option<&AsrSettings>,
    cli_model_dir: Option<&Path>,
) -> Result<ResolvedAsrConfig, String> {
    let mut cfg = ResolvedAsrConfig {
        model_dir: resolve_model_dir(settings, cli_model_dir)?,
        ..ResolvedAsrConfig::default()
    };

    let s = settings;
    let file = |field: &str, default_name: &str| {
        let value = match field {
            "encoder" => s.and_then(|s| s.encoder.as_deref()),
            "decoder" => s.and_then(|s| s.decoder.as_deref()),
            "joiner" => s.and_then(|s| s.joiner.as_deref()),
            "tokens" => s.and_then(|s| s.tokens.as_deref()),
            _ => None,
        };
        // 未显式配置时按模型目录内容探测默认文件名（外部导入/手工放置的目录
        // 可能不叫默认名；显式 settings 覆盖优先，与 KWS resolve 语义一致）
        let detected = if value.is_none() {
            detect_default_name(field, &cfg.model_dir, default_name)
        } else {
            default_name.to_string()
        };
        resolve_file(value, &detected, &cfg.model_dir)
    };

    // 引擎后端来源：settings 显式配置 > 缺省 Sherpa（老配置无字段 → 零行为变化）
    cfg.backend = match s.and_then(|s| s.backend.as_deref()) {
        Some(v) => AsrBackendKind::parse_str(v)
            .ok_or_else(|| format!("未知 ASR 后端: {v}（支持 sherpa / audiocpp）"))?,
        None => AsrBackendKind::default(),
    };

    // 模型类型来源：settings 显式配置 >（audiocpp 后端时）GGUF 文件名探测 >
    // 目录内容探测（老用户无字段 → Zipformer，零行为变化）。GGUF 探针仅在
    // backend=audiocpp 时介入，不污染 sherpa 的 ONNX 探测语义。
    let kind = s.and_then(|s| s.model_type).unwrap_or_else(|| {
        if cfg.backend == AsrBackendKind::Audiocpp
            && crate::audiocpp::asr_families::detect_gguf_in_dir(&cfg.model_dir).is_some()
        {
            AsrModelKind::Qwen3Asr
        } else {
            detect_kind_from_dir(&cfg.model_dir)
        }
    });
    cfg.model_type = kind;

    // 非法组合 fail-fast：audiocpp 后端当前仅 qwen3_asr 可查（asr_families 表）
    if cfg.backend == AsrBackendKind::Audiocpp
        && crate::audiocpp::asr_families::asr_family_desc(kind).is_none()
    {
        return Err(format!(
            "模型类型 {} 不支持 audiocpp 后端（请检查 [asr].model_type 与 backend 组合）",
            kind.as_str()
        ));
    }
    cfg.language = s.and_then(|s| s.language.clone());
    cfg.use_itn = s.and_then(|s| s.use_itn);

    match kind {
        AsrModelKind::Zipformer => {
            cfg.encoder = file("encoder", DEFAULT_ENCODER)?;
            cfg.decoder = file("decoder", DEFAULT_DECODER)?;
            cfg.joiner = file("joiner", DEFAULT_JOINER)?;
            cfg.tokens = file("tokens", DEFAULT_TOKENS)?;
        }
        AsrModelKind::Paraformer => {
            // 裸名探测（int8 偏好）；不能用 `file()` 闭包——其内部走 `{prefix}-` 前缀
            // 探测，对裸名不命中。joiner 不消费，保留默认常量。
            let enc = detect_bare_int8_onnx(&cfg.model_dir, "encoder");
            let dec = detect_bare_int8_onnx(&cfg.model_dir, "decoder");
            cfg.encoder = resolve_file(s.and_then(|s| s.encoder.as_deref()), &enc, &cfg.model_dir)?;
            cfg.decoder = resolve_file(s.and_then(|s| s.decoder.as_deref()), &dec, &cfg.model_dir)?;
            cfg.tokens = file("tokens", DEFAULT_TOKENS)?;
        }
        AsrModelKind::SenseVoice => {
            // 主模型探测 `model*.onnx`（回退默认常量名）；encoder/decoder/joiner 不消费
            let detected =
                detect_model_onnx(&cfg.model_dir).unwrap_or_else(|| SENSEVOICE_MODEL.to_string());
            cfg.model = Some(cfg.model_dir.join(detected));
            cfg.tokens = file("tokens", DEFAULT_TOKENS)?;
        }
        AsrModelKind::Whisper => {
            // 尺寸前缀探测（tiny/base/...），回退 "tiny"；settings 显式文件名覆盖优先
            let prefix =
                detect_whisper_prefix(&cfg.model_dir).unwrap_or_else(|| "tiny".to_string());
            cfg.encoder = file("encoder", &format!("{prefix}-encoder.onnx"))?;
            cfg.decoder = file("decoder", &format!("{prefix}-decoder.onnx"))?;
            cfg.tokens = file("tokens", &format!("{prefix}-tokens.txt"))?;
        }
        AsrModelKind::Qwen3Asr => {
            if cfg.backend == AsrBackendKind::Audiocpp {
                // audiocpp 后端：GGUF 定位由 asr_families 表完成（model_dir + gguf_file），
                // 不消费 ONNX 文件字段（对齐 tts resolve 的 audiocpp 族取舍：
                // encoder/decoder/tokens 保留默认值但不参与引擎构造）
                cfg.model = None;
            } else {
                // 裸名 int8 探测与 paraformer 同款；conv_frontend 固定名不提供覆盖
                // （包内无 int8 变体，同 SenseVoice 主模型取舍）。tokens 字段承载
                // tokenizer 目录（settings.tokens 可覆盖目录名），不能用 `file()` 闭包
                // ——那是 `{prefix}-` 文件名探测。
                cfg.model = Some(cfg.model_dir.join(QWEN3_CONV_FRONTEND));
                let enc = detect_bare_int8_onnx(&cfg.model_dir, "encoder");
                let dec = detect_bare_int8_onnx(&cfg.model_dir, "decoder");
                cfg.encoder =
                    resolve_file(s.and_then(|s| s.encoder.as_deref()), &enc, &cfg.model_dir)?;
                cfg.decoder =
                    resolve_file(s.and_then(|s| s.decoder.as_deref()), &dec, &cfg.model_dir)?;
                cfg.tokens = resolve_file(
                    s.and_then(|s| s.tokens.as_deref()),
                    QWEN3_TOKENIZER_DIR,
                    &cfg.model_dir,
                )?;
            }
        }
    }

    cfg.enabled = s.and_then(|s| s.enabled).unwrap_or(false);
    // 推理设备：用户显式配置优先；缺省时 audiocpp 后端按模型族取默认（metal）
    cfg.provider = s.and_then(|s| s.provider.clone()).unwrap_or_else(|| {
        match cfg.backend {
            AsrBackendKind::Audiocpp => {
                crate::audiocpp::asr_families::asr_family_desc(cfg.model_type)
                    .map(|d| d.default_provider)
                    .unwrap_or("cpu")
            }
            AsrBackendKind::Sherpa => "cpu",
        }
        .to_string()
    });
    cfg.engine_path = match s.and_then(|s| s.engine_path.as_deref()) {
        Some(v) => Some(PathBuf::from(resolve_env_ref(v)?)),
        None => None,
    };
    cfg.num_threads = s.and_then(|s| s.num_threads).unwrap_or(2);
    cfg.chunk_size = s.and_then(|s| s.chunk_size).unwrap_or(3200);
    cfg.sample_rate = s.and_then(|s| s.sample_rate).unwrap_or(16000);
    cfg.decoding_method = s
        .and_then(|s| s.decoding_method.clone())
        .unwrap_or_else(|| "greedy_search".to_string());
    cfg.enable_endpoint = s.and_then(|s| s.enable_endpoint).unwrap_or(true);
    cfg.rule1_min_trailing_silence = s.and_then(|s| s.rule1_min_trailing_silence).unwrap_or(2.4);
    cfg.rule2_min_trailing_silence = s.and_then(|s| s.rule2_min_trailing_silence).unwrap_or(1.2);
    cfg.rule3_min_utterance_length = s.and_then(|s| s.rule3_min_utterance_length).unwrap_or(20.0);
    cfg.blank_penalty = s.and_then(|s| s.blank_penalty).unwrap_or(0.0);
    cfg.hotwords = s.and_then(|s| s.hotwords.clone());
    cfg.enable_punctuation = s.and_then(|s| s.enable_punctuation).unwrap_or(true);
    cfg.punctuation_model = match s.and_then(|s| s.punctuation_model.as_deref()) {
        Some(v) => {
            let expanded = resolve_env_ref(v)?;
            let p = PathBuf::from(&expanded);
            if p.is_absolute() {
                p
            } else {
                // 相对路径锚定到标点模型目录（含旧根兜底）
                punctuation_default_dir().join(p)
            }
        }
        None => punctuation_default_dir().join(DEFAULT_PUNCT_MODEL),
    };
    cfg.debug = s.and_then(|s| s.debug).unwrap_or(false);

    Ok(cfg)
}

/// `set_asr_params` 载荷：可调整的 ASR 引擎/运行参数（snake_case 直传，缺省项不修改）。
///
/// 与 Tauri crate 的 `KwsParamsPatch` 对称，但放在 lib crate 内以便 `cargo test` 单测。
/// 引擎参数在 `start_asr_listen` 时固化：保存后需重启识别才生效（由前端处理）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AsrParamsPatch {
    pub num_threads: Option<i32>,
    pub chunk_size: Option<usize>,
    pub enable_endpoint: Option<bool>,
    pub rule1_min_trailing_silence: Option<f32>,
    pub rule2_min_trailing_silence: Option<f32>,
    pub rule3_min_utterance_length: Option<f32>,
    pub blank_penalty: Option<f32>,
    pub hotwords: Option<String>,
    pub enable_punctuation: Option<bool>,
    /// 转写语言（SenseVoice/Whisper；空串清空为自动检测）
    pub language: Option<String>,
    /// SenseVoice 反向文本正则化
    pub use_itn: Option<bool>,
    pub debug: Option<bool>,
}

impl AsrParamsPatch {
    /// 先整体校验（任一越界立即 Err），再逐项写入 `AsrSettings`，保证出错时不部分修改。
    pub fn apply_to(&self, asr: &mut AsrSettings) -> Result<(), String> {
        if let Some(v) = self.num_threads
            && !(1..=32).contains(&v)
        {
            return Err(format!("线程数需在 1~32，当前 {v}"));
        }
        if let Some(v) = self.chunk_size
            && !(400..=16_000).contains(&v)
        {
            return Err(format!("采样块大小需在 400~16000（@16k），当前 {v}"));
        }
        if let Some(v) = self.rule1_min_trailing_silence
            && !(0.0..=10.0).contains(&v)
        {
            return Err(format!("端点规则1尾随静音需在 0~10 秒，当前 {v}"));
        }
        if let Some(v) = self.rule2_min_trailing_silence
            && !(0.0..=10.0).contains(&v)
        {
            return Err(format!("端点规则2尾随静音需在 0~10 秒，当前 {v}"));
        }
        if let Some(v) = self.rule3_min_utterance_length
            && !(5.0..=60.0).contains(&v)
        {
            return Err(format!("端点最大句长需在 5~60 秒，当前 {v}"));
        }
        if let Some(v) = self.blank_penalty
            && !(0.0..=2.0).contains(&v)
        {
            return Err(format!("空白符惩罚需在 0~2，当前 {v}"));
        }

        // 族专属参数不落盘（防止切换后残留无意义配置；引擎层另有一级忽略）：
        // - blank_penalty / hotwords 为 transducer 专属，Paraformer（仅 greedy_search）跳过
        // - blank_penalty：Qwen3-ASR（LLM 解码）同样跳过；hotwords 放行
        //   （qwen3 是离线族中唯一支持热词的，build 层转逗号格式嵌 prompt）
        // - language/use_itn 为 SenseVoice/Whisper 概念，Qwen3-ASR
        //   （29 语言自动识别 + 原生标点）跳过
        // - audiocpp 后端：hotwords 跳过（audio.cpp qwen3_asr 无 hotwords 选项，
        //   前端已隐藏，这里兜底）；language 放行（映射请求 language，量化下
        //   auto 语种识别不可靠时的显式兜底，上游文档明示）
        let paraformer = asr.model_type == Some(AsrModelKind::Paraformer);
        let qwen3 = asr.model_type == Some(AsrModelKind::Qwen3Asr);
        let audiocpp = asr.backend.as_deref() == Some(AsrBackendKind::Audiocpp.as_str());

        if let Some(v) = self.num_threads {
            asr.num_threads = Some(v);
        }
        if let Some(v) = self.chunk_size {
            asr.chunk_size = Some(v);
        }
        if let Some(v) = self.enable_endpoint {
            asr.enable_endpoint = Some(v);
        }
        if let Some(v) = self.rule1_min_trailing_silence {
            asr.rule1_min_trailing_silence = Some(v);
        }
        if let Some(v) = self.rule2_min_trailing_silence {
            asr.rule2_min_trailing_silence = Some(v);
        }
        if let Some(v) = self.rule3_min_utterance_length {
            asr.rule3_min_utterance_length = Some(v);
        }
        if let Some(v) = self.blank_penalty
            && !(paraformer || qwen3)
        {
            asr.blank_penalty = Some(v);
        }
        if let Some(v) = &self.hotwords
            && !(paraformer || audiocpp)
        {
            asr.hotwords = if v.trim().is_empty() {
                None
            } else {
                Some(v.trim().to_string())
            };
        }
        if let Some(v) = self.enable_punctuation {
            asr.enable_punctuation = Some(v);
        }
        if let Some(v) = &self.language
            && !(qwen3 && !audiocpp)
        {
            asr.language = if v.trim().is_empty() {
                None
            } else {
                Some(v.trim().to_string())
            };
        }
        if let Some(v) = self.use_itn
            && !qwen3
        {
            asr.use_itn = Some(v);
        }
        if let Some(v) = self.debug {
            asr.debug = Some(v);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AsrSettings;
    use crate::test_util::run_with_temp_home;

    #[test]
    fn test_default_model_dir_dual_root_fallback() {
        run_with_temp_home(|home| {
            crate::test_util::set_custom_data_dir(home);
            let new_dir = user_default_model_dir();
            let legacy_dir = home
                .join(".zapmomo")
                .join("models")
                .join(new_dir.file_name().unwrap());

            for d in [&new_dir, &legacy_dir] {
                std::fs::create_dir_all(d).unwrap();
                std::fs::write(d.join(DEFAULT_TOKENS), b"t").unwrap();
            }
            assert_eq!(default_model_dir(), new_dir);

            std::fs::remove_dir_all(&new_dir).unwrap();
            assert_eq!(default_model_dir(), legacy_dir);

            std::fs::remove_dir_all(&legacy_dir).unwrap();
            assert_ne!(default_model_dir(), legacy_dir);
        });
    }

    #[test]
    fn test_punctuation_default_dir_dual_root_fallback() {
        run_with_temp_home(|home| {
            crate::test_util::set_custom_data_dir(home);
            let new_punct = crate::kws::model::punctuation_user_model_dir();
            let legacy_punct = home
                .join(".zapmomo")
                .join("models")
                .join(new_punct.file_name().unwrap());

            // 只有旧根 → 默认标点模型指向旧根
            std::fs::create_dir_all(&legacy_punct).unwrap();
            std::fs::write(legacy_punct.join(DEFAULT_PUNCT_MODEL), b"x").unwrap();
            let cfg = resolve(None, None).unwrap();
            assert_eq!(
                cfg.punctuation_model,
                legacy_punct.join(DEFAULT_PUNCT_MODEL)
            );

            // 新根装好后切到新根
            std::fs::create_dir_all(&new_punct).unwrap();
            std::fs::write(new_punct.join(DEFAULT_PUNCT_MODEL), b"x").unwrap();
            let cfg2 = resolve(None, None).unwrap();
            assert_eq!(cfg2.punctuation_model, new_punct.join(DEFAULT_PUNCT_MODEL));
        });
    }

    #[test]
    fn test_default_config_points_to_default_model_dir() {
        let cfg = ResolvedAsrConfig::default();
        assert_eq!(
            cfg.model_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string()),
            Some(crate::kws::model::asr_asset().name.clone())
        );
        assert_eq!(cfg.encoder.file_name().unwrap(), DEFAULT_ENCODER);
        assert_eq!(cfg.decoder.file_name().unwrap(), DEFAULT_DECODER);
        assert_eq!(cfg.joiner.file_name().unwrap(), DEFAULT_JOINER);
        assert_eq!(cfg.tokens.file_name().unwrap(), DEFAULT_TOKENS);
        assert_eq!(cfg.sample_rate, 16000);
        assert_eq!(cfg.chunk_size, 3200);
        assert!(cfg.enable_endpoint);
        assert_eq!(cfg.decoding_method, "greedy_search");
    }

    #[test]
    fn test_user_default_model_dir() {
        run_with_temp_home(|home| {
            let dir = super::user_default_model_dir();
            assert_eq!(
                dir,
                home.join(".zapmomo/models")
                    .join(crate::kws::model::asr_asset().name.as_str())
            );
        });
    }

    #[test]
    fn test_choose_default_model_dir_priority() {
        let base = tempfile::tempdir().unwrap();
        let user = base.path().join("user-model");
        let repo = base.path().join("repo-model");

        assert_eq!(choose_default_model_dir(&user, None, &repo), user);

        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(choose_default_model_dir(&user, None, &repo), repo);

        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(choose_default_model_dir(&user, None, &repo), user);

        std::fs::remove_file(user.join(DEFAULT_TOKENS)).unwrap();
        let legacy = base.path().join("legacy-model");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(
            choose_default_model_dir(&user, Some(&legacy), &repo),
            legacy
        );
    }

    #[test]
    fn test_resolve_relative_model_dir_anchored_to_user_dir() {
        run_with_temp_home(|home| {
            let settings = AsrSettings {
                model_dir: Some("models/my-asr".to_string()),
                ..AsrSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.model_dir, home.join(".zapmomo/models/my-asr"));
        });
    }

    #[test]
    fn test_resolve_no_settings_uses_defaults() {
        // 用临时 HOME 隔离，避免与其它 `run_with_temp_home` 测试并行时 HOME 竞态
        // 导致 `resolve` 与 `ResolvedAsrConfig::default` 两次读取到不同 HOME。
        run_with_temp_home(|_| {
            let cfg = resolve(None, None).unwrap();
            assert_eq!(cfg, ResolvedAsrConfig::default());
        });
    }

    #[test]
    fn test_resolve_enabled_default_false_and_override() {
        run_with_temp_home(|_| {
            // 缺省 enabled=false
            assert!(!resolve(None, None).unwrap().enabled);
            // settings 显式启用
            let settings = AsrSettings {
                enabled: Some(true),
                ..Default::default()
            };
            assert!(resolve(Some(&settings), None).unwrap().enabled);
        });
    }

    fn abs_path(rel: &str) -> PathBuf {
        std::path::absolute(rel).unwrap()
    }

    #[test]
    fn test_resolve_cli_model_dir_overrides_settings() {
        let settings = AsrSettings {
            model_dir: Some("settings-model".to_string()),
            ..AsrSettings::default()
        };
        let cli = abs_path("tmp/cli-asr");
        let cfg = resolve(Some(&settings), Some(&cli)).unwrap();
        assert_eq!(cfg.model_dir, cli);
        assert_eq!(cfg.encoder.parent().unwrap(), cli);
    }

    #[test]
    fn test_resolve_settings_model_dir() {
        let dir = abs_path("opt/asr");
        let settings = AsrSettings {
            model_dir: Some(dir.to_string_lossy().to_string()),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_dir, dir);
        assert_eq!(cfg.encoder, dir.join(DEFAULT_ENCODER));
        assert_eq!(cfg.decoder, dir.join(DEFAULT_DECODER));
        assert_eq!(cfg.joiner, dir.join(DEFAULT_JOINER));
    }

    #[test]
    fn test_resolve_numeric_overrides() {
        let settings = AsrSettings {
            num_threads: Some(4),
            chunk_size: Some(1600),
            enable_endpoint: Some(false),
            rule1_min_trailing_silence: Some(3.0),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.num_threads, 4);
        assert_eq!(cfg.chunk_size, 1600);
        assert!(!cfg.enable_endpoint);
        assert_eq!(cfg.rule1_min_trailing_silence, 3.0);
    }

    #[test]
    fn test_default_punctuation_and_hotwords() {
        let cfg = ResolvedAsrConfig::default();
        assert_eq!(cfg.hotwords, None);
        assert!(cfg.enable_punctuation);
        assert_eq!(
            cfg.punctuation_model,
            crate::kws::model::punctuation_user_model_dir().join(DEFAULT_PUNCT_MODEL)
        );
    }

    #[test]
    fn test_resolve_hotwords_and_punctuation_overrides() {
        let settings = AsrSettings {
            hotwords: Some("你好小智 文森特卡索".to_string()),
            enable_punctuation: Some(false),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.hotwords, Some("你好小智 文森特卡索".to_string()));
        assert!(!cfg.enable_punctuation);
    }

    #[test]
    fn test_asr_params_patch_applies_valid_values() {
        let patch = AsrParamsPatch {
            num_threads: Some(8),
            chunk_size: Some(1600),
            enable_endpoint: Some(false),
            rule1_min_trailing_silence: Some(3.0),
            rule2_min_trailing_silence: Some(1.5),
            rule3_min_utterance_length: Some(30.0),
            blank_penalty: Some(0.5),
            hotwords: Some("你好小智 文森特卡索".to_string()),
            enable_punctuation: Some(false),
            language: Some("zh".to_string()),
            use_itn: Some(true),
            debug: Some(true),
        };
        let mut asr = AsrSettings::default();
        patch.apply_to(&mut asr).unwrap();
        assert_eq!(asr.num_threads, Some(8));
        assert_eq!(asr.chunk_size, Some(1600));
        assert_eq!(asr.enable_endpoint, Some(false));
        assert_eq!(asr.rule1_min_trailing_silence, Some(3.0));
        assert_eq!(asr.rule2_min_trailing_silence, Some(1.5));
        assert_eq!(asr.rule3_min_utterance_length, Some(30.0));
        assert_eq!(asr.blank_penalty, Some(0.5));
        assert_eq!(asr.hotwords, Some("你好小智 文森特卡索".to_string()));
        assert_eq!(asr.enable_punctuation, Some(false));
        assert_eq!(asr.language, Some("zh".to_string()));
        assert_eq!(asr.use_itn, Some(true));
        assert_eq!(asr.debug, Some(true));
    }

    #[test]
    fn test_asr_params_patch_rejects_out_of_range() {
        let cases: &[(&str, AsrParamsPatch)] = &[
            (
                "线程数",
                AsrParamsPatch {
                    num_threads: Some(0),
                    ..Default::default()
                },
            ),
            (
                "线程数",
                AsrParamsPatch {
                    num_threads: Some(33),
                    ..Default::default()
                },
            ),
            (
                "采样块大小",
                AsrParamsPatch {
                    chunk_size: Some(399),
                    ..Default::default()
                },
            ),
            (
                "采样块大小",
                AsrParamsPatch {
                    chunk_size: Some(16_001),
                    ..Default::default()
                },
            ),
            (
                "规则1",
                AsrParamsPatch {
                    rule1_min_trailing_silence: Some(-0.1),
                    ..Default::default()
                },
            ),
            (
                "规则2",
                AsrParamsPatch {
                    rule2_min_trailing_silence: Some(10.1),
                    ..Default::default()
                },
            ),
            (
                "最大句长",
                AsrParamsPatch {
                    rule3_min_utterance_length: Some(4.9),
                    ..Default::default()
                },
            ),
            (
                "空白符",
                AsrParamsPatch {
                    blank_penalty: Some(2.1),
                    ..Default::default()
                },
            ),
        ];
        for (label, patch) in cases {
            let mut asr = AsrSettings::default();
            let err = patch.apply_to(&mut asr).unwrap_err();
            assert!(
                err.contains(label),
                "参数「{label}」应被拒绝，实际错误: {err}"
            );
        }
    }

    #[test]
    fn test_asr_params_patch_all_or_nothing() {
        let mut asr = AsrSettings {
            num_threads: Some(4),
            ..AsrSettings::default()
        };
        let patch = AsrParamsPatch {
            num_threads: Some(16),
            chunk_size: Some(50_000), // 非法
            ..Default::default()
        };
        let err = patch.apply_to(&mut asr).unwrap_err();
        assert!(err.contains("采样块大小"));
        // 校验失败 → num_threads 未被写入（部分修改被阻止）
        assert_eq!(asr.num_threads, Some(4));
    }

    #[test]
    fn test_asr_params_patch_hotwords_empty_becomes_none() {
        let mut asr = AsrSettings::default();
        let patch = AsrParamsPatch {
            hotwords: Some("  ".to_string()),
            ..Default::default()
        };
        patch.apply_to(&mut asr).unwrap();
        assert_eq!(asr.hotwords, None);

        let mut asr2 = AsrSettings::default();
        let patch2 = AsrParamsPatch {
            hotwords: Some(" 你好  ".to_string()),
            ..Default::default()
        };
        patch2.apply_to(&mut asr2).unwrap();
        assert_eq!(asr2.hotwords, Some("你好".to_string()));
    }

    /// 双语布局（int8+fp32 混放，即全部注册包的实际布局）→ 常量直接命中，零行为变化。
    #[test]
    fn test_detect_asr_default_constants_win() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "encoder-epoch-99-avg-1.int8.onnx",
            "encoder-epoch-99-avg-1.onnx",
            "decoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            "joiner-epoch-99-avg-1.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            detect_default_onnx(dir.path(), "encoder", DEFAULT_ENCODER, true),
            DEFAULT_ENCODER
        );
        assert_eq!(
            detect_default_onnx(dir.path(), "decoder", DEFAULT_DECODER, false),
            DEFAULT_DECODER
        );
        assert_eq!(
            detect_default_onnx(dir.path(), "joiner", DEFAULT_JOINER, true),
            DEFAULT_JOINER
        );
        assert!(asr_files_present(dir.path()));
    }

    /// 非默认名混放（epoch-20 系，外部导入场景）→ encoder/joiner 取 int8、decoder 取 fp32。
    #[test]
    fn test_detect_asr_prefers_int8_encoder_joiner_fp32_decoder() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            "encoder-epoch-20-avg-1-chunk-16-left-64.onnx",
            "decoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            "decoder-epoch-20-avg-1-chunk-16-left-64.onnx",
            "joiner-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            "joiner-epoch-20-avg-1-chunk-16-left-64.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            detect_default_onnx(dir.path(), "encoder", DEFAULT_ENCODER, true),
            "encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx"
        );
        assert_eq!(
            detect_default_onnx(dir.path(), "decoder", DEFAULT_DECODER, false),
            "decoder-epoch-20-avg-1-chunk-16-left-64.onnx"
        );
        assert_eq!(
            detect_default_onnx(dir.path(), "joiner", DEFAULT_JOINER, true),
            "joiner-epoch-20-avg-1-chunk-16-left-64.int8.onnx"
        );
        assert!(asr_files_present(dir.path()));
    }

    /// 单变体目录回退：int8-only 的 decoder 取 int8；fp32-only 的 encoder 取 fp32。
    #[test]
    fn test_detect_asr_fallback_to_any_variant() {
        let int8_only = tempfile::tempdir().unwrap();
        std::fs::write(
            int8_only.path().join("decoder-epoch-20-avg-1.int8.onnx"),
            b"x",
        )
        .unwrap();
        assert_eq!(
            detect_default_onnx(int8_only.path(), "decoder", DEFAULT_DECODER, false),
            "decoder-epoch-20-avg-1.int8.onnx"
        );

        let fp32_only = tempfile::tempdir().unwrap();
        std::fs::write(fp32_only.path().join("encoder-epoch-20-avg-1.onnx"), b"x").unwrap();
        assert_eq!(
            detect_default_onnx(fp32_only.path(), "encoder", DEFAULT_ENCODER, true),
            "encoder-epoch-20-avg-1.onnx"
        );
    }

    /// 目录不存在 / 空目录 / 无 onnx 候选 → 回退默认常量名。
    #[test]
    fn test_detect_asr_missing_or_empty_dir_falls_back_to_constant() {
        assert_eq!(
            detect_default_onnx(
                Path::new("/nonexistent-asr"),
                "encoder",
                DEFAULT_ENCODER,
                true
            ),
            DEFAULT_ENCODER
        );
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            detect_default_onnx(empty.path(), "encoder", DEFAULT_ENCODER, true),
            DEFAULT_ENCODER
        );
        // 前缀不匹配的 onnx 不算候选
        std::fs::write(empty.path().join("other-model.onnx"), b"x").unwrap();
        assert_eq!(
            detect_default_onnx(empty.path(), "encoder", DEFAULT_ENCODER, true),
            DEFAULT_ENCODER
        );
    }

    /// settings 只给 model_dir（切换模型的写法）+ 非默认命名目录 → resolve 命中探测名。
    #[test]
    fn test_resolve_detects_non_default_layout() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            "decoder-epoch-20-avg-1-chunk-16-left-64.onnx",
            "joiner-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(
            cfg.encoder,
            dir.path()
                .join("encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx")
        );
        assert_eq!(
            cfg.decoder,
            dir.path()
                .join("decoder-epoch-20-avg-1-chunk-16-left-64.onnx")
        );
        assert_eq!(
            cfg.joiner,
            dir.path()
                .join("joiner-epoch-20-avg-1-chunk-16-left-64.int8.onnx")
        );
        assert_eq!(cfg.tokens, dir.path().join(DEFAULT_TOKENS));
    }

    /// 显式 settings 文件覆盖优先，不探测（与 KWS resolve 语义一致）。
    #[test]
    fn test_resolve_explicit_file_overrides_skip_probe() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            encoder: Some("custom-encoder.onnx".to_string()),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(
            cfg.encoder,
            dir.path().join("custom-encoder.onnx"),
            "显式覆盖应直连，不被目录内文件影响"
        );
    }

    /// 探测式完整性判定：完整 true / 缺任一 false / 空目录 false / 不存在 false。
    #[test]
    fn test_asr_files_present() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!asr_files_present(dir.path()));
        assert!(!asr_files_present(Path::new("/nonexistent-asr")));

        for name in [
            "encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            "decoder-epoch-20-avg-1-chunk-16-left-64.onnx",
            "joiner-epoch-20-avg-1-chunk-16-left-64.int8.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert!(asr_files_present(dir.path()));

        std::fs::remove_file(dir.path().join(DEFAULT_TOKENS)).unwrap();
        assert!(!asr_files_present(dir.path()), "缺 tokens 应为 false");

        std::fs::write(dir.path().join(DEFAULT_TOKENS), b"x").unwrap();
        std::fs::remove_file(
            dir.path()
                .join("encoder-epoch-20-avg-1-chunk-16-left-64.int8.onnx"),
        )
        .unwrap();
        assert!(!asr_files_present(dir.path()), "缺 encoder 应为 false");
    }

    // ---- AsrModelKind / 多族探测与解析 ----

    #[test]
    fn test_model_kind_str_and_semantics() {
        for (kind, s) in [
            (AsrModelKind::Zipformer, "zipformer"),
            (AsrModelKind::Paraformer, "paraformer"),
            (AsrModelKind::SenseVoice, "sensevoice"),
            (AsrModelKind::Whisper, "whisper"),
            (AsrModelKind::Qwen3Asr, "qwen3_asr"),
        ] {
            assert_eq!(kind.as_str(), s);
            assert_eq!(AsrModelKind::parse_str(s), Some(kind));
        }
        assert_eq!(AsrModelKind::parse_str("unknown"), None);
        assert!(AsrModelKind::Zipformer.is_streaming());
        assert!(!AsrModelKind::Zipformer.is_offline());
        assert!(AsrModelKind::Paraformer.is_streaming());
        assert!(!AsrModelKind::Paraformer.is_offline());
        assert!(!AsrModelKind::SenseVoice.is_streaming());
        assert!(AsrModelKind::SenseVoice.is_offline());
        assert!(!AsrModelKind::Whisper.is_streaming());
        assert!(AsrModelKind::Whisper.is_offline());
        assert!(!AsrModelKind::Qwen3Asr.is_streaming());
        assert!(AsrModelKind::Qwen3Asr.is_offline());
        // serde snake_case 往返
        assert_eq!(
            serde_json::from_str::<AsrModelKind>("\"sensevoice\"").unwrap(),
            AsrModelKind::SenseVoice
        );
        assert_eq!(
            serde_json::from_str::<AsrModelKind>("\"paraformer\"").unwrap(),
            AsrModelKind::Paraformer
        );
        assert_eq!(
            serde_json::from_str::<AsrModelKind>("\"qwen3_asr\"").unwrap(),
            AsrModelKind::Qwen3Asr
        );
        assert_eq!(
            serde_json::to_string(&AsrModelKind::Qwen3Asr).unwrap(),
            "\"qwen3_asr\""
        );
        assert_eq!(
            serde_json::to_string(&AsrModelKind::Whisper).unwrap(),
            "\"whisper\""
        );
    }

    #[test]
    fn test_required_files_by_kind() {
        assert_eq!(required_files(AsrModelKind::Zipformer).len(), 4);
        assert_eq!(
            required_files(AsrModelKind::Paraformer),
            &[PARAFORMER_ENCODER, PARAFORMER_DECODER, DEFAULT_TOKENS]
        );
        assert_eq!(
            required_files(AsrModelKind::SenseVoice),
            &[SENSEVOICE_MODEL, DEFAULT_TOKENS]
        );
        // whisper 因 tiny/base 文件名不同，安装清单按 role 精确声明，此处运行时返回空（探测兜底）
        assert!(required_files(AsrModelKind::Whisper).is_empty());
        assert_eq!(
            required_files(AsrModelKind::Qwen3Asr),
            &[
                QWEN3_CONV_FRONTEND,
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "tokenizer/vocab.json",
                "tokenizer/merges.txt",
                "tokenizer/tokenizer_config.json",
            ]
        );
    }

    #[test]
    fn test_detect_kind_from_dir_probes() {
        // SenseVoice：model.onnx + tokens.txt
        let sense = tempfile::tempdir().unwrap();
        std::fs::write(sense.path().join("model.onnx"), b"x").unwrap();
        std::fs::write(sense.path().join(DEFAULT_TOKENS), b"x").unwrap();
        assert_eq!(detect_kind_from_dir(sense.path()), AsrModelKind::SenseVoice);

        // Whisper：*-encoder.onnx + *-decoder.onnx + *-tokens.txt
        let whisper = tempfile::tempdir().unwrap();
        for name in ["tiny-encoder.onnx", "tiny-decoder.onnx", "tiny-tokens.txt"] {
            std::fs::write(whisper.path().join(name), b"x").unwrap();
        }
        assert_eq!(detect_kind_from_dir(whisper.path()), AsrModelKind::Whisper);

        // Paraformer：裸名 encoder/decoder（int8）+ tokens
        let para_int8 = tempfile::tempdir().unwrap();
        for name in [PARAFORMER_ENCODER, PARAFORMER_DECODER, DEFAULT_TOKENS] {
            std::fs::write(para_int8.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            detect_kind_from_dir(para_int8.path()),
            AsrModelKind::Paraformer
        );

        // Paraformer：fp32-only 目录同样可判（外部导出场景）
        let para_fp32 = tempfile::tempdir().unwrap();
        for name in ["encoder.onnx", "decoder.onnx", DEFAULT_TOKENS] {
            std::fs::write(para_fp32.path().join(name), b"x").unwrap();
        }
        assert_eq!(
            detect_kind_from_dir(para_fp32.path()),
            AsrModelKind::Paraformer
        );

        // Zipformer：四件套
        let zip = tempfile::tempdir().unwrap();
        for name in [
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            DEFAULT_TOKENS,
        ] {
            std::fs::write(zip.path().join(name), b"x").unwrap();
        }
        assert_eq!(detect_kind_from_dir(zip.path()), AsrModelKind::Zipformer);

        // 空目录 → Zipformer 兜底
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(detect_kind_from_dir(empty.path()), AsrModelKind::Zipformer);
    }

    #[test]
    fn test_resolve_sensevoice_sets_model_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"x").unwrap();
        std::fs::write(dir.path().join(DEFAULT_TOKENS), b"x").unwrap();
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::SenseVoice),
            language: Some("zh".to_string()),
            use_itn: Some(true),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_type, AsrModelKind::SenseVoice);
        assert_eq!(
            cfg.model.as_deref(),
            Some(dir.path().join("model.onnx").as_path())
        );
        assert_eq!(cfg.tokens, dir.path().join(DEFAULT_TOKENS));
        assert_eq!(cfg.language.as_deref(), Some("zh"));
        assert_eq!(cfg.use_itn, Some(true));
        // 离线族不消费 joiner/encoder/decoder 探测，保留默认常量
        assert_eq!(cfg.encoder.file_name().unwrap(), DEFAULT_ENCODER);
    }

    #[test]
    fn test_resolve_whisper_sets_encoder_decoder_tokens() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["base-encoder.onnx", "base-decoder.onnx", "base-tokens.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Whisper),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_type, AsrModelKind::Whisper);
        assert_eq!(cfg.encoder.file_name().unwrap(), "base-encoder.onnx");
        assert_eq!(cfg.decoder.file_name().unwrap(), "base-decoder.onnx");
        assert_eq!(cfg.tokens.file_name().unwrap(), "base-tokens.txt");
        assert_eq!(cfg.model, None);
    }

    #[test]
    fn test_resolve_whisper_falls_back_tiny_prefix() {
        // 目录无 whisper 文件但显式指定 Whisper → prefix 回退 "tiny"
        let dir = tempfile::tempdir().unwrap();
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Whisper),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.encoder.file_name().unwrap(), "tiny-encoder.onnx");
        assert_eq!(cfg.decoder.file_name().unwrap(), "tiny-decoder.onnx");
        assert_eq!(cfg.tokens.file_name().unwrap(), "tiny-tokens.txt");
    }

    #[test]
    fn test_resolve_paraformer_sets_encoder_decoder_tokens() {
        // int8 文件齐 → 默认消费 int8；joiner 不消费（保留默认常量）
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "encoder.onnx",
            PARAFORMER_ENCODER,
            "decoder.onnx",
            PARAFORMER_DECODER,
            DEFAULT_TOKENS,
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Paraformer),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_type, AsrModelKind::Paraformer);
        assert_eq!(cfg.encoder.file_name().unwrap(), PARAFORMER_ENCODER);
        assert_eq!(cfg.decoder.file_name().unwrap(), PARAFORMER_DECODER);
        assert_eq!(cfg.tokens.file_name().unwrap(), DEFAULT_TOKENS);
        assert_eq!(cfg.joiner.file_name().unwrap(), DEFAULT_JOINER);
        assert_eq!(cfg.model, None);

        // 显式 settings 覆盖优先（fp32 文件名）
        let settings_fp32 = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Paraformer),
            encoder: Some("encoder.onnx".to_string()),
            decoder: Some("decoder.onnx".to_string()),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings_fp32), None).unwrap();
        assert_eq!(cfg.encoder.file_name().unwrap(), "encoder.onnx");
        assert_eq!(cfg.decoder.file_name().unwrap(), "decoder.onnx");
    }

    #[test]
    fn test_asr_files_present_family_aware_paraformer() {
        let dir = tempfile::tempdir().unwrap();
        for name in [PARAFORMER_ENCODER, PARAFORMER_DECODER, DEFAULT_TOKENS] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert!(asr_files_present_for_kind(
            dir.path(),
            AsrModelKind::Paraformer
        ));
        assert!(asr_files_present(dir.path()), "探测应归到 Paraformer");
        std::fs::remove_file(dir.path().join(PARAFORMER_DECODER)).unwrap();
        assert!(!asr_files_present_for_kind(
            dir.path(),
            AsrModelKind::Paraformer
        ));
        // 缺 decoder 后不再像 paraformer → 探测落 Zipformer 兜底，四件套也不齐
        assert!(!asr_files_present(dir.path()));
    }

    #[test]
    fn test_asr_params_patch_skips_transducer_only_fields_for_paraformer() {
        // paraformer 下热词/空白惩罚不落盘；num_threads 等通用项正常写入
        let mut asr = AsrSettings {
            model_type: Some(AsrModelKind::Paraformer),
            hotwords: Some("旧热词".to_string()),
            ..AsrSettings::default()
        };
        let patch = AsrParamsPatch {
            num_threads: Some(4),
            blank_penalty: Some(1.5),
            hotwords: Some("新热词".to_string()),
            ..AsrParamsPatch::default()
        };
        patch.apply_to(&mut asr).unwrap();
        assert_eq!(asr.num_threads, Some(4));
        assert_eq!(asr.hotwords.as_deref(), Some("旧热词"), "热词不应被改写");
        assert_eq!(asr.blank_penalty, None, "空白惩罚不应落盘");

        // zipformer 下行为不变（回归）
        let mut zip = AsrSettings {
            model_type: Some(AsrModelKind::Zipformer),
            ..AsrSettings::default()
        };
        patch.apply_to(&mut zip).unwrap();
        assert_eq!(zip.hotwords.as_deref(), Some("新热词"));
        assert_eq!(zip.blank_penalty, Some(1.5));
    }

    #[test]
    fn test_detect_kind_from_dir_qwen3() {
        // int8 目录：conv_frontend（独有标记）+ 裸名 int8 encoder/decoder + tokenizer 目录
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "conv_frontend.onnx",
            "encoder.int8.onnx",
            "decoder.int8.onnx",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("tokenizer")).unwrap();
        std::fs::write(dir.path().join("tokenizer/vocab.json"), b"x").unwrap();
        assert_eq!(detect_kind_from_dir(dir.path()), AsrModelKind::Qwen3Asr);
        assert!(asr_files_present_for_kind(
            dir.path(),
            AsrModelKind::Qwen3Asr
        ));
        assert!(asr_files_present(dir.path()), "探测应归到 Qwen3-ASR");

        // fp32 裸名目录同判（外部导出场景）
        let fp32 = tempfile::tempdir().unwrap();
        for name in ["conv_frontend.onnx", "encoder.onnx", "decoder.onnx"] {
            std::fs::write(fp32.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(fp32.path().join("tokenizer")).unwrap();
        assert_eq!(detect_kind_from_dir(fp32.path()), AsrModelKind::Qwen3Asr);

        // 删 tokenizer 目录后不再命中，且不误判 paraformer
        // （裸名 encoder/decoder 在，但 paraformer 探针要求 tokens.txt）
        std::fs::remove_dir_all(dir.path().join("tokenizer")).unwrap();
        assert_ne!(detect_kind_from_dir(dir.path()), AsrModelKind::Qwen3Asr);
        assert!(!asr_files_present_for_kind(
            dir.path(),
            AsrModelKind::Qwen3Asr
        ));
    }

    #[test]
    fn test_resolve_qwen3_maps_model_encoder_decoder_tokenizer_dir() {
        // model=conv_frontend、encoder/decoder=裸名 int8、tokens=tokenizer 目录；
        // joiner 不消费保留默认常量（与 SenseVoice/Paraformer 同款取舍）
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "conv_frontend.onnx",
            "encoder.onnx",
            "encoder.int8.onnx",
            "decoder.onnx",
            "decoder.int8.onnx",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("tokenizer")).unwrap();
        let settings = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Qwen3Asr),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_type, AsrModelKind::Qwen3Asr);
        assert_eq!(
            cfg.model.as_deref(),
            Some(dir.path().join("conv_frontend.onnx").as_path())
        );
        assert_eq!(cfg.encoder.file_name().unwrap(), "encoder.int8.onnx");
        assert_eq!(cfg.decoder.file_name().unwrap(), "decoder.int8.onnx");
        assert_eq!(cfg.tokens.file_name().unwrap(), "tokenizer");
        assert!(cfg.tokens.is_dir(), "tokens 承载 tokenizer 目录");
        assert_eq!(cfg.joiner.file_name().unwrap(), DEFAULT_JOINER);

        // 显式 settings 覆盖优先（fp32 文件名）
        let settings_fp32 = AsrSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            model_type: Some(AsrModelKind::Qwen3Asr),
            encoder: Some("encoder.onnx".to_string()),
            decoder: Some("decoder.onnx".to_string()),
            ..AsrSettings::default()
        };
        let cfg = resolve(Some(&settings_fp32), None).unwrap();
        assert_eq!(cfg.encoder.file_name().unwrap(), "encoder.onnx");
        assert_eq!(cfg.decoder.file_name().unwrap(), "decoder.onnx");
    }

    #[test]
    fn test_asr_params_patch_qwen3_rules() {
        // qwen3 下热词落盘（离线族唯一支持，build 层转逗号格式）；
        // blank_penalty/language/use_itn 均非本族概念，不落盘
        let mut asr = AsrSettings {
            model_type: Some(AsrModelKind::Qwen3Asr),
            ..AsrSettings::default()
        };
        let patch = AsrParamsPatch {
            num_threads: Some(4),
            blank_penalty: Some(1.5),
            hotwords: Some("尼日尔河 ZapMomo".to_string()),
            language: Some("zh".to_string()),
            use_itn: Some(true),
            ..AsrParamsPatch::default()
        };
        patch.apply_to(&mut asr).unwrap();
        assert_eq!(asr.num_threads, Some(4));
        assert_eq!(asr.hotwords.as_deref(), Some("尼日尔河 ZapMomo"));
        assert_eq!(asr.blank_penalty, None, "空白惩罚不应落盘");
        assert_eq!(asr.language, None, "qwen3 自动识别语种，language 不落盘");
        assert_eq!(asr.use_itn, None, "qwen3 原生标点，use_itn 不落盘");

        // paraformer/zipformer 回归不变（paraformer 热词仍跳过、language 正常落盘）
        let mut para = AsrSettings {
            model_type: Some(AsrModelKind::Paraformer),
            ..AsrSettings::default()
        };
        patch.apply_to(&mut para).unwrap();
        assert_eq!(para.hotwords, None, "paraformer 热词仍跳过");
        assert_eq!(para.language.as_deref(), Some("zh"));
        let mut zip = AsrSettings {
            model_type: Some(AsrModelKind::Zipformer),
            ..AsrSettings::default()
        };
        patch.apply_to(&mut zip).unwrap();
        assert_eq!(zip.hotwords.as_deref(), Some("尼日尔河 ZapMomo"));
        assert_eq!(zip.blank_penalty, Some(1.5));
        assert_eq!(zip.language.as_deref(), Some("zh"));
    }

    #[test]
    fn test_resolve_zipformer_unchanged_for_legacy() {
        run_with_temp_home(|_| {
            // 老配置（无 model_type 字段）→ Zipformer，行为与现状一致
            let cfg = resolve(None, None).unwrap();
            assert_eq!(cfg.model_type, AsrModelKind::Zipformer);
            assert_eq!(cfg.encoder.file_name().unwrap(), DEFAULT_ENCODER);
            assert_eq!(cfg.decoder.file_name().unwrap(), DEFAULT_DECODER);
            assert_eq!(cfg.joiner.file_name().unwrap(), DEFAULT_JOINER);
            assert_eq!(cfg.tokens.file_name().unwrap(), DEFAULT_TOKENS);
            assert_eq!(cfg.model, None);
        });
    }

    #[test]
    fn test_asr_files_present_family_aware() {
        // SenseVoice 目录
        let sense = tempfile::tempdir().unwrap();
        std::fs::write(sense.path().join("model.onnx"), b"x").unwrap();
        std::fs::write(sense.path().join(DEFAULT_TOKENS), b"x").unwrap();
        assert!(asr_files_present(sense.path()));

        // Whisper 目录
        let whisper = tempfile::tempdir().unwrap();
        for name in ["tiny-encoder.onnx", "tiny-decoder.onnx", "tiny-tokens.txt"] {
            std::fs::write(whisper.path().join(name), b"x").unwrap();
        }
        assert!(asr_files_present(whisper.path()));

        // 缺 tokens → false
        std::fs::remove_file(whisper.path().join("tiny-tokens.txt")).unwrap();
        assert!(!asr_files_present(whisper.path()));

        // 空目录 / 不存在 → false
        assert!(!asr_files_present(tempfile::tempdir().unwrap().path()));
        assert!(!asr_files_present(Path::new("/nonexistent-asr")));
    }

    // ---- AsrBackendKind / audiocpp 后端 ----

    #[test]
    fn test_backend_kind_str_and_semantics() {
        for (kind, s) in [
            (AsrBackendKind::Sherpa, "sherpa"),
            (AsrBackendKind::Audiocpp, "audiocpp"),
        ] {
            assert_eq!(kind.as_str(), s);
            assert_eq!(AsrBackendKind::parse_str(s), Some(kind));
        }
        assert_eq!(AsrBackendKind::parse_str("unknown"), None);
        assert_eq!(AsrBackendKind::default(), AsrBackendKind::Sherpa);
        assert_eq!(
            serde_json::from_str::<AsrBackendKind>("\"audiocpp\"").unwrap(),
            AsrBackendKind::Audiocpp
        );
        assert_eq!(
            serde_json::to_string(&AsrBackendKind::Audiocpp).unwrap(),
            "\"audiocpp\""
        );
    }

    #[test]
    fn test_resolve_backend_default_sherpa_and_explicit() {
        run_with_temp_home(|_| {
            // 缺省 sherpa（老配置无字段 → 零行为变化）
            let cfg = resolve(None, None).unwrap();
            assert_eq!(cfg.backend, AsrBackendKind::Sherpa);
            assert_eq!(cfg.engine_path, None);

            // 显式 audiocpp + qwen3
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path()
                    .join(crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file),
                b"x",
            )
            .unwrap();
            let settings = AsrSettings {
                model_dir: Some(dir.path().to_string_lossy().to_string()),
                backend: Some("audiocpp".to_string()),
                ..AsrSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.backend, AsrBackendKind::Audiocpp);
            assert_eq!(cfg.model_type, AsrModelKind::Qwen3Asr, "GGUF 探测命中");
            assert_eq!(cfg.model, None, "audiocpp 不消费 ONNX 主模型字段");
            assert_eq!(cfg.provider, "metal", "audiocpp 缺省 provider 取族表");

            // 显式 provider 优先
            let settings = AsrSettings {
                model_dir: Some(dir.path().to_string_lossy().to_string()),
                backend: Some("audiocpp".to_string()),
                provider: Some("cpu".to_string()),
                engine_path: Some("/opt/engines/audiocpp_server".to_string()),
                ..AsrSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.provider, "cpu");
            assert_eq!(
                cfg.engine_path.as_deref(),
                Some(Path::new("/opt/engines/audiocpp_server"))
            );
        });
    }

    #[test]
    fn test_resolve_backend_invalid_and_combo_error() {
        run_with_temp_home(|_| {
            // 非法 backend 值
            let settings = AsrSettings {
                backend: Some("mystery".to_string()),
                ..AsrSettings::default()
            };
            let err = resolve(Some(&settings), None).unwrap_err();
            assert!(err.contains("未知 ASR 后端"), "err: {err}");

            // audiocpp + sherpa-only kind 组合报错（fail-fast）
            let settings = AsrSettings {
                backend: Some("audiocpp".to_string()),
                model_type: Some(AsrModelKind::Zipformer),
                ..AsrSettings::default()
            };
            let err = resolve(Some(&settings), None).unwrap_err();
            assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");
        });
    }

    #[test]
    fn test_resolve_gguf_probe_only_under_audiocpp_backend() {
        run_with_temp_home(|_| {
            // 只含 GGUF 的目录：backend 缺省（sherpa）→ 仍落 Zipformer 兜底
            // （GGUF 探针不介入 sherpa 路径，行为零变化）
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path()
                    .join(crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file),
                b"x",
            )
            .unwrap();
            let settings = AsrSettings {
                model_dir: Some(dir.path().to_string_lossy().to_string()),
                ..AsrSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.model_type, AsrModelKind::Zipformer);
        });
    }

    #[test]
    fn test_preflight_and_models_present_audiocpp() {
        run_with_temp_home(|_| {
            let dir = tempfile::tempdir().unwrap();
            let settings = AsrSettings {
                model_dir: Some(dir.path().to_string_lossy().to_string()),
                model_type: Some(AsrModelKind::Qwen3Asr),
                backend: Some("audiocpp".to_string()),
                ..AsrSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            // GGUF 缺失 → preflight 报 registry hint
            assert!(!models_present(&cfg));
            let err = preflight(&cfg).unwrap_err();
            assert!(err.contains("缺少模型文件"), "err: {err}");
            assert!(err.contains("asr-qwen3-0.6b-audiocpp"), "err: {err}");

            // GGUF 就位 → 通过
            std::fs::write(
                dir.path()
                    .join(crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file),
                b"x",
            )
            .unwrap();
            let cfg = resolve(Some(&settings), None).unwrap();
            assert!(models_present(&cfg));
            preflight(&cfg).unwrap();
        });
    }

    #[test]
    fn test_asr_params_patch_audiocpp_rules() {
        // audiocpp 后端：热词不落盘（上游无此能力）；language 放行（映射请求 language）
        let mut asr = AsrSettings {
            model_type: Some(AsrModelKind::Qwen3Asr),
            backend: Some("audiocpp".to_string()),
            ..AsrSettings::default()
        };
        let patch = AsrParamsPatch {
            num_threads: Some(4),
            hotwords: Some("尼日尔河 ZapMomo".to_string()),
            language: Some("zh".to_string()),
            ..AsrParamsPatch::default()
        };
        patch.apply_to(&mut asr).unwrap();
        assert_eq!(asr.num_threads, Some(4));
        assert_eq!(asr.hotwords, None, "audiocpp 后端热词不落盘");
        assert_eq!(
            asr.language.as_deref(),
            Some("zh"),
            "audiocpp 后端 language 放行"
        );

        // sherpa qwen3 回归：热词落盘、language 不落盘（既有行为不变）
        let mut sherpa = AsrSettings {
            model_type: Some(AsrModelKind::Qwen3Asr),
            ..AsrSettings::default()
        };
        patch.apply_to(&mut sherpa).unwrap();
        assert_eq!(sherpa.hotwords.as_deref(), Some("尼日尔河 ZapMomo"));
        assert_eq!(sherpa.language, None);
    }
}
