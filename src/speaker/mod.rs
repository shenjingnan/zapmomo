//! 声纹识别（Speaker Recognition）。
//!
//! 基于声音本身（speaker embedding，不依赖 ASR 文本）区分说话人：
//! 注册（enrollment）、验证（1:1 verify）、识别（1:N identify）、JSON 持久化。
//! 默认模型为 sherpa-onnx 官方 3D-Speaker CAM++（中文 16k，192 维）。
//!
//! 模块分层：[`config`] 配置解析（无副作用）；[`embedding`] 唯一接触
//! sherpa 声纹对象的提取层；[`profiles`] 档案持久化；[`recognizer`] 决策
//! 逻辑与 [`SpeakerRecognizer`] 门面。
//!
//! ## 安全声明
//!
//! 声纹识别仅用于「区分是谁在说话」这类低风险场景，**不是安全认证**：
//! 录音回放、背景噪声、麦克风差异、身体状态与年龄变化都可能导致误判，
//! 不应据此做高敏感度身份确认或绝对权限控制。

pub mod config;
mod embedding;
pub mod profiles;
mod recognizer;

use std::path::PathBuf;

use crate::kws::model::{DownloadProgress, install_raw_file_to, speaker_asset};

pub use config::ResolvedSpeakerConfig;
pub use recognizer::{
    EnrollSummary, LatencyInfo, SkipReason, SpeakerIdentification, SpeakerInfo, SpeakerRecognizer,
    SpeakerScore, SpeakerVerification,
};

/// 确保声纹模型就绪，返回模型文件路径。
///
/// 已存在（或配置显式指定且存在）直接返回；缺失且 `auto_download` 时从
/// 模型清单下载安装（默认模型）。配置显式指定 `[speaker].model` 而文件缺失
/// 时**不**自动下载（自定义模型由用户自管），直接报错。
pub fn ensure_model(cfg: &ResolvedSpeakerConfig) -> Result<PathBuf, String> {
    if let Some(path) = cfg.model_path().filter(|p| p.is_file()) {
        return Ok(path);
    }
    if cfg.model.is_some() || !cfg.auto_download {
        return Err(format!(
            "声纹模型不存在（{}）：可运行 `zapmomo speaker install-model`，\
             或检查 [speaker].model / [speaker].model_dir 配置",
            cfg.model_dir.display()
        ));
    }
    let asset = speaker_asset();
    let dest = cfg.model_dir.join(&asset.archive);
    let mut progress = |p: DownloadProgress| {
        tracing::info!("声纹模型下载: {} {}%", p.message, p.percent);
    };
    install_raw_file_to(asset, &dest, false, &mut progress).map_err(|e| e.to_string())?;
    Ok(dest)
}
