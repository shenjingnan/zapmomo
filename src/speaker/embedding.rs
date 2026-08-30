//! speaker embedding 提取（sherpa-onnx `SpeakerEmbeddingExtractor` 封装）。
//!
//! speaker 模块中唯一接触 sherpa-onnx 声纹对象的层级：输入任意采样率的
//! 单声道 f32 PCM，内部重采样到 16k 后过模型。embedding 维度运行时
//! `dim()` 取得，不硬编码（换模型不改代码）。模型加载一次常驻，进程内
//! 不重复创建 ONNX Session。

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

use crate::audio::Resampler;
use crate::speaker::config::{ResolvedSpeakerConfig, SPEAKER_SAMPLE_RATE};

/// speaker embedding 提取引擎。
///
/// sherpa 上游对 extractor 承诺单对象线程安全（`unsafe impl Send + Sync`），
/// 方法全部 `&self`，可跨线程共享读取。
pub struct EmbeddingEngine {
    extractor: SpeakerEmbeddingExtractor,
    /// 创建时解析出的模型文件名（写进 profile，跨模型加载守卫用）
    model_name: String,
}

impl EmbeddingEngine {
    /// 从已解析配置创建引擎（不下载模型——ensure 见 [`crate::speaker::ensure_model`]）。
    pub fn new(cfg: &ResolvedSpeakerConfig) -> Result<Self, String> {
        let model_path = cfg.model_path().ok_or_else(|| {
            format!(
                "模型目录 {} 中未找到声纹模型：可运行 `zapmomo speaker install-model`，\
                 或配置 [speaker].model 指定 onnx 文件名",
                cfg.model_dir.display()
            )
        })?;
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(model_path.display().to_string()),
            num_threads: cfg.num_threads,
            debug: cfg.debug,
            provider: Some(cfg.provider.clone()),
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config)
            .ok_or_else(|| format!("声纹模型加载失败: {}", model_path.display()))?;
        let model_name = model_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| model_path.display().to_string());
        Ok(Self {
            extractor,
            model_name,
        })
    }

    /// embedding 维度（CAM++ 为 192）。
    pub fn dim(&self) -> i32 {
        self.extractor.dim()
    }

    /// 创建引擎时使用的模型文件名。
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// 提取 embedding。`sample_rate` 非 16k 时内部先重采样。
    ///
    /// 返回 `Err` 的典型场景：音频太短（不足一个特征窗）、模型推理失败。
    pub fn compute(&self, samples: &[f32], sample_rate: u32) -> Result<Vec<f32>, String> {
        let samples = to_model_rate(samples, sample_rate)?;
        let stream = self.extractor.create_stream().ok_or("创建声纹推理流失败")?;
        stream.accept_waveform(SPEAKER_SAMPLE_RATE, &samples);
        stream.input_finished();
        // 官方语义：input_finished 后 is_ready 为 false 即「音频太短」
        if !self.extractor.is_ready(&stream) {
            return Err("语音太短，无法提取声纹（有效语音不足一个特征窗）".to_string());
        }
        self.extractor
            .compute(&stream)
            .ok_or_else(|| "声纹 embedding 计算失败".to_string())
    }
}

/// 非模型采样率输入的一次性重采样（16k 原样拷贝透传）。
fn to_model_rate(samples: &[f32], sample_rate: u32) -> Result<Vec<f32>, String> {
    if sample_rate == SPEAKER_SAMPLE_RATE as u32 {
        return Ok(samples.to_vec());
    }
    let mut resampler = Resampler::new(sample_rate as i32, SPEAKER_SAMPLE_RATE)
        .map_err(|e| format!("创建重采样器失败（{sample_rate} → {SPEAKER_SAMPLE_RATE}）: {e}"))?;
    Ok(resampler.process(samples, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_model_rate_passthrough_16k() {
        let samples = vec![0.5_f32; 1600];
        let out = to_model_rate(&samples, 16_000).unwrap();
        assert_eq!(out, samples);
    }

    #[test]
    fn test_to_model_rate_upsamples_8k() {
        // 8k → 16k：长度约翻倍
        let samples: Vec<f32> = (0..8000)
            .map(|i| ((i as f32) * 440.0 * 2.0 * std::f32::consts::PI / 8000.0).sin())
            .collect();
        let out = to_model_rate(&samples, 8_000).unwrap();
        assert!(
            out.len() > 15_000 && out.len() <= 16_100,
            "got {}",
            out.len()
        );
    }

    #[test]
    fn test_to_model_rate_downsamples_44k() {
        let samples: Vec<f32> = (0..44_100)
            .map(|i| ((i as f32) * 440.0 * 2.0 * std::f32::consts::PI / 44_100.0).sin())
            .collect();
        let out = to_model_rate(&samples, 44_100).unwrap();
        assert!(
            out.len() > 15_000 && out.len() <= 16_100,
            "got {}",
            out.len()
        );
    }

    /// 真模型冒烟：模型缺失时优雅跳过（模式同 `kws::test_offline_detects_bundled_keyword`）。
    #[test]
    #[ignore = "需要先运行 cargo run -- speaker install-model 下载模型"]
    fn test_compute_with_real_model() {
        let model_path = crate::kws::model::speaker_user_model_path();
        if !model_path.is_file() {
            eprintln!("跳过：声纹模型未安装（{}）", model_path.display());
            return;
        }
        let cfg = ResolvedSpeakerConfig::default();
        let engine = EmbeddingEngine::new(&cfg).unwrap();
        // 2 秒 440Hz 正弦（合成音，仅验证提取链路与维度）
        let samples: Vec<f32> = (0..32_000)
            .map(|i| ((i as f32) * 440.0 * 2.0 * std::f32::consts::PI / 16_000.0).sin())
            .collect();
        let emb = engine.compute(&samples, 16_000).unwrap();
        assert_eq!(emb.len() as i32, engine.dim());
        assert_eq!(
            engine.model_name(),
            model_path.file_name().unwrap().to_string_lossy()
        );
    }
}
