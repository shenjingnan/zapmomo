/// 语音会话配置解析。
///
/// 聚合 KWS / ASR / LLM / TTS 四个引擎配置（复用各自的 `config::resolve`）+
/// 会话级参数（唤醒词 / 音色 / 语速 / 轮数 / 打断等）。
/// 优先级与各引擎一致：CLI 覆盖 > settings.toml `[voice]` 段 > 内置默认。
use crate::config::settings::{AppConfig, VoiceSettings};
use std::path::PathBuf;

/// 默认历史消息条数上限（传给 LLM 的多轮上下文；按条数计。工具轮一条对话
/// 占 4 条——user + call + result + assistant——24 条 ≈ 6 个单工具轮）。
pub const DEFAULT_HISTORY_MAX: usize = 24;
/// 默认打断 KWS 触发阈值（高于监听阈值 0.25，缓解回声误触发）。
pub const DEFAULT_BARGE_IN_THRESHOLD: f32 = 0.5;
/// 语音打断（ASR barge-in）缺省开启；非流式 ASR 后端不生效（自动降级）。
pub const DEFAULT_VOICE_BARGE_IN: bool = true;
/// 语音打断回声比对阈值：字符 bigram Dice ≥ 此值判回声忽略（外放保守值）。
/// 注意与 [`DEFAULT_BARGE_IN_THRESHOLD`]（KWS RMS 触发阈值）语义不同。
pub const DEFAULT_BARGE_IN_SIMILARITY_THRESHOLD: f32 = 0.5;
/// 默认唤醒欢迎语（TTS 用当前音色合成播放）。
pub const DEFAULT_WELCOME_TEXT: &str = "你好，我在。";
/// 默认「真正说话」RMS 音量阈值。
pub const DEFAULT_VAD_SILENCE_THRESHOLD: f32 = 0.02;
/// 默认 ASR 说完的连续静音秒数。
pub const DEFAULT_ASR_MAX_TRAILING_SILENCE: f32 = 3.0;
/// 默认单句连续语音时长上限（秒）：达到即强制断句进入回复，杜绝无限聆听
/// （对齐 sherpa 端点 rule3「超长句强制断段」语义）。
pub const DEFAULT_ASR_MAX_UTTERANCE_DURATION: f32 = 30.0;
/// 默认欢迎语后等用户说话的超时（秒），超时回待唤醒。
pub const DEFAULT_WELCOME_WAIT_TIMEOUT: f32 = 8.0;

/// CLI 覆盖参数（来自 `voice run` 命令行，缺省字段不覆盖 settings）。
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    /// 麦克风设备名（None = 系统默认或 settings.microphone）
    pub device: Option<String>,
    pub keywords: Option<String>,
    pub voice: Option<String>,
    pub speed: Option<f32>,
    pub max_turns: Option<u32>,
    pub history_max: Option<usize>,
    /// true = CLI 显式 `--no-bargein` 强制关闭打断（缺省 false 不强制，交给 settings）
    pub no_bargein: bool,
    /// true = CLI 显式 `--no-follow-up` 强制关闭跟听窗口（缺省 false 不强制，交给 settings）
    pub no_follow_up: bool,
    /// true = CLI 显式 `--no-voice-barge-in` 强制关闭语音打断（缺省 false 不强制，交给 settings）
    pub no_voice_barge_in: bool,
    pub barge_in_threshold: Option<f32>,
    pub welcome_text: Option<String>,
    pub vad_silence_threshold: Option<f32>,
    pub asr_max_trailing_silence: Option<f32>,
    pub welcome_wait_timeout: Option<f32>,
    pub kws_model_dir: Option<PathBuf>,
    pub asr_model_dir: Option<PathBuf>,
    pub tts_model_dir: Option<PathBuf>,
}

/// 解析后的完整会话配置（字段全部为具体类型）。
#[derive(Debug, Clone)]
pub struct ResolvedSessionConfig {
    /// 麦克风设备名（None = 系统默认）
    pub mic_device: Option<String>,
    pub kws: crate::kws::config::ResolvedKwsConfig,
    pub asr: crate::asr::config::ResolvedAsrConfig,
    pub tts: crate::tts::config::ResolvedTtsConfig,
    pub llm: crate::llm::config::ResolvedLlmConfig,
    /// TTS 音色 id（None = 用 `tts` 配置默认参考音频）
    pub voice_id: Option<String>,
    /// TTS 语速
    pub speed: f32,
    /// 会话唤醒词（None = KWS 模型内置关键词）
    pub keywords: Option<String>,
    /// 最多对话轮数（None = 无限，Ctrl-C 退出）
    pub max_turns: Option<u32>,
    /// 传给 LLM 的历史消息条数上限
    pub history_max: usize,
    /// 播报/思考中唤醒词打断
    pub barge_in: bool,
    /// 播报/思考中语音打断（ASR 识别到用户说话即打断；仅流式 ASR 后端生效，
    /// 能力门控见 `AsrBackend::has_streaming_partial`）
    pub voice_barge_in: bool,
    /// 语音打断的回声比对阈值（字符 bigram Dice，≥ 判回声忽略）
    pub barge_in_similarity_threshold: f32,
    /// 回复播完后自动进入 ASR 聆听（跟听免唤醒；空识别保持聆听，不回待唤醒）
    pub follow_up: bool,
    /// 打断用 KWS 触发阈值
    pub barge_in_threshold: f32,
    /// 唤醒欢迎语文本
    pub welcome_text: String,
    /// 「真正说话」RMS 音量阈值
    pub vad_silence_threshold: f32,
    /// ASR 说完的连续静音秒数
    pub asr_max_trailing_silence: f32,
    /// 单句连续语音时长上限（秒），达到即强制断句（防无限聆听兜底）
    pub asr_max_utterance_duration: f32,
    /// 欢迎语后等用户说话的超时（秒）
    pub welcome_wait_timeout: f32,
    /// active 伙伴的音色克隆参考（`apply_companion_overrides` 注入；None = 无伙伴音色，
    /// 上层回退 `[voice].voice` > `[tts].voice` > 内置默认）。
    pub character_voice: Option<crate::companion::CharacterVoice>,
    /// active 伙伴的预合成欢迎语（`apply_companion_overrides` 注入；None = 唤醒时
    /// 走实时合成）。新鲜度在注入时按指纹判定，唤醒时 `load_fresh` 再校验一次。
    pub welcome_clip: Option<WelcomeClip>,
}

/// 预合成欢迎语的定位信息（路径 + 注入时的期望指纹）。
///
/// 唤醒时指纹必须与旁车一致才直接播放；不匹配（生成中/已过期/被删）降级实时合成。
#[derive(Debug, Clone, PartialEq)]
pub struct WelcomeClip {
    pub wav: std::path::PathBuf,
    pub meta: std::path::PathBuf,
    pub fingerprint: String,
}

/// 合并 settings 与 CLI 覆盖得到最终会话配置。
pub fn resolve(
    settings: Option<&AppConfig>,
    cli: &CliOverrides,
) -> Result<ResolvedSessionConfig, String> {
    let voice: Option<&VoiceSettings> = settings.and_then(|s| s.voice.as_ref());
    let kws = crate::kws::config::resolve(
        settings.and_then(|s| s.kws.as_ref()),
        cli.kws_model_dir.as_deref(),
    )?;
    let asr = crate::asr::config::resolve(
        settings.and_then(|s| s.asr.as_ref()),
        cli.asr_model_dir.as_deref(),
    )?;
    let tts = crate::tts::config::resolve(
        settings.and_then(|s| s.tts.as_ref()),
        cli.tts_model_dir.as_deref(),
    )?;
    let llm = crate::llm::config::resolve(settings.and_then(|s| s.llm.as_ref()))?;

    Ok(ResolvedSessionConfig {
        // CLI --device > settings.microphone（全局）> 系统默认
        mic_device: cli
            .device
            .clone()
            .or_else(|| settings.and_then(|s| s.microphone.clone())),
        kws,
        asr,
        tts,
        llm,
        voice_id: cli
            .voice
            .clone()
            .or_else(|| voice.and_then(|v| v.voice.clone())),
        speed: cli
            .speed
            .or_else(|| voice.and_then(|v| v.speed))
            .unwrap_or(1.0),
        // 唤醒词：CLI > [voice].keywords > [kws].custom_keywords（复用用户已配的唤醒词）
        keywords: cli
            .keywords
            .clone()
            .or_else(|| voice.and_then(|v| v.keywords.clone()))
            .or_else(|| {
                settings
                    .and_then(|s| s.kws.as_ref())
                    .and_then(|k| k.custom_keywords.clone())
            }),
        max_turns: cli.max_turns.or_else(|| voice.and_then(|v| v.max_turns)),
        history_max: cli
            .history_max
            .or_else(|| voice.and_then(|v| v.history_max))
            .unwrap_or(DEFAULT_HISTORY_MAX),
        // CLI `--no-bargein` 强制关；未指定时尊重 settings（缺省开）
        barge_in: !cli.no_bargein && voice.and_then(|v| v.barge_in).unwrap_or(true),
        // CLI `--no-voice-barge-in` 强制关；未指定时尊重 settings（缺省开；
        // 仅流式 ASR 后端真正生效，能力门控在会话侧）
        voice_barge_in: !cli.no_voice_barge_in
            && voice
                .and_then(|v| v.voice_barge_in)
                .unwrap_or(DEFAULT_VOICE_BARGE_IN),
        // 回声比对阈值仅 settings 可调（无 CLI flag；外放保守默认值）
        barge_in_similarity_threshold: voice
            .and_then(|v| v.barge_in_similarity_threshold)
            .unwrap_or(DEFAULT_BARGE_IN_SIMILARITY_THRESHOLD),
        // CLI `--no-follow-up` 强制关；未指定时尊重 settings（缺省开）
        follow_up: !cli.no_follow_up && voice.and_then(|v| v.follow_up).unwrap_or(true),
        barge_in_threshold: cli
            .barge_in_threshold
            .or_else(|| voice.and_then(|v| v.barge_in_threshold))
            .unwrap_or(DEFAULT_BARGE_IN_THRESHOLD),
        welcome_text: cli
            .welcome_text
            .clone()
            .or_else(|| voice.and_then(|v| v.welcome_text.clone()))
            .unwrap_or_else(|| DEFAULT_WELCOME_TEXT.to_string()),
        vad_silence_threshold: cli
            .vad_silence_threshold
            .or_else(|| voice.and_then(|v| v.vad_silence_threshold))
            .unwrap_or(DEFAULT_VAD_SILENCE_THRESHOLD),
        asr_max_trailing_silence: cli
            .asr_max_trailing_silence
            .or_else(|| voice.and_then(|v| v.asr_max_trailing_silence))
            .unwrap_or(DEFAULT_ASR_MAX_TRAILING_SILENCE),
        // 仅 settings 可调（无 CLI flag，与 barge_in_similarity_threshold 同款）
        asr_max_utterance_duration: voice
            .and_then(|v| v.asr_max_utterance_duration)
            .unwrap_or(DEFAULT_ASR_MAX_UTTERANCE_DURATION),
        welcome_wait_timeout: cli
            .welcome_wait_timeout
            .or_else(|| voice.and_then(|v| v.welcome_wait_timeout))
            .unwrap_or(DEFAULT_WELCOME_WAIT_TIMEOUT),
        character_voice: None,
        welcome_clip: None,
    })
}

/// 应用 active 伙伴覆盖（人设 + 唤醒词 + 欢迎语 + 音色），在 `resolve` 之后调用。
///
/// - 人设：active 伙伴是角色包且 character.md 非空 → **完全覆盖** `cfg.llm.system_prompt`
///   （不写盘，切回普通伙伴后下次会话自然回退全局 `[llm].system_prompt`）；
/// - 唤醒词：`resolve_wake_word` 叠加「激活即换词」语义（角色词压过 resolve 的
///   合并结果，编码失败回退并告警）；
/// - 欢迎语：覆盖为角色级生效文本（自定义或「你好，我是{name}。」默认模板）——
///   **有 active 伙伴时全局 `[voice].welcome_text` 不再生效**（预期行为变化）；
/// - 预合成欢迎语：指纹新鲜才注入（唤醒直接播放），否则唤醒时降级实时合成；
/// - 音色：伙伴音色三级解析（托管目录 `voice/` > 音色库绑定 > None，任意 format 均可，
///   见 `companion::companion_voice_in`）；仅当前 TTS 模型支持参考音频克隆时注入
///   `cfg.character_voice`（非克隆模型走全局音色，优雅降级）。
pub fn apply_companion_overrides(cfg: &mut ResolvedSessionConfig) {
    if let Some(persona) = crate::companion::active_persona() {
        cfg.llm.system_prompt = persona;
    }
    // 音色注入必须先于指纹计算：指纹覆盖 character_voice，改音色要能触发重生成。
    if cfg.tts.uses_reference_audio()
        && let Some(voice) = crate::companion::active_companion_voice()
    {
        cfg.character_voice = Some(voice);
    }
    let model = crate::companion::active_model_fast();
    if let Some(m) = &model {
        let r = crate::companion::resolve_wake_word(cfg.keywords.as_deref(), &cfg.kws.tokens);
        if !r.companion_ok {
            tracing::warn!(
                "伙伴 {} 的唤醒词「{}」无法转为 KWS token，已回退全局唤醒词",
                m.id,
                crate::companion::effective_wake_word(m)
            );
        }
        cfg.keywords = r.word;
        cfg.welcome_text = crate::companion::effective_welcome_text(m);
        let fingerprint = crate::companion_welcome::clip_fingerprint(cfg);
        let model_dir = std::path::Path::new(&m.model_dir);
        if crate::companion_welcome::is_fresh(model_dir, &fingerprint) {
            cfg.welcome_clip = Some(WelcomeClip {
                wav: crate::companion_welcome::welcome_wav_path(model_dir),
                meta: crate::companion_welcome::welcome_meta_path(model_dir),
                fingerprint,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn settings_with_voice(voice: VoiceSettings) -> AppConfig {
        AppConfig {
            voice: Some(voice),
            ..Default::default()
        }
    }

    #[test]
    fn test_defaults_with_no_settings() {
        run_with_temp_home(|_| {
            let cfg = resolve(None, &CliOverrides::default()).unwrap();
            assert_eq!(cfg.speed, 1.0);
            assert_eq!(cfg.history_max, DEFAULT_HISTORY_MAX);
            assert!(cfg.barge_in);
            assert!(cfg.follow_up);
            assert!(cfg.voice_barge_in);
            assert_eq!(
                cfg.barge_in_similarity_threshold,
                DEFAULT_BARGE_IN_SIMILARITY_THRESHOLD
            );
            assert_eq!(cfg.barge_in_threshold, DEFAULT_BARGE_IN_THRESHOLD);
            assert_eq!(cfg.welcome_text, DEFAULT_WELCOME_TEXT);
            assert_eq!(cfg.vad_silence_threshold, DEFAULT_VAD_SILENCE_THRESHOLD);
            assert_eq!(
                cfg.asr_max_trailing_silence,
                DEFAULT_ASR_MAX_TRAILING_SILENCE
            );
            assert_eq!(
                cfg.asr_max_utterance_duration,
                DEFAULT_ASR_MAX_UTTERANCE_DURATION
            );
            assert_eq!(cfg.welcome_wait_timeout, DEFAULT_WELCOME_WAIT_TIMEOUT);
            assert_eq!(cfg.voice_id, None);
            assert_eq!(cfg.keywords, None);
            assert_eq!(cfg.max_turns, None);
        });
    }

    #[test]
    fn test_keywords_falls_back_to_kws_custom() {
        run_with_temp_home(|_| {
            // 未配置 [voice].keywords 时，回退到 [kws].custom_keywords
            let app = AppConfig {
                kws: Some(crate::config::settings::KwsSettings {
                    custom_keywords: Some("你好小智/大月下".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let cfg = resolve(Some(&app), &CliOverrides::default()).unwrap();
            assert_eq!(cfg.keywords.as_deref(), Some("你好小智/大月下"));
        });
    }

    #[test]
    fn test_settings_voice_section_overrides() {
        run_with_temp_home(|_| {
            let voice = VoiceSettings {
                keywords: Some("你好小智".to_string()),
                voice: Some("news-female".to_string()),
                speed: Some(1.2),
                max_turns: Some(5),
                history_max: Some(20),
                barge_in: Some(false),
                follow_up: Some(false),
                voice_barge_in: Some(false),
                barge_in_similarity_threshold: Some(0.6),
                barge_in_threshold: Some(0.7),
                welcome_text: Some("我在呢".to_string()),
                vad_silence_threshold: Some(0.03),
                asr_max_trailing_silence: Some(2.5),
                asr_max_utterance_duration: Some(45.0),
                welcome_wait_timeout: Some(6.0),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &CliOverrides::default()).unwrap();
            assert_eq!(cfg.keywords.as_deref(), Some("你好小智"));
            assert_eq!(cfg.voice_id.as_deref(), Some("news-female"));
            assert_eq!(cfg.speed, 1.2);
            assert_eq!(cfg.max_turns, Some(5));
            assert_eq!(cfg.history_max, 20);
            assert!(!cfg.barge_in);
            assert!(!cfg.follow_up);
            assert!(!cfg.voice_barge_in);
            assert_eq!(cfg.barge_in_similarity_threshold, 0.6);
            assert_eq!(cfg.barge_in_threshold, 0.7);
            assert_eq!(cfg.welcome_text, "我在呢");
            assert_eq!(cfg.vad_silence_threshold, 0.03);
            assert_eq!(cfg.asr_max_trailing_silence, 2.5);
            assert_eq!(cfg.asr_max_utterance_duration, 45.0);
            assert_eq!(cfg.welcome_wait_timeout, 6.0);
        });
    }

    #[test]
    fn test_cli_overrides_settings() {
        run_with_temp_home(|_| {
            let voice = VoiceSettings {
                keywords: Some("settings词".to_string()),
                voice: Some("leijun-1".to_string()),
                speed: Some(0.8),
                ..Default::default()
            };
            let cli = CliOverrides {
                keywords: Some("cli词".to_string()),
                voice: Some("news-female-2".to_string()),
                speed: Some(1.5),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &cli).unwrap();
            assert_eq!(cfg.keywords.as_deref(), Some("cli词"));
            assert_eq!(cfg.voice_id.as_deref(), Some("news-female-2"));
            assert_eq!(cfg.speed, 1.5);
        });
    }

    #[test]
    fn test_cli_no_bargein_forces_off() {
        run_with_temp_home(|_| {
            // settings 缺省开，CLI --no-bargein 强制关
            let cli = CliOverrides {
                no_bargein: true,
                ..Default::default()
            };
            let cfg = resolve(None, &cli).unwrap();
            assert!(!cfg.barge_in);
        });
    }

    #[test]
    fn test_barge_in_respects_settings_when_cli_unset() {
        run_with_temp_home(|_| {
            // CLI 缺省（barge_in=true）时，settings barge_in=false 生效
            let voice = VoiceSettings {
                barge_in: Some(false),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &CliOverrides::default()).unwrap();
            assert!(!cfg.barge_in);
        });
    }

    #[test]
    fn test_cli_no_follow_up_forces_off() {
        run_with_temp_home(|_| {
            // settings 缺省开，CLI --no-follow-up 强制关
            let cli = CliOverrides {
                no_follow_up: true,
                ..Default::default()
            };
            let cfg = resolve(None, &cli).unwrap();
            assert!(!cfg.follow_up);
        });
    }

    #[test]
    fn test_follow_up_respects_settings_when_cli_unset() {
        run_with_temp_home(|_| {
            // CLI 缺省（follow_up=true）时，settings follow_up=false 生效
            let voice = VoiceSettings {
                follow_up: Some(false),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &CliOverrides::default()).unwrap();
            assert!(!cfg.follow_up);
        });
    }

    #[test]
    fn test_cli_no_voice_barge_in_forces_off() {
        run_with_temp_home(|_| {
            // settings 缺省开，CLI --no-voice-barge-in 强制关
            let cli = CliOverrides {
                no_voice_barge_in: true,
                ..Default::default()
            };
            let cfg = resolve(None, &cli).unwrap();
            assert!(!cfg.voice_barge_in);
        });
    }

    #[test]
    fn test_voice_barge_in_respects_settings_when_cli_unset() {
        run_with_temp_home(|_| {
            // CLI 缺省（no_voice_barge_in=false）时，settings voice_barge_in=false 生效
            let voice = VoiceSettings {
                voice_barge_in: Some(false),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &CliOverrides::default()).unwrap();
            assert!(!cfg.voice_barge_in);
        });
    }

    #[test]
    fn test_barge_in_similarity_threshold_from_settings() {
        run_with_temp_home(|_| {
            let voice = VoiceSettings {
                barge_in_similarity_threshold: Some(0.7),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings_with_voice(voice)), &CliOverrides::default()).unwrap();
            assert_eq!(cfg.barge_in_similarity_threshold, 0.7);
        });
    }

    #[test]
    fn test_voice_settings_serde_roundtrip() {
        run_with_temp_home(|_| {
            let voice = VoiceSettings {
                enabled: Some(true),
                keywords: Some("你好小智".to_string()),
                voice: Some("leijun-1".to_string()),
                speed: Some(1.1),
                max_turns: Some(10),
                history_max: Some(16),
                barge_in: Some(true),
                follow_up: Some(false),
                barge_in_threshold: Some(0.6),
                welcome_text: Some("你好".to_string()),
                vad_silence_threshold: Some(0.02),
                asr_max_trailing_silence: Some(3.0),
                asr_max_utterance_duration: Some(30.0),
                welcome_wait_timeout: Some(8.0),
                voice_barge_in: Some(true),
                barge_in_similarity_threshold: Some(0.55),
            };
            let app = settings_with_voice(voice);
            let toml_str = toml::to_string(&app).unwrap();
            let loaded: AppConfig = toml::from_str(&toml_str).unwrap();
            assert_eq!(loaded.voice, app.voice);
        });
    }

    /// 导入一个带人设/音色的角色包并设为 active（首次导入自动 active）。
    fn import_active_character(home: &std::path::Path) {
        let dir = home.join("furina");
        std::fs::create_dir_all(dir.join("voice")).unwrap();
        std::fs::write(dir.join("character.md"), "# 芙宁娜\n\n你是芙宁娜。\n").unwrap();
        std::fs::write(dir.join("character.png"), b"\x89PNG\r\n\x1a\n png").unwrap();
        // 单声道 wav（合法 RIFF 头即可，不触发导入时的混音改写）
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("voice/reference.wav"), spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        std::fs::write(dir.join("voice/reference.txt"), "哼~没错，就是我。").unwrap();
        crate::companion::import_character_from_dir(&dir).unwrap();
    }

    #[test]
    fn test_apply_companion_overrides_covers_persona_and_voice() {
        run_with_temp_home(|home| {
            import_active_character(home);
            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            let global_prompt = cfg.llm.system_prompt.clone();
            apply_companion_overrides(&mut cfg);
            assert!(cfg.llm.system_prompt.contains("芙宁娜"));
            assert_ne!(cfg.llm.system_prompt, global_prompt);
            // 默认 TTS 配置（ZipVoice）支持参考音频 → 注入角色音色
            let voice = cfg.character_voice.clone().unwrap();
            assert!(voice.wav.ends_with("voice/reference.wav"));
            assert_eq!(voice.text, "哼~没错，就是我。");
        });
    }

    #[test]
    fn test_apply_companion_overrides_noop_without_character() {
        run_with_temp_home(|_| {
            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            let prompt = cfg.llm.system_prompt.clone();
            apply_companion_overrides(&mut cfg);
            // 无 active 角色包 → 全局配置原样
            assert_eq!(cfg.llm.system_prompt, prompt);
            assert!(cfg.character_voice.is_none());
            assert!(cfg.welcome_clip.is_none());
        });
    }

    /// 写最小 KWS token 集（`n ǐ` 已 tokenized 序列可透传），并把 cfg 指向它。
    fn seed_tokens(cfg: &mut ResolvedSessionConfig, home: &std::path::Path) {
        let dir = home.join("kws-model");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokens.txt"), "n\nǐ\n").unwrap();
        cfg.kws.tokens = dir.join("tokens.txt");
    }

    /// 给 active 伙伴预置新鲜欢迎语（先跑一遍 override 拿到生效指纹再写资产）。
    fn seed_fresh_clip(cfg: &mut ResolvedSessionConfig) {
        let fp = crate::companion_welcome::clip_fingerprint(cfg);
        let model = crate::companion::active_model_fast().unwrap();
        let model_dir = std::path::PathBuf::from(&model.model_dir);
        std::fs::create_dir_all(model_dir.join("voice")).unwrap();
        crate::audio::write_wav_f32(
            &crate::companion_welcome::welcome_wav_path(&model_dir),
            24_000,
            &[0.1; 2_400],
        )
        .unwrap();
        let meta = crate::companion_welcome::WelcomeMeta {
            text: cfg.welcome_text.clone(),
            fingerprint: fp.clone(),
            sample_rate: 24_000,
            generated_at: "2026-08-30T00:00:00Z".to_string(),
        };
        std::fs::write(
            crate::companion_welcome::welcome_meta_path(&model_dir),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_apply_companion_overrides_injects_wake_word_and_welcome() {
        run_with_temp_home(|home| {
            import_active_character(home);
            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            seed_tokens(&mut cfg, home);
            cfg.keywords = Some("全局词".to_string());
            apply_companion_overrides(&mut cfg);
            // 角色无自定义唤醒词 → 跟随名字「芙宁娜」；中文名在 token 集外会回退，
            // 这里名字不可编码（token 集只有 n/ǐ）→ 回退全局词。
            assert_eq!(cfg.keywords.as_deref(), Some("全局词"));
            // 欢迎语覆盖为角色级默认模板。
            assert_eq!(cfg.welcome_text, "你好，我是芙宁娜。");
            // 无预生成资产 → 不注入 clip。
            assert!(cfg.welcome_clip.is_none());

            // 设可编码的自定义唤醒词 → 压过全局词。
            crate::companion::set_wake_word(
                &crate::companion::active_model_fast().unwrap().id,
                Some("n ǐ"),
            )
            .unwrap();
            apply_companion_overrides(&mut cfg);
            assert_eq!(cfg.keywords.as_deref(), Some("n ǐ"));

            // 预置新鲜资产 → 注入 clip 且指纹一致。
            seed_fresh_clip(&mut cfg);
            apply_companion_overrides(&mut cfg);
            let clip = cfg.welcome_clip.clone().unwrap();
            assert_eq!(
                clip.fingerprint,
                crate::companion_welcome::clip_fingerprint(&cfg)
            );
        });
    }

    #[test]
    fn test_apply_companion_overrides_wake_word_fallback_on_bad_name() {
        run_with_temp_home(|home| {
            import_active_character(home);
            crate::companion::set_wake_word(
                &crate::companion::active_model_fast().unwrap().id,
                Some("😂😂"),
            )
            .unwrap();
            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            seed_tokens(&mut cfg, home);
            cfg.keywords = Some("全局词".to_string());
            apply_companion_overrides(&mut cfg);
            // 角色词不可编码 → 回退全局词（欢迎语照常生效）。
            assert_eq!(cfg.keywords.as_deref(), Some("全局词"));
            assert_eq!(cfg.welcome_text, "你好，我是芙宁娜。");
        });
    }

    #[test]
    fn test_apply_companion_overrides_skips_voice_for_non_clone_model() {
        run_with_temp_home(|home| {
            import_active_character(home);
            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            // 非克隆模型（未收录二期占位 kind）→ 人设仍覆盖，音色不注入
            cfg.tts.model_type = crate::tts::config::TtsModelKind::Kitten;
            apply_companion_overrides(&mut cfg);
            assert!(cfg.llm.system_prompt.contains("芙宁娜"));
            assert!(cfg.character_voice.is_none());
        });
    }

    /// 导入一个最小合法 Live2D 伙伴（无角色包结构），返回其 id。
    fn import_live2d_companion(home: &std::path::Path) -> String {
        let dir = home.join("大月下");
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("model.moc3"), b"moc").unwrap();
        std::fs::write(dir.join("textures/texture_00.png"), b"png").unwrap();
        std::fs::write(
            dir.join("l.model3.json"),
            r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"]}}"#,
        )
        .unwrap();
        crate::companion::import_from_dir(&dir).unwrap().0.id
    }

    /// 往音色库存一条音色，返回 (id, wav_path)。
    fn save_library_voice(home: &std::path::Path) -> (String, std::path::PathBuf) {
        let wav = home.join("lib-voice.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        let v = crate::tts::voice_store::save_voice("绑定的音色", &wav, "库音色转写").unwrap();
        (v.id, v.wav_path)
    }

    /// 音色库绑定对非角色包伙伴同样注入（第 2 级解析，任意 format）。
    #[test]
    fn test_apply_companion_overrides_binding_for_live2d() {
        run_with_temp_home(|home| {
            let companion_id = import_live2d_companion(home);
            let (voice_id, wav_path) = save_library_voice(home);
            crate::companion::set_voice_binding(&companion_id, Some(&voice_id)).unwrap();

            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            assert!(cfg.character_voice.is_none());
            apply_companion_overrides(&mut cfg);
            let voice = cfg.character_voice.unwrap();
            assert_eq!(voice.wav, wav_path);
            assert_eq!(voice.text, "库音色转写");
        });
    }

    /// 绑定指向的音色被删 → fail-open：不注入，走全局默认。
    #[test]
    fn test_apply_companion_overrides_stale_binding_falls_back() {
        run_with_temp_home(|home| {
            let companion_id = import_live2d_companion(home);
            let (voice_id, _) = save_library_voice(home);
            crate::companion::set_voice_binding(&companion_id, Some(&voice_id)).unwrap();
            crate::tts::voice_store::delete_voice(&voice_id).unwrap();

            let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
            apply_companion_overrides(&mut cfg);
            assert!(cfg.character_voice.is_none());
        });
    }
}
