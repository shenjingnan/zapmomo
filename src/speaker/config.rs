//! 声纹识别配置解析。
//!
//! 把 `settings.toml` 的 `[speaker]` 段（全 `Option`）收敛为已填默认值的
//! [`ResolvedSpeakerConfig`]，模式与 `kws::config` / `asr::config` 一致。
//! 解析层保持**无副作用**（不下载、不建目录、不加载模型）——模型 ensure
//! 见 [`crate::speaker`]。

use std::path::{Path, PathBuf};

use crate::config::settings::{SpeakerSettings, get_models_dir, resolve_env_ref};
use crate::kws::model;

/// 声纹模型输入采样率（CAM++ 系列为 16k 单声道，输入会先重采样到此）。
pub const SPEAKER_SAMPLE_RATE: i32 = 16_000;

/// 说话人相似度判定阈值（余弦相似度，sherpa-onnx 官方示例值）。
pub const DEFAULT_THRESHOLD: f32 = 0.6;

/// 参与识别的最短语音时长（秒），低于则跳过（防「嗯/啊」短促声误识别）。
pub const DEFAULT_MIN_AUDIO_DURATION_SECS: f32 = 1.0;

/// 实时会话声纹缓冲上限（秒），超长截断保留最近音频。
pub const DEFAULT_MAX_BUFFER_DURATION_SECS: f32 = 20.0;

/// 解析后的完整声纹识别配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSpeakerConfig {
    /// 是否在语音会话中启用声纹识别，缺省 false
    pub enabled: bool,
    /// 声纹模型目录（已展开 `${env.VAR}`，相对路径锚定 `~/.zapmomo`）
    pub model_dir: PathBuf,
    /// embedding 模型 onnx 文件名；`None` = 运行时按目录内容探测（见 [`ResolvedSpeakerConfig::model_path`]）
    pub model: Option<String>,
    /// 说话人相似度判定阈值（余弦相似度）
    pub threshold: f32,
    /// 参与识别的最短语音时长（秒）
    pub min_audio_duration_secs: f32,
    /// 推理后端
    pub provider: String,
    /// 推理线程数
    pub num_threads: i32,
    /// 模型缺失时是否自动下载
    pub auto_download: bool,
    /// 实时会话声纹缓冲上限（秒）
    pub max_buffer_duration_secs: f32,
    /// 调试输出
    pub debug: bool,
}

impl Default for ResolvedSpeakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_dir: default_model_dir(),
            model: None,
            threshold: DEFAULT_THRESHOLD,
            min_audio_duration_secs: DEFAULT_MIN_AUDIO_DURATION_SECS,
            provider: "cpu".to_string(),
            num_threads: 1,
            auto_download: true,
            max_buffer_duration_secs: DEFAULT_MAX_BUFFER_DURATION_SECS,
            debug: false,
        }
    }
}

impl ResolvedSpeakerConfig {
    /// 解析出的模型文件路径（`model` 未配置时按目录内容探测，探测不到返回 `None`）。
    pub fn model_path(&self) -> Option<PathBuf> {
        let name = match &self.model {
            Some(m) => m.clone(),
            None => detect_default_model(&self.model_dir)?,
        };
        Some(self.model_dir.join(name))
    }
}

/// 用户默认模型目录：`~/.zapmomo/models/<模型名>`。
pub fn default_model_dir() -> PathBuf {
    get_models_dir().join(&model::speaker_asset().name)
}

/// onnx 默认文件名探测：settings 未显式配置时按模型目录内容选择。
///
/// 规则（确定性）：文件名含 `campplus`、`.onnx` 结尾的按字母序取第一个；
/// 目录不存在或无匹配 → `None`（后续 ensure 走自动下载，错误路径清晰）。
fn detect_default_model(model_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(model_dir).ok()?;
    let mut candidates: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.to_ascii_lowercase().contains("campplus") && n.ends_with(".onnx"))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// 解析模型目录：settings（支持 `${env.VAR}`，相对路径锚定 `~/.zapmomo`）> 默认。
fn resolve_model_dir(settings: Option<&SpeakerSettings>) -> Result<PathBuf, String> {
    match settings.and_then(|s| s.model_dir.as_deref()) {
        Some(dir) => {
            let expanded = resolve_env_ref(dir)?;
            let p = PathBuf::from(expanded);
            Ok(if p.is_absolute() {
                p
            } else {
                crate::config::settings::get_settings_dir().join(p)
            })
        }
        None => Ok(default_model_dir()),
    }
}

/// 合并配置并填充默认值。
pub fn resolve(settings: Option<&SpeakerSettings>) -> Result<ResolvedSpeakerConfig, String> {
    let mut cfg = ResolvedSpeakerConfig {
        model_dir: resolve_model_dir(settings)?,
        ..ResolvedSpeakerConfig::default()
    };
    let s = settings;
    cfg.enabled = s.and_then(|s| s.enabled).unwrap_or(false);
    cfg.model = s.and_then(|s| s.model.clone());
    cfg.threshold = s.and_then(|s| s.threshold).unwrap_or(DEFAULT_THRESHOLD);
    cfg.min_audio_duration_secs = s
        .and_then(|s| s.min_audio_duration_secs)
        .unwrap_or(DEFAULT_MIN_AUDIO_DURATION_SECS);
    cfg.provider = s
        .and_then(|s| s.provider.clone())
        .unwrap_or_else(|| "cpu".to_string());
    cfg.num_threads = s.and_then(|s| s.num_threads).unwrap_or(1);
    cfg.auto_download = s.and_then(|s| s.auto_download).unwrap_or(true);
    cfg.max_buffer_duration_secs = s
        .and_then(|s| s.max_buffer_duration_secs)
        .unwrap_or(DEFAULT_MAX_BUFFER_DURATION_SECS);
    cfg.debug = s.and_then(|s| s.debug).unwrap_or(false);
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    #[test]
    fn test_resolve_defaults() {
        run_with_temp_home(|_home| {
            let cfg = resolve(None).unwrap();
            assert_eq!(cfg, ResolvedSpeakerConfig::default());
            assert!(!cfg.enabled);
            assert_eq!(cfg.threshold, 0.6);
            assert_eq!(cfg.min_audio_duration_secs, 1.0);
            assert_eq!(cfg.num_threads, 1);
            assert_eq!(cfg.provider, "cpu");
            assert!(cfg.auto_download);
            assert_eq!(cfg.max_buffer_duration_secs, 20.0);
            assert_eq!(cfg.model_dir, default_model_dir());
            assert_eq!(cfg.model, None);
        });
    }

    #[test]
    fn test_resolve_overrides_from_settings() {
        run_with_temp_home(|_home| {
            let settings = SpeakerSettings {
                enabled: Some(true),
                model_dir: Some("/tmp/my-models".to_string()),
                model: Some("my.onnx".to_string()),
                threshold: Some(0.75),
                min_audio_duration_secs: Some(0.5),
                provider: Some("coreml".to_string()),
                num_threads: Some(4),
                auto_download: Some(false),
                max_buffer_duration_secs: Some(10.0),
                debug: Some(true),
            };
            let cfg = resolve(Some(&settings)).unwrap();
            assert!(cfg.enabled);
            assert_eq!(cfg.model_dir, PathBuf::from("/tmp/my-models"));
            assert_eq!(cfg.model.as_deref(), Some("my.onnx"));
            assert_eq!(
                cfg.model_path(),
                Some(PathBuf::from("/tmp/my-models/my.onnx"))
            );
            assert!((cfg.threshold - 0.75).abs() < f32::EPSILON);
            assert!((cfg.min_audio_duration_secs - 0.5).abs() < f32::EPSILON);
            assert_eq!(cfg.provider, "coreml");
            assert_eq!(cfg.num_threads, 4);
            assert!(!cfg.auto_download);
            assert!((cfg.max_buffer_duration_secs - 10.0).abs() < f32::EPSILON);
            assert!(cfg.debug);
        });
    }

    #[test]
    fn test_resolve_relative_model_dir_anchored_to_settings_dir() {
        run_with_temp_home(|_home| {
            let settings = SpeakerSettings {
                model_dir: Some("speaker-models".to_string()),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings)).unwrap();
            assert_eq!(
                cfg.model_dir,
                crate::config::settings::get_settings_dir().join("speaker-models")
            );
        });
    }

    #[test]
    fn test_model_path_detects_campplus_onnx() {
        run_with_temp_home(|_home| {
            let dir = std::env::temp_dir().join(format!("zapmomo-spk-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"),
                b"x",
            )
            .unwrap();
            std::fs::write(dir.join("readme.txt"), b"x").unwrap();
            let cfg = ResolvedSpeakerConfig {
                model_dir: dir.clone(),
                ..ResolvedSpeakerConfig::default()
            };
            assert_eq!(
                cfg.model_path(),
                Some(dir.join("3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx"))
            );
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }

    #[test]
    fn test_model_path_none_when_missing() {
        run_with_temp_home(|_home| {
            let cfg = ResolvedSpeakerConfig {
                model_dir: PathBuf::from("/nonexistent-zapmomo-speaker"),
                ..ResolvedSpeakerConfig::default()
            };
            assert_eq!(cfg.model_path(), None);
        });
    }

    #[test]
    fn test_settings_toml_roundtrip() {
        run_with_temp_home(|home| {
            let dir = home.join(".zapmomo");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("settings.toml"),
                "[speaker]\nenabled = true\nthreshold = 0.7\n",
            )
            .unwrap();
            let config = crate::config::settings::load_settings().unwrap().unwrap();
            let cfg = resolve(config.speaker.as_ref()).unwrap();
            assert!(cfg.enabled);
            assert!((cfg.threshold - 0.7).abs() < f32::EPSILON);
        });
    }
}
