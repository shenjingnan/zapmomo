/// TTS 音色（参考音色）列表。
///
/// ZipVoice 是零样本声音克隆模型，音色 = 参考音频 + 参考文本。内置音色来自
/// 模型包内 `test_wavs/prompt.txt`（每行 `<wav文件名> <转写文本>`），运行时解析。
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::tts::config::ResolvedTtsConfig;

/// 一个可用音色。
///
/// 两种语义：ZipVoice 参考音色（`wav_path`/`reference_text` 有效）与 Kokoro 等 sid
/// 模型的预置音色（`sid`/`group` 有效，`wav_path` 为空、`reference_text` 为空串）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TtsVoice {
    /// 唯一标识（wav 文件名去 `.wav` 后缀，如 `leijun-1`；Kokoro 为音色名如 `zf_001`）。
    pub id: String,
    /// 显示名（内置音色有友好中文名，否则用 id）。
    pub name: String,
    /// 参考音频绝对路径（Kokoro 等 sid 音色为空路径）。
    pub wav_path: PathBuf,
    /// 参考音频的逐字转写文本（Kokoro 等 sid 音色为空串）。
    pub reference_text: String,
    /// 是否为用户自定义音色（true = 来自音色库，false = 模型包内置）。
    pub custom: bool,
    /// sid 模型（Kokoro）音色的 speaker id；参考音色（zipvoice）为 None。
    pub sid: Option<i32>,
    /// Kokoro 音色分组（前端分组下拉）；参考音色为 None。
    pub group: Option<crate::tts::kokoro_voices::KokoroVoiceGroup>,
}

/// 内置音色的友好中文名（prompt.txt 只有文件名，这里做一层展示映射）。
fn friendly_name(id: &str) -> String {
    match id {
        "leijun-1" => "雷军（男）".to_string(),
        "news-female" => "新闻女声".to_string(),
        "news-female-2" => "新闻女声 2".to_string(),
        _ => id.to_string(),
    }
}

/// 解析 `test_wavs/prompt.txt` 的一行。
fn parse_prompt_line(line: &str, model_dir: &Path) -> Option<TtsVoice> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let (wav_name, text) = line.split_once(' ')?;
    let wav_name = wav_name.trim();
    let text = text.trim();
    if wav_name.is_empty() || text.is_empty() || !wav_name.ends_with(".wav") {
        return None;
    }
    let id = wav_name.trim_end_matches(".wav").to_string();
    Some(TtsVoice {
        name: friendly_name(&id),
        id,
        wav_path: model_dir.join("test_wavs").join(wav_name),
        reference_text: text.to_string(),
        custom: false,
        sid: None,
        group: None,
    })
}

/// 列出模型包内置的参考音色（解析 `<model_dir>/test_wavs/prompt.txt`）。
pub fn list_builtin_voices(model_dir: &Path) -> Vec<TtsVoice> {
    let prompt = model_dir.join("test_wavs").join("prompt.txt");
    let Ok(content) = std::fs::read_to_string(&prompt) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| parse_prompt_line(line, model_dir))
        .collect()
}

/// 按 id 从音色列表中查找。
pub fn find_voice<'a>(voices: &'a [TtsVoice], id: &str) -> Option<&'a TtsVoice> {
    voices.iter().find(|v| v.id == id)
}

/// 解析最终参考音色：自定义 wav > 自定义音色（id/名称）> 内置音色 id > 配置默认。
///
/// 音色 id 优先级：显式传入的 `voice_id` 优先于配置默认音色（`cfg.voice`，即
/// `[tts].voice`），再回退 `cfg.reference_wav`（leijun）。因此设置「默认音色」后，
/// 所有不显式指定音色的合成（测试语音 / 语音会话 / CLI tts run）都会统一使用该默认音色。
pub fn resolve_reference(
    cfg: &ResolvedTtsConfig,
    voice_id: Option<&str>,
    custom_wav: Option<&Path>,
    custom_text: Option<&str>,
) -> Result<(PathBuf, String), String> {
    if let Some(wav) = custom_wav {
        let text = custom_text
            .ok_or_else(|| "自定义参考音频必须同时提供参考文本（逐字转写）".to_string())?;
        return Ok((wav.to_path_buf(), text.to_string()));
    }
    let id = voice_id.or(cfg.voice.as_deref());
    if let Some(id) = id {
        // 优先匹配用户自定义音色（音色库，支持按 id 或展示名）
        if let Some(v) = crate::tts::voice_store::list_custom_voices()
            .into_iter()
            .find(|v| v.id == id || v.name == id)
        {
            return Ok((v.wav_path, v.reference_text));
        }
        // 再匹配模型包内置音色
        let voices = list_builtin_voices(&cfg.model_dir);
        let v = find_voice(&voices, id).ok_or_else(|| format!("未找到音色: {id}"))?;
        return Ok((v.wav_path.clone(), v.reference_text.clone()));
    }
    Ok((cfg.reference_wav.clone(), cfg.reference_text.clone()))
}

/// sid 模型（Kokoro / VITS / Matcha）的说话人解析统一入口。
///
/// 优先级：显式数字 `sid` > 音色名 `voice_id`（或配置默认 `cfg.voice`）> 模型默认。
/// - Kokoro：名字查 `KOKORO_VOICES`（未知名报错提示拼写），sid 钳界 0..=102，
///   无任何指定时用默认音色 `zf_001`（中文女声）。
/// - VITS / Matcha：单说话人模型，恒 0（显式非负 sid 可覆盖）。
///
/// 与 [`resolve_reference`]（zipvoice 参考音频语义）并列，是 sid 模型链路
/// （CLI / 语音会话 / GUI 合成）共用的音色决策点。
pub fn resolve_sid_voice(
    cfg: &ResolvedTtsConfig,
    voice_id: Option<&str>,
    sid: Option<i32>,
) -> Result<crate::tts::TtsVoiceParams, String> {
    use crate::tts::config::TtsModelKind;
    let explicit_name = voice_id.or(cfg.voice.as_deref());
    match cfg.model_type {
        TtsModelKind::Kokoro => {
            let resolved = match (sid, explicit_name) {
                (Some(s), _) => crate::tts::kokoro_voices::normalize_sid(s),
                (None, Some(name)) => crate::tts::kokoro_voices::sid_by_name(name)
                    .ok_or_else(|| format!("未找到 Kokoro 音色: {name}"))?,
                (None, None) => crate::tts::kokoro_voices::KOKORO_DEFAULT_SID,
            };
            Ok(crate::tts::TtsVoiceParams::Sid(resolved))
        }
        _ => Ok(crate::tts::TtsVoiceParams::Sid(sid.unwrap_or(0).max(0))),
    }
}

/// 合成音色参数的统一解析入口（backend + 模型族感知）。
///
/// 收敛此前散落 4 处（语音会话 / dsh 播报 / GUI 合成 / CLI speak）的同构
/// 分支逻辑。三档语义：
/// - 参考音频克隆（sherpa zipvoice / audiocpp omnivoice / audiocpp qwen3_tts）：
///   显式音色（`voice_id` > `cfg.voice`）或自定义 wav → `resolve_reference`；
///   omnivoice 无任何音色时不回退模型包默认参考（包内无 test_wavs），
///   返回 `Sid(0)`——client 侧省略音色字段，走 server auto voice；
///   qwen3_tts Base 无音色时报错（上游无 auto voice，必须克隆音色）；
/// - audiocpp 固定具名音色族（pocket）：`Named(voice_id > cfg.voice > 族默认)`；
/// - 其余（kokoro/vits/matcha/kitten）：`resolve_sid_voice`。
pub fn resolve_voice_params(
    cfg: &ResolvedTtsConfig,
    voice_id: Option<&str>,
    sid: Option<i32>,
    custom_wav: Option<&Path>,
    custom_text: Option<&str>,
) -> Result<crate::tts::TtsVoiceParams, String> {
    use crate::tts::TtsVoiceParams;
    use crate::tts::config::TtsBackendKind;
    if cfg.uses_reference_audio() {
        let has_any = voice_id.is_some() || cfg.voice.is_some() || custom_wav.is_some();
        if !has_any && cfg.backend == TtsBackendKind::Audiocpp {
            // 按族分派无音色兜底：omnivoice → auto voice（Sid(0)，client 省略
            // 音色字段）；qwen3_tts Base 上游无 auto voice → 提前报错
            let desc = crate::audiocpp::families::family_desc(cfg.model_type);
            if desc.is_some_and(|d| {
                matches!(
                    d.voice_semantics,
                    crate::audiocpp::families::VoiceSemantics::ReferenceCloneRequired
                )
            }) {
                return Err("Qwen3-TTS 需要克隆音色：请先在音色库选择或录制一个音色".to_string());
            }
            return Ok(TtsVoiceParams::Sid(0));
        }
        let (wav, text) = resolve_reference(cfg, voice_id, custom_wav, custom_text)?;
        Ok(TtsVoiceParams::Reference {
            wav_path: wav,
            reference_text: text,
        })
    } else if cfg.backend == TtsBackendKind::Audiocpp {
        let desc = crate::audiocpp::families::family_desc(cfg.model_type)
            .ok_or_else(|| format!("模型类型 {} 不支持 audiocpp 后端", cfg.model_type.as_str()))?;
        let crate::audiocpp::families::VoiceSemantics::FixedNamed(default) = desc.voice_semantics
        else {
            // 克隆族已被 uses_reference_audio() 分支覆盖，此处防御
            return Ok(TtsVoiceParams::Sid(0));
        };
        Ok(TtsVoiceParams::Named(
            voice_id
                .or(cfg.voice.as_deref())
                .unwrap_or(default)
                .to_string(),
        ))
    } else {
        resolve_sid_voice(cfg, voice_id, sid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_prompt(model_dir: &Path, content: &str) {
        std::fs::create_dir_all(model_dir.join("test_wavs")).unwrap();
        std::fs::write(model_dir.join("test_wavs/prompt.txt"), content).unwrap();
    }

    #[test]
    fn test_list_builtin_voices_parses_prompt() {
        let dir = tempfile::tempdir().unwrap();
        make_prompt(
            dir.path(),
            "leijun-1.wav 那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.\n\
             news-female.wav 各位村民, 大家新年好! 近期, 湖北省武汉市等多个地区\n\
             news-female-2.wav 本台消息, 中共中央国务院, 近日印发关于构建数据基础制度.\n",
        );
        let voices = list_builtin_voices(dir.path());
        assert_eq!(voices.len(), 3);

        let leijun = find_voice(&voices, "leijun-1").unwrap();
        assert_eq!(leijun.name, "雷军（男）");
        assert_eq!(leijun.wav_path, dir.path().join("test_wavs/leijun-1.wav"));
        assert!(leijun.reference_text.contains("计算机系"));

        let news = find_voice(&voices, "news-female").unwrap();
        assert_eq!(news.name, "新闻女声");
    }

    #[test]
    fn test_list_builtin_voices_skips_invalid_lines() {
        let dir = tempfile::tempdir().unwrap();
        make_prompt(
            dir.path(),
            "\n\nmissing-text.wav\nno-extension 文本\nleijun-1.wav 有效的参考文本\n",
        );
        let voices = list_builtin_voices(dir.path());
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].id, "leijun-1");
    }

    #[test]
    fn test_list_builtin_voices_missing_prompt_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let voices = list_builtin_voices(dir.path());
        assert!(voices.is_empty());
    }

    /// 生成一个合法最小 wav（RIFF 头 + 少量样本），满足 `voice_store::save_voice` 校验。
    fn sample_wav_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&44u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&16000u32.to_le_bytes());
        buf.extend_from_slice(&32000u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&8u32.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf
    }

    #[test]
    fn test_resolve_reference_custom_voice_by_name() {
        crate::test_util::run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = crate::tts::voice_store::save_voice("大月下", &src, "为什么人类要起这么早啊")
                .unwrap();

            let cfg = ResolvedTtsConfig::default();
            let (wav, text) = resolve_reference(&cfg, Some("大月下"), None, None).unwrap();
            assert_eq!(wav, v.wav_path);
            assert_eq!(text, "为什么人类要起这么早啊");
        });
    }

    #[test]
    fn test_resolve_reference_custom_voice_by_id() {
        crate::test_util::run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = crate::tts::voice_store::save_voice("大月下", &src, "参考文本").unwrap();

            let cfg = ResolvedTtsConfig::default();
            let (wav, text) = resolve_reference(&cfg, Some(&v.id), None, None).unwrap();
            assert_eq!(wav, v.wav_path);
            assert_eq!(text, "参考文本");
        });
    }

    #[test]
    fn test_resolve_reference_default_voice_custom_when_no_voice_id() {
        // 配置了默认音色（[tts].voice = 自定义音色 id），不显式传 voice_id → 用默认自定义音色
        crate::test_util::run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = crate::tts::voice_store::save_voice("我的声音", &src, "参考文本").unwrap();

            let cfg = ResolvedTtsConfig {
                voice: Some(v.id.clone()),
                ..ResolvedTtsConfig::default()
            };
            let (wav, text) = resolve_reference(&cfg, None, None, None).unwrap();
            assert_eq!(wav, v.wav_path);
            assert_eq!(text, "参考文本");
        });
    }

    #[test]
    fn test_resolve_reference_default_voice_builtin_when_no_voice_id() {
        // 配置了默认音色（内置 id），不显式传 voice_id → 用默认内置音色
        let dir = tempfile::tempdir().unwrap();
        make_prompt(dir.path(), "news-female.wav 各位村民, 大家新年好!\n");
        let cfg = ResolvedTtsConfig {
            model_dir: dir.path().to_path_buf(),
            voice: Some("news-female".to_string()),
            ..Default::default()
        };
        let (wav, text) = resolve_reference(&cfg, None, None, None).unwrap();
        assert_eq!(wav, dir.path().join("test_wavs/news-female.wav"));
        assert!(text.contains("大家新年好"));
    }

    #[test]
    fn test_resolve_reference_explicit_voice_id_overrides_default() {
        // 显式传 voice_id 优先于配置默认音色（默认是 news-female，显式选 leijun）
        let dir = tempfile::tempdir().unwrap();
        make_prompt(
            dir.path(),
            "leijun-1.wav 那还是36年前.\nnews-female.wav 各位村民!\n",
        );
        let cfg = ResolvedTtsConfig {
            model_dir: dir.path().to_path_buf(),
            voice: Some("news-female".to_string()),
            ..Default::default()
        };
        let (wav, _) = resolve_reference(&cfg, Some("leijun-1"), None, None).unwrap();
        assert_eq!(wav, dir.path().join("test_wavs/leijun-1.wav"));
    }

    #[test]
    fn test_resolve_reference_builtin_still_works() {
        let dir = tempfile::tempdir().unwrap();
        make_prompt(dir.path(), "leijun-1.wav 那还是36年前, 1987年.\n");
        let cfg = ResolvedTtsConfig {
            model_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let (wav, text) = resolve_reference(&cfg, Some("leijun-1"), None, None).unwrap();
        assert_eq!(wav, dir.path().join("test_wavs/leijun-1.wav"));
        assert!(text.contains("1987年"));
    }

    #[test]
    fn test_resolve_reference_unknown_voice_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ResolvedTtsConfig {
            model_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let err = resolve_reference(&cfg, Some("不存在的音色"), None, None).unwrap_err();
        assert!(err.contains("未找到音色"), "err: {err}");
    }

    #[test]
    fn test_resolve_reference_custom_wav_requires_text() {
        let cfg = ResolvedTtsConfig::default();
        let err = resolve_reference(&cfg, None, Some(Path::new("/tmp/a.wav")), None).unwrap_err();
        assert!(err.contains("参考文本"), "err: {err}");
    }

    fn kokoro_cfg() -> ResolvedTtsConfig {
        let mut cfg = ResolvedTtsConfig::default();
        cfg.model_type = crate::tts::config::TtsModelKind::Kokoro;
        cfg
    }

    #[test]
    fn test_resolve_sid_voice_kokoro_priority_matrix() {
        use crate::tts::TtsVoiceParams;
        // 显式 sid 最优先（含越界钳界回落默认 zf_001=3）
        assert!(matches!(
            resolve_sid_voice(&kokoro_cfg(), Some("zf_050"), Some(58)),
            Ok(TtsVoiceParams::Sid(58))
        ));
        assert!(matches!(
            resolve_sid_voice(&kokoro_cfg(), None, Some(999)),
            Ok(TtsVoiceParams::Sid(3))
        ));
        // 无 sid 时名字查表（voice_id > cfg.voice）
        assert!(matches!(
            resolve_sid_voice(&kokoro_cfg(), Some("zm_010"), None),
            Ok(TtsVoiceParams::Sid(59))
        ));
        let mut cfg_with_voice = kokoro_cfg();
        cfg_with_voice.voice = Some("zf_099".to_string());
        assert!(matches!(
            resolve_sid_voice(&cfg_with_voice, None, None),
            Ok(TtsVoiceParams::Sid(57))
        ));
        assert!(matches!(
            resolve_sid_voice(&cfg_with_voice, Some("af_maple"), None),
            Ok(TtsVoiceParams::Sid(0))
        ));
        // 全无 → 模型默认 zf_001（sid 3）
        assert!(matches!(
            resolve_sid_voice(&kokoro_cfg(), None, None),
            Ok(TtsVoiceParams::Sid(3))
        ));
        // 未知名报错
        let err = resolve_sid_voice(&kokoro_cfg(), Some("leijun-1"), None).unwrap_err();
        assert!(err.contains("未找到 Kokoro 音色"), "err: {err}");
    }

    #[test]
    fn test_resolve_sid_voice_single_speaker_models_stay_zero() {
        use crate::tts::TtsVoiceParams;
        for kind in [
            crate::tts::config::TtsModelKind::Vits,
            crate::tts::config::TtsModelKind::Matcha,
        ] {
            let mut cfg = ResolvedTtsConfig::default();
            cfg.model_type = kind;
            // 无任何指定 → 0
            assert!(matches!(
                resolve_sid_voice(&cfg, None, None),
                Ok(TtsVoiceParams::Sid(0))
            ));
            // 显式 sid 可覆盖；负数钳到 0
            assert!(matches!(
                resolve_sid_voice(&cfg, None, Some(2)),
                Ok(TtsVoiceParams::Sid(2))
            ));
            assert!(matches!(
                resolve_sid_voice(&cfg, None, Some(-1)),
                Ok(TtsVoiceParams::Sid(0))
            ));
        }
    }

    fn audiocpp_cfg(kind: crate::tts::config::TtsModelKind) -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: kind,
            ..ResolvedTtsConfig::default()
        }
    }

    /// omnivoice（克隆族）：自定义音色库命中 → Reference；无任何音色 → Sid(0)
    /// （client 省略音色字段走 auto voice，不回退模型包默认参考）。
    #[test]
    fn test_resolve_voice_params_omnivoice() {
        crate::test_util::run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = crate::tts::voice_store::save_voice("我的声音", &src, "参考转写").unwrap();

            let cfg = audiocpp_cfg(crate::tts::config::TtsModelKind::Omnivoice);
            // 显式音色 id → Reference（voice_store 命中）
            let out = resolve_voice_params(&cfg, Some(&v.id), None, None, None).unwrap();
            let crate::tts::TtsVoiceParams::Reference { wav_path, .. } = out else {
                panic!("应为 Reference: {out:?}");
            };
            assert_eq!(wav_path, v.wav_path);
            // 自定义 wav + 转写 → Reference
            let out =
                resolve_voice_params(&cfg, None, None, Some(Path::new("/tmp/x.wav")), Some("t"))
                    .unwrap();
            assert!(matches!(out, crate::tts::TtsVoiceParams::Reference { .. }));
            // 无任何音色 → Sid(0)（auto voice 语义）
            let out = resolve_voice_params(&cfg, None, None, None, None).unwrap();
            assert!(matches!(out, crate::tts::TtsVoiceParams::Sid(0)));
        });
    }

    /// pocket（固定音色族）：voice_id/cfg.voice/族默认三级回退 → Named。
    #[test]
    fn test_resolve_voice_params_pocket() {
        let cfg = audiocpp_cfg(crate::tts::config::TtsModelKind::Pocket);
        let out = resolve_voice_params(&cfg, None, None, None, None).unwrap();
        assert!(matches!(
            out,
            crate::tts::TtsVoiceParams::Named(ref v) if v == "alba"
        ));
        let out = resolve_voice_params(&cfg, Some("cosette"), None, None, None).unwrap();
        assert!(matches!(
            out,
            crate::tts::TtsVoiceParams::Named(ref v) if v == "cosette"
        ));
    }

    /// qwen3_tts 无音色来源时明确报错（omnivoice 走 Sid(0) auto voice，qwen3 不能）。
    #[test]
    fn test_resolve_voice_params_qwen3_requires_voice() {
        let cfg = audiocpp_cfg(crate::tts::config::TtsModelKind::Qwen3Tts06);
        let err = resolve_voice_params(&cfg, None, None, None, None).unwrap_err();
        assert!(err.contains("克隆音色"), "err: {err}");

        // 有自定义音色 -> Reference
        let base = tempfile::tempdir().unwrap();
        let wav = base.path().join("my.wav");
        std::fs::write(&wav, sample_wav_bytes()).unwrap();
        let params = resolve_voice_params(&cfg, None, None, Some(&wav), Some("转写")).unwrap();
        match params {
            crate::tts::TtsVoiceParams::Reference {
                wav_path,
                reference_text,
            } => {
                assert_eq!(wav_path, wav);
                assert_eq!(reference_text, "转写");
            }
            other => panic!("应为 Reference，got {other:?}"),
        }
    }

    /// sherpa 族（zipvoice/kokoro）经由统一入口行为不变（旁路 sid/reference 解析）。
    #[test]
    fn test_resolve_voice_params_sherpa_passthrough() {
        let cfg = ResolvedTtsConfig::default(); // zipvoice + sherpa
        let out = resolve_voice_params(&cfg, None, None, None, None).unwrap();
        assert!(matches!(out, crate::tts::TtsVoiceParams::Reference { .. }));
        let out = resolve_voice_params(&kokoro_cfg(), Some("zf_099"), None, None, None).unwrap();
        assert!(matches!(out, crate::tts::TtsVoiceParams::Sid(_)));
    }
}
