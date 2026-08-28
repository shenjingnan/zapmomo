use crate::config::settings::{TtsSettings, resolve_env_ref};
/// TTS 配置解析与校验。
///
/// 负责把 `settings.toml` 的 `[tts]` 表与 CLI flag 合并成一份已展开、已填默认值的
/// `ResolvedTtsConfig`。优先级：CLI `--model-dir` > settings > 内置默认。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 模型包内默认文件名（sherpa-onnx 官方 zipvoice distill int8 打包版）。
pub const DEFAULT_ENCODER: &str = "encoder.int8.onnx";
pub const DEFAULT_DECODER: &str = "decoder.int8.onnx";
/// 声码器（独立发布，`tts install-model` 时与主包一并下载）。
pub const DEFAULT_VOCODER: &str = "vocos_24khz.onnx";
pub const DEFAULT_TOKENS: &str = "tokens.txt";
pub const DEFAULT_LEXICON: &str = "lexicon.txt";
/// espeak-ng 数据目录（相对模型目录）。
pub const DEFAULT_DATA_DIR: &str = "espeak-ng-data";
/// 默认参考音频（零样本声音克隆的音色来源）。
pub const DEFAULT_REFERENCE_WAV: &str = "test_wavs/leijun-1.wav";
/// 默认参考音频的逐字转写（来自模型包内 test_wavs/prompt.txt）。
pub const DEFAULT_REFERENCE_TEXT: &str = "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.";

/// ZipVoice 官方示例推荐参数（Rust 的 `Default` 全为 0，需显式设置）。
pub const DEFAULT_FEAT_SCALE: f32 = 0.1;
pub const DEFAULT_T_SHIFT: f32 = 0.5;
pub const DEFAULT_TARGET_RMS: f32 = 0.1;
pub const DEFAULT_GUIDANCE_SCALE: f32 = 1.0;

/// 模型安装完成所需的文件（相对目标目录；espeak-ng-data 目录与参考 wav 由引擎单独校验）。
pub const REQUIRED_FILES: [&str; 5] = [
    DEFAULT_ENCODER,
    DEFAULT_DECODER,
    DEFAULT_VOCODER,
    DEFAULT_TOKENS,
    DEFAULT_LEXICON,
];

/// TTS 模型类型（sherpa-onnx `OfflineTtsModelConfig` 的分支）。
///
/// 全链路显式传递：`[tts].model_type`（持久化）→ `ResolvedTtsConfig.model_type` →
/// `TtsEngine` 构造分支 + 合成参数分支。默认 Zipvoice（零样本声音克隆）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TtsModelKind {
    /// ZipVoice：参考音频零样本声音克隆（当前默认）
    #[default]
    Zipvoice,
    Kitten,
    /// OmniVoice：audio.cpp 后端专用（600+ 语种零样本克隆，Qwen3-0.6B 基座）。
    /// 「audiocpp-only kind」：仅由 `set_selected_model` 权威写入，目录内容不探测
    /// （族差异见 `crate::audiocpp::families` 表）。
    Omnivoice,
    /// VoxCPM2：audio.cpp 后端专用（OpenBMB 2B，48kHz 录音室级 + 30 语种克隆）。
    /// 同款「audiocpp-only kind」语义（见 `Omnivoice` 注释）。
    Voxcpm2,
    /// Qwen3-TTS 0.6B Base：audio.cpp 后端专用（10 语种音色克隆，24kHz）。
    /// 同款「audiocpp-only kind」语义（见 `Omnivoice` 注释）。
    /// Base 版必须提供克隆参考音频（上游无 auto voice）。
    /// 显式 serde rename：派生 snake_case 会得到 `qwen3_tts06`，与 as_str/parse_str 不一致
    #[serde(rename = "qwen3_tts_06")]
    Qwen3Tts06,
    /// Qwen3-TTS 1.7B Base：audio.cpp 后端专用（质量优先变体）。
    #[serde(rename = "qwen3_tts_17")]
    Qwen3Tts17,
    Supertonic,
}

impl TtsModelKind {
    /// snake_case 字符串（配置/JSON 直传）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Zipvoice => "zipvoice",
            Self::Kitten => "kitten",
            Self::Omnivoice => "omnivoice",
            Self::Voxcpm2 => "voxcpm2",
            Self::Qwen3Tts06 => "qwen3_tts_06",
            Self::Qwen3Tts17 => "qwen3_tts_17",
            Self::Supertonic => "supertonic",
        }
    }

    /// 解析 snake_case 字符串（与 `ModelType::from_str_value` 同款命名，避免与
    /// `std::str::FromStr` 混淆）。
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "zipvoice" => Some(Self::Zipvoice),
            "kitten" => Some(Self::Kitten),
            "omnivoice" => Some(Self::Omnivoice),
            "voxcpm2" => Some(Self::Voxcpm2),
            "qwen3_tts_06" => Some(Self::Qwen3Tts06),
            "qwen3_tts_17" => Some(Self::Qwen3Tts17),
            "supertonic" => Some(Self::Supertonic),
            _ => None,
        }
    }

    /// 是否使用参考音频（声音克隆）语义：ZipVoice（sherpa）与 OmniVoice/VoxCPM2/
    /// Qwen3-TTS（audiocpp）支持克隆，其余（未收录的二期占位 kind）不支持。
    pub fn uses_reference_audio(&self) -> bool {
        matches!(
            self,
            Self::Zipvoice | Self::Omnivoice | Self::Voxcpm2 | Self::Qwen3Tts06 | Self::Qwen3Tts17
        )
    }

    /// 是否需要 espeak-ng 数据目录：仅 ZipVoice（包内 `espeak-ng-data/`）。
    pub fn requires_data_dir(&self) -> bool {
        matches!(self, Self::Zipvoice)
    }
}

/// 手写 Deserialize：未知 kind 回落默认 Zipvoice。
///
/// 历史版本曾收录 vits/matcha/kokoro/pocket 等模型并持久化进 `settings.toml`，
/// 收录移除后老配置里的这些值若走派生反序列化会让**整份** settings 加载失败。
/// 回落默认值让升级用户平滑迁移到 Zipvoice（模型目录不匹配时预检会给出
/// install-model 提示，引导重新选择受支持的模型）。
impl<'de> Deserialize<'de> for TtsModelKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::parse_str(&s).unwrap_or_default())
    }
}

/// TTS 推理后端：sherpa-onnx（进程内）或 audio.cpp（sidecar HTTP）。
///
/// 与 [`TtsModelKind`] 正交：模型类型描述「什么模型」，后端描述「谁推理」。
/// 当前 audiocpp 后端服务 OmniVoice/VoxCPM2/Qwen3-TTS 克隆族。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TtsBackendKind {
    /// sherpa-onnx 进程内 `OfflineTts`（默认，行为不变）
    #[default]
    Sherpa,
    /// audio.cpp sidecar 进程（audiocpp_server，OpenAI 风格 HTTP）
    Audiocpp,
}

impl TtsBackendKind {
    /// snake_case 字符串（配置/JSON 直传）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sherpa => "sherpa",
            Self::Audiocpp => "audiocpp",
        }
    }

    /// 解析 snake_case 字符串（与 `TtsModelKind::parse_str` 同款命名）。
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "sherpa" => Some(Self::Sherpa),
            "audiocpp" => Some(Self::Audiocpp),
            _ => None,
        }
    }
}

/// 各模型类型安装完成所需文件（相对模型目录；`data_dir`/参考音频由引擎单独校验）。
pub fn required_files(kind: TtsModelKind) -> &'static [&'static str] {
    match kind {
        TtsModelKind::Zipvoice => &REQUIRED_FILES,
        // 二期模型（kitten/supertonic）：registry 尚未收录，暂无下载路径；
        // audiocpp 族由 families 表校验，不消费 sherpa 清单
        _ => &[],
    }
}

/// 解析后的完整 TTS 配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTtsConfig {
    /// 是否启用语音合成
    pub enabled: bool,
    /// 模型类型（决定引擎构造分支与合成参数语义；默认 Zipvoice）
    pub model_type: TtsModelKind,
    pub model_dir: PathBuf,
    /// Kitten 主模型文件（zipvoice 无单一主模型，为 None）
    pub model: Option<PathBuf>,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub vocoder: PathBuf,
    pub tokens: PathBuf,
    pub lexicon: PathBuf,
    pub data_dir: PathBuf,
    pub reference_wav: PathBuf,
    pub reference_text: String,
    /// 默认音色 id（zipvoice 如 `leijun-1`/自定义音色 id）。
    pub voice: Option<String>,
    /// 扩散解码步数（质量/速度权衡）
    pub num_steps: i32,
    /// 语速
    pub speed: f32,
    pub provider: String,
    pub num_threads: i32,
    pub debug: bool,
    /// 推理后端（决定 `TtsEngine` 内部分派；缺省 Sherpa，向后兼容）
    pub backend: TtsBackendKind,
    /// audiocpp 引擎二进制覆盖路径（开发/调试用；None = locator 自动定位）
    pub engine_path: Option<PathBuf>,
}

impl Default for ResolvedTtsConfig {
    fn default() -> Self {
        let model_dir = default_model_dir();
        let join = |name: &str| model_dir.join(name);
        Self {
            enabled: true,
            model_type: TtsModelKind::Zipvoice,
            model: None,
            encoder: join(DEFAULT_ENCODER),
            decoder: join(DEFAULT_DECODER),
            vocoder: join(DEFAULT_VOCODER),
            tokens: join(DEFAULT_TOKENS),
            lexicon: join(DEFAULT_LEXICON),
            data_dir: join(DEFAULT_DATA_DIR),
            reference_wav: join(DEFAULT_REFERENCE_WAV),
            model_dir,
            reference_text: DEFAULT_REFERENCE_TEXT.to_string(),
            voice: None,
            num_steps: 4,
            speed: 1.0,
            provider: "cpu".to_string(),
            num_threads: 2,
            debug: false,
            backend: TtsBackendKind::Sherpa,
            engine_path: None,
        }
    }
}

impl ResolvedTtsConfig {
    /// 是否使用参考音频（声音克隆）语义：sherpa 后端仅 ZipVoice，audiocpp 后端
    /// 为 OmniVoice/VoxCPM2/Qwen3-TTS 克隆族。
    ///
    /// 编排层（voice 会话 / GUI / CLI）应调此方法而非裸
    /// `model_type.uses_reference_audio()`——保持 backend 感知，防「audiocpp
    /// 后端 + 目录探测误判 Zipvoice」的老场景误走 Reference 路径。
    pub fn uses_reference_audio(&self) -> bool {
        match self.backend {
            TtsBackendKind::Sherpa => self.model_type == TtsModelKind::Zipvoice,
            TtsBackendKind::Audiocpp => {
                matches!(
                    self.model_type,
                    TtsModelKind::Omnivoice
                        | TtsModelKind::Voxcpm2
                        | TtsModelKind::Qwen3Tts06
                        | TtsModelKind::Qwen3Tts17
                )
            }
        }
    }
}

/// TTS 就绪预检（backend 感知的单一权威入口）。
///
/// - sherpa：按 [`required_files`] 逐文件 + `requires_data_dir` 校验 espeak-ng-data；
/// - audiocpp：按模型族描述表（`crate::audiocpp::families`）的 `required_files`
///   逐文件校验（不查 sherpa 五件套）；sherpa-only kind 配 audiocpp 后端的非法
///   组合在此明确报错。
pub fn preflight(cfg: &ResolvedTtsConfig) -> Result<(), String> {
    let (files, hint): (&[&str], &str) = match cfg.backend {
        TtsBackendKind::Sherpa => {
            if cfg.model_type.requires_data_dir() && !cfg.data_dir.is_dir() {
                return Err(format!(
                    "缺少数据目录 data_dir: {}\n请运行 `zapmomo tts install-model` 下载模型。",
                    cfg.data_dir.display()
                ));
            }
            (
                required_files(cfg.model_type),
                "zapmomo tts install-model" as &str,
            )
        }
        TtsBackendKind::Audiocpp => {
            let desc = crate::audiocpp::families::family_desc(cfg.model_type).ok_or_else(|| {
                format!(
                    "模型类型 {} 不支持 audiocpp 后端（请检查 [tts].model_type 与 backend 组合）",
                    cfg.model_type.as_str()
                )
            })?;
            (desc.required_files, desc.registry_hint)
        }
    };
    for name in files {
        let p = cfg.model_dir.join(name);
        if !p.is_file() {
            return Err(format!(
                "缺少模型文件 {name}: {}\n请运行 `{hint}` 下载模型。",
                p.display()
            ));
        }
    }
    Ok(())
}

/// 模型是否就绪（[`preflight`] 的布尔版，GUI `models_present` 徽标用）。
///
/// 引擎二进制定位失败不在此拦截（合成时报错更精确）。
pub fn models_present(cfg: &ResolvedTtsConfig) -> bool {
    preflight(cfg).is_ok()
}

/// 用户默认模型目录：`~/.zapmomo/models/<模型名>`
pub fn user_default_model_dir() -> PathBuf {
    crate::kws::model::tts_user_model_dir()
}

/// 源码仓库中的模型目录（开发者 `./models/<模型名>`，仅作开发回退）。
fn repo_models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(&crate::kws::model::tts_asset().name)
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
        .map(|l| l.join(&crate::kws::model::tts_asset().name));
    choose_default_model_dir(
        &user_default_model_dir(),
        legacy.as_deref(),
        &repo_models_dir(),
    )
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

/// 解析模型目录：CLI > settings > 默认。
fn resolve_model_dir(
    settings: Option<&TtsSettings>,
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
    settings: Option<&TtsSettings>,
    cli_model_dir: Option<&Path>,
) -> Result<ResolvedTtsConfig, String> {
    let mut cfg = ResolvedTtsConfig {
        model_dir: resolve_model_dir(settings, cli_model_dir)?,
        ..ResolvedTtsConfig::default()
    };

    let s = settings;
    cfg.enabled = s.and_then(|s| s.enabled).unwrap_or(true);
    // 模型类型：settings 显式 > 默认 Zipvoice（managed 目录名 → kind 的权威写入
    // 在 `set_selected_model`；无字段的老配置 → Zipvoice，行为不变）
    cfg.model_type = s.and_then(|s| s.model_type).unwrap_or_default();

    let file = |field: &str, default_name: &str| {
        let value = match field {
            "encoder" => s.and_then(|s| s.encoder.as_deref()),
            "decoder" => s.and_then(|s| s.decoder.as_deref()),
            "vocoder" => s.and_then(|s| s.vocoder.as_deref()),
            "tokens" => s.and_then(|s| s.tokens.as_deref()),
            "lexicon" => s.and_then(|s| s.lexicon.as_deref()),
            "data_dir" => s.and_then(|s| s.data_dir.as_deref()),
            "reference_wav" => s.and_then(|s| s.reference_wav.as_deref()),
            _ => None,
        };
        resolve_file(value, default_name, &cfg.model_dir)
    };

    cfg.encoder = file("encoder", DEFAULT_ENCODER)?;
    cfg.decoder = file("decoder", DEFAULT_DECODER)?;
    cfg.vocoder = file("vocoder", DEFAULT_VOCODER)?;
    cfg.tokens = file("tokens", DEFAULT_TOKENS)?;
    cfg.lexicon = file("lexicon", DEFAULT_LEXICON)?;
    cfg.data_dir = file("data_dir", DEFAULT_DATA_DIR)?;
    cfg.reference_wav = file("reference_wav", DEFAULT_REFERENCE_WAV)?;

    // 按模型类型填主模型（zipvoice 无主模型；非 zipvoice 的旧 zipvoice 字段保持
    // 默认但由预检/引擎按 `required_files(kind)` 分支跳过，不参与消费）。
    if cfg.model_type == TtsModelKind::Kitten {
        cfg.model = Some(cfg.model_dir.join("model.onnx"));
    }
    // audiocpp 族不消费 sherpa 文件字段：GGUF 定位由 `AudiocppTts` 内部经
    // families 表完成（model_dir + gguf_file）。

    cfg.reference_text = s
        .and_then(|s| s.reference_text.clone())
        .unwrap_or_else(|| DEFAULT_REFERENCE_TEXT.to_string());
    cfg.voice = s.and_then(|s| s.voice.clone());
    cfg.num_steps = s.and_then(|s| s.num_steps).unwrap_or(4);
    cfg.speed = s.and_then(|s| s.speed).unwrap_or(1.0);
    cfg.num_threads = s.and_then(|s| s.num_threads).unwrap_or(2);
    cfg.debug = s.and_then(|s| s.debug).unwrap_or(false);
    // 推理后端：缺省 sherpa（老用户无字段行为不变），非法值显式报错
    cfg.backend = match s.and_then(|s| s.backend.as_deref()) {
        Some(v) => TtsBackendKind::parse_str(v)
            .ok_or_else(|| format!("未知 TTS 后端: {v}（支持 sherpa / audiocpp）"))?,
        None => TtsBackendKind::default(),
    };
    // 推理设备：用户显式配置优先；缺省时 audiocpp 后端按模型族取默认
    // （克隆族 metal——CPU RTF 6.6 不可用、Metal 0.41 达标，
    // 技术方案阶段 1 实测），sherpa 恒 cpu。
    cfg.provider = match s.and_then(|s| s.provider.clone()) {
        Some(p) => p,
        None => {
            if cfg.backend == TtsBackendKind::Audiocpp
                && let Some(desc) = crate::audiocpp::families::family_desc(cfg.model_type)
            {
                desc.default_provider.to_string()
            } else {
                "cpu".to_string()
            }
        }
    };
    cfg.engine_path = s
        .and_then(|s| s.engine_path.as_deref())
        .map(resolve_env_ref)
        .transpose()?
        .map(PathBuf::from);

    Ok(cfg)
}

/// `set_tts_params` 载荷：可调整的 TTS 合成参数（snake_case 直传，缺省项不修改）。
///
/// 与 `AsrParamsPatch` 对称，放在 lib crate 内以便 `cargo test` 单测。
/// 引擎在每次合成时新建（`synthesize_tts` → `TtsEngine::new`），因此保存后**下一次合成即生效**，无需重启。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TtsParamsPatch {
    /// 扩散解码步数（质量/速度权衡）
    pub num_steps: Option<i32>,
    /// 默认语速（单次合成可经 `synthesize_tts.speed` 覆盖）
    pub speed: Option<f32>,
    /// 推理线程数
    pub num_threads: Option<i32>,
    /// 调试输出
    pub debug: Option<bool>,
}

impl TtsParamsPatch {
    /// 先整体校验（任一越界立即 Err），再逐项写入 `TtsSettings`，保证出错时不部分修改。
    pub fn apply_to(&self, tts: &mut TtsSettings) -> Result<(), String> {
        if let Some(v) = self.num_steps
            && !(1..=32).contains(&v)
        {
            return Err(format!("扩散步数需在 1~32，当前 {v}"));
        }
        if let Some(v) = self.speed
            && !(0.5..=2.0).contains(&v)
        {
            return Err(format!("语速需在 0.5~2.0，当前 {v}"));
        }
        if let Some(v) = self.num_threads
            && !(1..=32).contains(&v)
        {
            return Err(format!("线程数需在 1~32，当前 {v}"));
        }

        if let Some(v) = self.num_steps {
            tts.num_steps = Some(v);
        }
        if let Some(v) = self.speed {
            tts.speed = Some(v);
        }
        if let Some(v) = self.num_threads {
            tts.num_threads = Some(v);
        }
        if let Some(v) = self.debug {
            tts.debug = Some(v);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::TtsSettings;
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
    fn test_default_config_points_to_default_model_dir() {
        let cfg = ResolvedTtsConfig::default();
        assert_eq!(
            cfg.model_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string()),
            Some(crate::kws::model::tts_asset().name.clone())
        );
        assert_eq!(cfg.encoder.file_name().unwrap(), DEFAULT_ENCODER);
        assert_eq!(cfg.decoder.file_name().unwrap(), DEFAULT_DECODER);
        assert_eq!(cfg.vocoder.file_name().unwrap(), DEFAULT_VOCODER);
        assert_eq!(cfg.tokens.file_name().unwrap(), DEFAULT_TOKENS);
        assert_eq!(cfg.lexicon.file_name().unwrap(), DEFAULT_LEXICON);
        assert_eq!(cfg.data_dir.file_name().unwrap(), DEFAULT_DATA_DIR);
        assert_eq!(cfg.reference_wav.file_name().unwrap(), "leijun-1.wav");
        assert_eq!(cfg.reference_text, DEFAULT_REFERENCE_TEXT);
        assert_eq!(cfg.num_steps, 4);
        assert_eq!(cfg.speed, 1.0);
        assert_eq!(cfg.provider, "cpu");
    }

    #[test]
    fn test_user_default_model_dir() {
        run_with_temp_home(|home| {
            let dir = super::user_default_model_dir();
            assert_eq!(
                dir,
                home.join(".zapmomo/models")
                    .join(crate::kws::model::tts_asset().name.as_str())
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
    fn test_resolve_enabled_default_true_and_override() {
        // 未配置时默认启用，避免破坏现有用户
        assert!(resolve(None, None).unwrap().enabled);
        // settings 显式关闭时生效
        let settings = TtsSettings {
            enabled: Some(false),
            ..TtsSettings::default()
        };
        assert!(!resolve(Some(&settings), None).unwrap().enabled);
    }

    #[test]
    fn test_resolve_no_settings_uses_defaults() {
        // 用临时 HOME 隔离，避免与其它 `run_with_temp_home` 测试并行时 HOME 竞态
        // 导致 `resolve` 与 `ResolvedTtsConfig::default` 两次读取到不同 HOME。
        run_with_temp_home(|_| {
            let cfg = resolve(None, None).unwrap();
            assert_eq!(cfg, ResolvedTtsConfig::default());
        });
    }

    fn abs_path(rel: &str) -> PathBuf {
        std::path::absolute(rel).unwrap()
    }

    #[test]
    fn test_resolve_cli_model_dir_overrides_settings() {
        let settings = TtsSettings {
            model_dir: Some("settings-model".to_string()),
            ..TtsSettings::default()
        };
        let cli = abs_path("tmp/cli-tts");
        let cfg = resolve(Some(&settings), Some(&cli)).unwrap();
        assert_eq!(cfg.model_dir, cli);
        assert_eq!(cfg.encoder.parent().unwrap(), cli);
    }

    #[test]
    fn test_resolve_settings_overrides_numeric_and_text() {
        let settings = TtsSettings {
            num_threads: Some(4),
            num_steps: Some(6),
            speed: Some(1.5),
            reference_text: Some("自定义参考文本".to_string()),
            debug: Some(true),
            ..TtsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.num_threads, 4);
        assert_eq!(cfg.num_steps, 6);
        assert_eq!(cfg.speed, 1.5);
        assert_eq!(cfg.reference_text, "自定义参考文本");
        assert!(cfg.debug);
    }

    #[test]
    fn test_resolve_relative_model_dir_anchored_to_user_dir() {
        run_with_temp_home(|home| {
            let settings = TtsSettings {
                model_dir: Some("models/my-tts".to_string()),
                ..TtsSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.model_dir, home.join(".zapmomo/models/my-tts"));
        });
    }

    #[test]
    fn test_resolve_voice_default_none_and_override() {
        // 未配置默认音色 → None（用 reference_wav 即 leijun）
        let cfg = resolve(None, None).unwrap();
        assert_eq!(cfg.voice, None);
        // settings 配置音色 id → 解析生效
        let settings = TtsSettings {
            voice: Some("custom-123".to_string()),
            ..TtsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.voice.as_deref(), Some("custom-123"));
    }

    #[test]
    fn test_required_files_count_and_content() {
        assert_eq!(REQUIRED_FILES.len(), 5);
        assert!(REQUIRED_FILES.contains(&DEFAULT_VOCODER));
        assert!(REQUIRED_FILES.contains(&DEFAULT_LEXICON));
    }

    #[test]
    fn test_required_files_by_kind() {
        assert_eq!(required_files(TtsModelKind::Zipvoice).len(), 5);
        // 二期模型尚无下载路径
        assert!(required_files(TtsModelKind::Kitten).is_empty());
        assert!(required_files(TtsModelKind::Supertonic).is_empty());
    }

    #[test]
    fn test_model_kind_str_and_semantics() {
        for (s, kind) in [
            ("zipvoice", TtsModelKind::Zipvoice),
            ("kitten", TtsModelKind::Kitten),
        ] {
            assert_eq!(TtsModelKind::parse_str(s), Some(kind), "{s}");
            assert_eq!(kind.as_str(), s);
        }
        assert_eq!(TtsModelKind::parse_str("unknown"), None);
        // 参考音频克隆语义仅 zipvoice（sherpa 侧）
        assert!(TtsModelKind::Zipvoice.uses_reference_audio());
        assert!(!TtsModelKind::Kitten.uses_reference_audio());
        // espeak-ng-data 需求仅 zipvoice
        assert!(TtsModelKind::Zipvoice.requires_data_dir());
        assert!(!TtsModelKind::Kitten.requires_data_dir());
    }

    /// 老版本 settings 里的已移除 kind（vits/matcha/kokoro/pocket）反序列化
    /// 不炸整份配置，回落默认 Zipvoice（升级迁移路径）。
    #[test]
    fn test_kind_deserialize_unknown_falls_back_to_default() {
        let toml_str = r#"
enabled = true
model_type = "kokoro"
"#;
        let settings: TtsSettings = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.model_type, Some(TtsModelKind::Zipvoice));
        // 合法值照常解析
        let toml_str = r#"
model_type = "omnivoice"
"#;
        let settings: TtsSettings = toml::from_str(toml_str).unwrap();
        assert_eq!(settings.model_type, Some(TtsModelKind::Omnivoice));
    }

    /// qwen3_tts 两尺寸 kind 解析/序列化往返 + 克隆语义。
    #[test]
    fn test_qwen3_tts_kind_semantics() {
        for (s, kind) in [
            ("qwen3_tts_06", TtsModelKind::Qwen3Tts06),
            ("qwen3_tts_17", TtsModelKind::Qwen3Tts17),
        ] {
            assert_eq!(TtsModelKind::parse_str(s), Some(kind), "{s}");
            assert_eq!(kind.as_str(), s);
        }
        assert!(TtsModelKind::Qwen3Tts06.uses_reference_audio());
        assert!(TtsModelKind::Qwen3Tts17.uses_reference_audio());
    }

    #[test]
    fn test_params_patch_applies_all_fields() {
        let mut tts = TtsSettings::default();
        let patch = TtsParamsPatch {
            num_steps: Some(8),
            speed: Some(1.2),
            num_threads: Some(4),
            debug: Some(true),
        };
        patch.apply_to(&mut tts).unwrap();
        assert_eq!(tts.num_steps, Some(8));
        assert_eq!(tts.speed, Some(1.2));
        assert_eq!(tts.num_threads, Some(4));
        assert_eq!(tts.debug, Some(true));
    }

    #[test]
    fn test_params_patch_validates_before_writing() {
        // 任一字段越界即整体失败，且不部分修改其它字段
        let mut tts = TtsSettings {
            num_steps: Some(4),
            ..TtsSettings::default()
        };
        let err = TtsParamsPatch {
            num_steps: Some(100),
            num_threads: Some(4),
            ..TtsParamsPatch::default()
        }
        .apply_to(&mut tts)
        .unwrap_err();
        assert!(err.contains("扩散步数"), "err: {err}");
        assert_eq!(tts.num_threads, None, "校验失败时不应写入其它字段");
        assert_eq!(tts.num_steps, Some(4));

        let err = TtsParamsPatch {
            speed: Some(3.0),
            ..TtsParamsPatch::default()
        }
        .apply_to(&mut TtsSettings::default())
        .unwrap_err();
        assert!(err.contains("语速"), "err: {err}");

        let err = TtsParamsPatch {
            num_threads: Some(64),
            ..TtsParamsPatch::default()
        }
        .apply_to(&mut TtsSettings::default())
        .unwrap_err();
        assert!(err.contains("线程数"), "err: {err}");
    }

    #[test]
    fn test_params_patch_none_leaves_unchanged() {
        let mut tts = TtsSettings {
            num_steps: Some(6),
            speed: Some(1.5),
            num_threads: Some(8),
            debug: Some(true),
            ..TtsSettings::default()
        };
        TtsParamsPatch::default().apply_to(&mut tts).unwrap();
        assert_eq!(tts.num_steps, Some(6));
        assert_eq!(tts.speed, Some(1.5));
        assert_eq!(tts.num_threads, Some(8));
        assert_eq!(tts.debug, Some(true));
    }

    #[test]
    fn test_backend_kind_str_and_parse() {
        for (s, kind) in [
            ("sherpa", TtsBackendKind::Sherpa),
            ("audiocpp", TtsBackendKind::Audiocpp),
        ] {
            assert_eq!(TtsBackendKind::parse_str(s), Some(kind), "{s}");
            assert_eq!(kind.as_str(), s);
        }
        assert_eq!(TtsBackendKind::parse_str("unknown"), None);
        assert_eq!(TtsBackendKind::default(), TtsBackendKind::Sherpa);
    }

    #[test]
    fn test_resolve_backend_default_explicit_and_invalid() {
        // 缺省 → sherpa（老用户行为不变）
        assert_eq!(resolve(None, None).unwrap().backend, TtsBackendKind::Sherpa);
        // 显式 audiocpp → 生效
        let settings = TtsSettings {
            backend: Some("audiocpp".to_string()),
            ..TtsSettings::default()
        };
        assert_eq!(
            resolve(Some(&settings), None).unwrap().backend,
            TtsBackendKind::Audiocpp
        );
        // 非法值 → 报错（含支持列表）
        let settings = TtsSettings {
            backend: Some("vllm".to_string()),
            ..TtsSettings::default()
        };
        let err = resolve(Some(&settings), None).unwrap_err();
        assert!(err.contains("未知 TTS 后端"), "err: {err}");
        assert!(err.contains("sherpa / audiocpp"), "err: {err}");
    }

    #[test]
    fn test_resolve_engine_path_passthrough() {
        // 未配置 → None（locator 自动定位）
        assert_eq!(resolve(None, None).unwrap().engine_path, None);
        // 显式配置 → 透传（支持 env 引用语义与 model_dir 一致）
        let settings = TtsSettings {
            engine_path: Some("/opt/audiocpp/audiocpp_server".to_string()),
            ..TtsSettings::default()
        };
        assert_eq!(
            resolve(Some(&settings), None).unwrap().engine_path,
            Some(PathBuf::from("/opt/audiocpp/audiocpp_server"))
        );
    }

    #[test]
    fn test_uses_reference_audio_backend_aware() {
        // sherpa + zipvoice → true（默认组合）
        assert!(ResolvedTtsConfig::default().uses_reference_audio());
        // sherpa + 未收录二期 kind → false
        let mut cfg = ResolvedTtsConfig::default();
        cfg.model_type = TtsModelKind::Kitten;
        assert!(!cfg.uses_reference_audio());
        // audiocpp 后端 + sherpa kind（settings 残留）→ false（非法组合由预检拦截）
        let settings = TtsSettings {
            backend: Some("audiocpp".to_string()),
            ..TtsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert!(!cfg.uses_reference_audio());

        // audiocpp + qwen3_tts -> true（克隆族，backend 感知）
        let cfg = ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_type: TtsModelKind::Qwen3Tts06,
            ..ResolvedTtsConfig::default()
        };
        assert!(cfg.uses_reference_audio());
    }

    /// omnivoice（单文件清单）+ 非法组合（sherpa kind 配 audiocpp 后端）报错。
    #[test]
    fn test_preflight_audiocpp_omnivoice_and_invalid_combo() {
        let base = tempfile::tempdir().unwrap();
        let mut cfg = ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: TtsModelKind::Omnivoice,
            model_dir: base.path().to_path_buf(),
            ..ResolvedTtsConfig::default()
        };

        // 空目录 → 报缺 omnivoice gguf（提示语指向 omnivoice registry id）
        let err = preflight(&cfg).unwrap_err();
        assert!(err.contains("omnivoice-q8_0.gguf"), "err: {err}");
        assert!(err.contains("tts-omnivoice-q8-audiocpp"), "err: {err}");

        // 单文件齐 → 通过（无 embeddings 副件）；models_present 同步为 true
        std::fs::write(cfg.model_dir.join("omnivoice-q8_0.gguf"), b"x").unwrap();
        assert!(preflight(&cfg).is_ok());
        assert!(models_present(&cfg));

        // 非法组合：sherpa kind + audiocpp 后端 → 明确报组合错误
        cfg.model_type = TtsModelKind::Zipvoice;
        let err = preflight(&cfg).unwrap_err();
        assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");

        // qwen3：空目录 -> 报缺 base gguf（提示语指向 qwen3 registry id）
        let cfg = ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: TtsModelKind::Qwen3Tts06,
            model_dir: base.path().to_path_buf(),
            ..ResolvedTtsConfig::default()
        };
        let err = preflight(&cfg).unwrap_err();
        assert!(
            err.contains("qwen3-tts-12hz-0.6b-base-q8_0.gguf"),
            "err: {err}"
        );
        assert!(err.contains("tts-qwen3-06b-base-q8-audiocpp"), "err: {err}");

        // 单文件齐 -> 通过
        std::fs::write(
            cfg.model_dir.join("qwen3-tts-12hz-0.6b-base-q8_0.gguf"),
            b"x",
        )
        .unwrap();
        assert!(preflight(&cfg).is_ok());
    }

    #[test]
    fn test_preflight_sherpa_keeps_existing_behavior() {
        // sherpa 缺文件 → 沿用 install-model 文案（既有测试锚点不变）
        let mut cfg = ResolvedTtsConfig::default();
        cfg.model_dir = PathBuf::from("/nonexistent/model");
        // data_dir 显式指向已存在目录：并行下 HOME 可能被 run_with_temp_home 临时
        // 替换，default() 解析出的 data_dir 会落在临时路径上触发「缺数据目录」分支，
        // 掩盖本测试要锚定的「缺模型文件」分支
        let data_dir = tempfile::tempdir().unwrap();
        cfg.data_dir = data_dir.path().to_path_buf();
        let err = preflight(&cfg).unwrap_err();
        assert!(err.contains("install-model"), "err: {err}");
        assert!(!models_present(&cfg));
        assert!(err.contains("encoder.int8.onnx"), "err: {err}");
    }
}
