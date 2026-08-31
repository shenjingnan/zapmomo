//! 伙伴欢迎语预合成音频：`{model_dir}/voice/welcome.wav` + 指纹旁车 `welcome.json`。
//!
//! 唤醒后直接播放预合成 wav（零合成延迟）；旁车指纹不匹配（改文案/改音色/换
//! TTS 模型/参考音频变动）时降级走会话内实时合成，并由宿主后台重生成。
//!
//! 原子性：wav 先落盘、旁车后落盘（均 tmp + rename）——中间态「新 wav + 旧旁车」
//! 指纹不匹配只会触发降级，绝不出现「旁车说新鲜但内容是旧的」。

use crate::voice::config::ResolvedSessionConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 欢迎语预合成 wav（16-bit PCM mono，采样率 = 引擎采样率）。
pub const WELCOME_WAV: &str = "welcome.wav";
/// 指纹旁车。**不含绝对路径**：`relocate_payload` 搬迁托管目录后仍然有效；
/// `remove` 删除整个托管目录时自然清理。
pub const WELCOME_META: &str = "welcome.json";

/// 欢迎语预合成元数据（旁车 `welcome.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WelcomeMeta {
    /// 合成时实际使用的文本（清洗后；清洗为空时回退原文）。
    pub text: String,
    /// 生成时的输入指纹（`clip_fingerprint`）。
    pub fingerprint: String,
    /// 引擎采样率（wav 头里也有，冗余存储供展示/诊断）。
    pub sample_rate: u32,
    /// 生成时间（RFC3339）。
    pub generated_at: String,
}

/// 托管目录内的欢迎语 wav 路径（`voice/` 子目录与音色参考同级）。
pub fn welcome_wav_path(model_dir: &Path) -> PathBuf {
    model_dir.join("voice").join(WELCOME_WAV)
}

/// 托管目录内的欢迎语旁车路径。
pub fn welcome_meta_path(model_dir: &Path) -> PathBuf {
    model_dir.join("voice").join(WELCOME_META)
}

/// 计算欢迎语预合成的输入指纹（sha256 前 16 hex）。
///
/// 覆盖：生效欢迎语文本、TTS 模型目录/类型/后端、语速、音色 id、角色音色参考
/// wav 的 `(path, len, mtime)`。参考音频用 stat 而非内容 hash：指纹在会话启动
/// 与视图构建时反复计算，3MB 音频读盘不划算，mtime 已足够捕捉「被替换」。
pub fn clip_fingerprint(cfg: &ResolvedSessionConfig) -> String {
    let mut h = Sha256::new();
    h.update(cfg.welcome_text.as_bytes());
    h.update(cfg.tts.model_dir.to_string_lossy().as_bytes());
    h.update(format!("{:?}", cfg.tts.model_type).as_bytes());
    h.update(format!("{:?}", cfg.tts.backend).as_bytes());
    h.update(cfg.speed.to_le_bytes());
    if let Some(vid) = &cfg.voice_id {
        h.update(vid.as_bytes());
    }
    if let Some(v) = &cfg.character_voice {
        h.update(v.wav.to_string_lossy().as_bytes());
        if let Ok(meta) = std::fs::metadata(&v.wav) {
            h.update(meta.len().to_le_bytes());
            if let Ok(mtime) = meta.modified()
                && let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH)
            {
                h.update(d.as_secs().to_le_bytes());
            }
        }
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// 读取并解析旁车（缺失/损坏 → None）。
fn read_meta(model_dir: &Path) -> Option<WelcomeMeta> {
    let content = std::fs::read_to_string(welcome_meta_path(model_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

/// 预合成音频是否与期望指纹一致（只 stat，不读样本；供视图/注入判断）。
pub fn is_fresh(model_dir: &Path, expected_fp: &str) -> bool {
    read_meta(model_dir).is_some_and(|m| m.fingerprint == expected_fp)
        && welcome_wav_path(model_dir).is_file()
}

/// 唤醒时加载新鲜的预合成音频：旁车指纹匹配且 wav 可解码才返回。
///
/// 在会话线程调用，小文件（1~3s 音频）读盘毫秒级，无需预载——这样新生成/
/// 重生成的 wav 无需第二次重启会话即可被下一次唤醒使用。
pub fn load_fresh(wav: &Path, meta: &Path, expected_fp: &str) -> Option<(Vec<f32>, u32)> {
    let m: WelcomeMeta = serde_json::from_str(&std::fs::read_to_string(meta).ok()?).ok()?;
    if m.fingerprint != expected_fp {
        return None;
    }
    crate::audio::read_wav_mono(wav).ok()
}

/// 合成欢迎语 wav 并写旁车。**同步阻塞（秒级），调用方必须 spawn_blocking。**
///
/// 门控：TTS 禁用或模型文件缺失直接 `Err`（不 spawn audiocpp sidecar、不留
/// 半成品）；清洗规则与唤醒实时合成一致（清洗为空回退原文）。失败时可能留下
/// 已更新的 wav + 旧旁车（指纹不匹配 → 唤醒降级），无害。
pub fn generate(cfg: &ResolvedSessionConfig, model_dir: &Path) -> Result<WelcomeMeta, String> {
    if !cfg.tts.enabled {
        return Err("TTS 未启用，跳过欢迎语预合成".to_string());
    }
    crate::tts::config::preflight(&cfg.tts)?;
    let engine = crate::tts::TtsEngine::new(cfg.tts.clone())?;
    generate_with_engine(cfg, model_dir, &engine)
}

/// `generate` 的引擎注入核心：清洗 → 解析音色 → 合成 → 原子写盘。
///
/// 引擎由调用方提供（生产走 `TtsEngine::new`；测试注入 stub 引擎直连），
/// 其余逻辑与生产完全一致。
fn generate_with_engine(
    cfg: &ResolvedSessionConfig,
    model_dir: &Path,
    engine: &crate::tts::TtsEngine,
) -> Result<WelcomeMeta, String> {
    // 清洗为空必须回退原文——与 step_armed 同规则，避免「全 emoji 文案」合成空音频。
    let text = crate::voice::sanitizer::sanitize_for_tts(&cfg.welcome_text);
    let text = if text.is_empty() {
        cfg.welcome_text.clone()
    } else {
        text
    };
    let voice = crate::tts::voice::resolve_voice_params(
        &cfg.tts,
        cfg.voice_id.as_deref(),
        None,
        cfg.character_voice.as_ref().map(|v| v.wav.as_path()),
        cfg.character_voice.as_ref().map(|v| v.text.as_str()),
    )?;

    let voice_dir = model_dir.join("voice");
    std::fs::create_dir_all(&voice_dir).map_err(|e| format!("创建 voice 目录失败: {e}"))?;

    // wav 先落盘（原子 rename），旁车后写——顺序保证「旁车说新鲜」时 wav 必然已就绪。
    let wav_path = welcome_wav_path(model_dir);
    let tmp_wav = wav_path.with_extension("tmp.wav");
    engine
        .synthesize_to_wav(&text, cfg.speed, &voice, &tmp_wav)
        .map_err(|e| format!("合成欢迎语失败: {e}"))?;
    std::fs::rename(&tmp_wav, &wav_path).map_err(|e| format!("提交欢迎语 wav 失败: {e}"))?;

    let meta = WelcomeMeta {
        text,
        fingerprint: clip_fingerprint(cfg),
        sample_rate: engine.sample_rate().max(0) as u32,
        generated_at: crate::datetime::iso_timestamp_now(),
    };
    let meta_path = welcome_meta_path(model_dir);
    let tmp_meta = meta_path.with_extension("tmp.json");
    std::fs::write(
        &tmp_meta,
        serde_json::to_string(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入欢迎语旁车失败: {e}"))?;
    std::fs::rename(&tmp_meta, &meta_path).map_err(|e| format!("提交欢迎语旁车失败: {e}"))?;
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::companion::CharacterVoice;
    use crate::test_util::run_with_temp_home;
    use crate::voice::config::{CliOverrides, resolve};

    /// 最小会话配置（temp home 内 resolve 默认值 + 指定欢迎语文本）。
    fn session_cfg(welcome: &str) -> ResolvedSessionConfig {
        let mut cfg = resolve(None, &CliOverrides::default()).unwrap();
        cfg.welcome_text = welcome.to_string();
        cfg
    }

    /// stub 引擎用的克隆音色（路径不被 stub 读取，仅走请求体映射）。
    fn stub_voice() -> CharacterVoice {
        CharacterVoice {
            wav: PathBuf::from("/voices/stub.wav"),
            text: "stub".to_string(),
        }
    }

    /// 起一个返回固定采样率 wav 的 stub server（对齐 synthesizer.rs 的同款模式）。
    fn spawn_stub_wav(sample_rate: u32) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let samples = vec![0.2f32; (sample_rate / 10) as usize];
                let base = tempfile::tempdir().unwrap();
                let path = base.path().join("resp.wav");
                crate::audio::write_wav_f32(&path, sample_rate, &samples).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                let _ = request.respond(tiny_http::Response::from_data(bytes));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 真实 audiocpp 引擎直连 stub（无需模型文件与真实 server 进程）。
    fn stub_engine(base_url: &str) -> crate::tts::TtsEngine {
        let cfg = crate::tts::config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Qwen3Tts06,
            ..crate::tts::config::ResolvedTtsConfig::default()
        };
        crate::tts::TtsEngine::from_audiocpp_for_test(
            crate::audiocpp::client::AudiocppTts::new_with_base_url(cfg, base_url),
        )
    }

    /// 手写旁车（测试构造新鲜/过期态用）。
    fn write_meta(model_dir: &Path, fingerprint: &str) {
        let meta = WelcomeMeta {
            text: "你好，我是测试。".to_string(),
            fingerprint: fingerprint.to_string(),
            sample_rate: 24_000,
            generated_at: "2026-08-30T00:00:00Z".to_string(),
        };
        std::fs::create_dir_all(model_dir.join("voice")).unwrap();
        std::fs::write(
            welcome_meta_path(model_dir),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_clip_fingerprint_sensitive_to_inputs() {
        let mut a = session_cfg("你好，我是大月下。");
        let b = session_cfg("你好，我是大月下。");
        assert_eq!(clip_fingerprint(&a), clip_fingerprint(&b), "同输入指纹稳定");

        a.welcome_text = "换一句".to_string();
        assert_ne!(clip_fingerprint(&a), clip_fingerprint(&b), "文本敏感");
        let mut a = session_cfg("你好，我是大月下。");
        a.speed = 1.5;
        assert_ne!(clip_fingerprint(&a), clip_fingerprint(&b), "语速敏感");
        let mut a = session_cfg("你好，我是大月下。");
        a.voice_id = Some("v1".to_string());
        assert_ne!(clip_fingerprint(&a), clip_fingerprint(&b), "音色 id 敏感");
        let mut a = session_cfg("你好，我是大月下。");
        a.tts.model_dir = PathBuf::from("/other/model");
        assert_ne!(clip_fingerprint(&a), clip_fingerprint(&b), "模型目录敏感");
        let mut a = session_cfg("你好，我是大月下。");
        a.character_voice = Some(stub_voice());
        assert_ne!(clip_fingerprint(&a), clip_fingerprint(&b), "角色音色敏感");
    }

    #[test]
    fn test_clip_fingerprint_tracks_reference_wav_stat() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ref.wav");
        crate::audio::write_wav_f32(&wav, 16_000, &[0.1; 160]).unwrap();
        let mut a = session_cfg("你好");
        a.character_voice = Some(CharacterVoice {
            wav: wav.clone(),
            text: "t".to_string(),
        });
        let fp_before = clip_fingerprint(&a);
        // 重写文件改变 mtime/len → 指纹必须变（参考音频被替换的场景）。
        std::thread::sleep(std::time::Duration::from_millis(20));
        crate::audio::write_wav_f32(&wav, 16_000, &[0.1; 320]).unwrap();
        assert_ne!(clip_fingerprint(&a), fp_before, "参考 wav 替换敏感");
    }

    #[test]
    fn test_is_fresh_and_load_fresh_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("m");
        std::fs::create_dir_all(model_dir.join("voice")).unwrap();
        let samples = vec![0.3f32; 2_400];
        crate::audio::write_wav_f32(&welcome_wav_path(&model_dir), 24_000, &samples).unwrap();
        write_meta(&model_dir, "abc123");

        assert!(is_fresh(&model_dir, "abc123"));
        assert!(!is_fresh(&model_dir, "other"), "指纹不匹配 → 不新鲜");
        let wav = welcome_wav_path(&model_dir);
        let meta_path = welcome_meta_path(&model_dir);
        let (loaded, rate) = load_fresh(&wav, &meta_path, "abc123").unwrap();
        assert_eq!(rate, 24_000);
        assert_eq!(loaded.len(), 2_400);
        assert!(load_fresh(&wav, &meta_path, "x").is_none());
    }

    #[test]
    fn test_is_fresh_rejects_missing_or_broken() {
        let dir = tempfile::tempdir().unwrap();
        let model_dir = dir.path().join("m");
        std::fs::create_dir_all(model_dir.join("voice")).unwrap();
        // 缺 wav + 缺 meta。
        assert!(!is_fresh(&model_dir, "fp"));
        // 有 meta 无 wav。
        write_meta(&model_dir, "fp");
        assert!(!is_fresh(&model_dir, "fp"));
        // 有 wav 有 meta 但 wav 损坏 → load_fresh None。
        std::fs::write(welcome_wav_path(&model_dir), b"not a wav").unwrap();
        let wav = welcome_wav_path(&model_dir);
        let meta_path = welcome_meta_path(&model_dir);
        assert!(load_fresh(&wav, &meta_path, "fp").is_none());
    }

    #[test]
    fn test_generate_gates_on_disabled_or_missing_model() {
        run_with_temp_home(|_home| {
            let dir = tempfile::tempdir().unwrap();
            let model_dir = dir.path().join("m");
            let mut cfg = session_cfg("你好");
            cfg.tts.enabled = false;
            assert!(generate(&cfg, &model_dir).is_err(), "TTS 禁用必须拒绝");
            assert!(!welcome_wav_path(&model_dir).exists(), "门控拒绝不留半成品");

            cfg.tts.enabled = true;
            // 默认（sherpa）模型在 temp home 下不存在 → preflight 失败。
            assert!(generate(&cfg, &model_dir).is_err(), "模型缺失必须拒绝");
            assert!(!welcome_wav_path(&model_dir).exists());
        });
    }

    #[test]
    fn test_generate_with_stub_engine_end_to_end() {
        run_with_temp_home(|_home| {
            let url = spawn_stub_wav(24_000);
            let engine = stub_engine(&url);
            let dir = tempfile::tempdir().unwrap();
            let model_dir = dir.path().join("m");
            let mut cfg = session_cfg("你好，我是大月下！");
            cfg.character_voice = Some(stub_voice());

            let meta = generate_with_engine(&cfg, &model_dir, &engine).unwrap();
            assert_eq!(meta.sample_rate, 24_000);
            assert_eq!(meta.fingerprint, clip_fingerprint(&cfg));
            // 中文标点属可朗读内容，sanitize 保留（剥的是 markdown/emoji/fence）。
            assert_eq!(meta.text, "你好，我是大月下！");
            assert!(is_fresh(&model_dir, &meta.fingerprint));
            let wav = welcome_wav_path(&model_dir);
            let meta_path = welcome_meta_path(&model_dir);
            let (samples, rate) = load_fresh(&wav, &meta_path, &meta.fingerprint).unwrap();
            assert_eq!(rate, 24_000);
            assert!(!samples.is_empty());
            assert!(!wav.with_extension("tmp.wav").exists(), "tmp 文件应已提交");
        });
    }

    #[test]
    fn test_generate_falls_back_to_raw_text_when_sanitized_empty() {
        run_with_temp_home(|_home| {
            let url = spawn_stub_wav(16_000);
            let engine = stub_engine(&url);
            let dir = tempfile::tempdir().unwrap();
            let mut cfg = session_cfg("😂😂");
            cfg.character_voice = Some(stub_voice());
            let meta = generate_with_engine(&cfg, &dir.path().join("m"), &engine).unwrap();
            assert_eq!(
                meta.text, "😂😂",
                "清洗为空必须回退原文（与 step_armed 同规则）"
            );
        });
    }
}
