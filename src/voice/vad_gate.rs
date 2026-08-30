/// 说话门控（[`SpeechGate`]）：判定一块音频是否「人声在说话」。
///
/// `Listening` 的说完判定与 `WaitingSpeech` 的进聆听门控共用：优先 Silero 神经 VAD
/// （区分人声与非人声噪声——媒体配乐/键盘/环境声不再被当成说话，杜绝说完判定永不
/// 达成的「无限聆听」）；模型缺失或 ASR 采样率非 16k 时降级为 RMS 能量门限（历史行为）。
use crate::asr::config::ResolvedAsrConfig;
use crate::asr::dictate::{self, DictateConfig};
use sherpa_onnx::VoiceActivityDetector;

/// VAD 内部结果缓冲秒数（语音段只当状态用、每块即排空，30s 仅兜内存，与听写一致）。
const VAD_BUFFER_SECONDS: f32 = 30.0;

/// 说话门控。`vad` 为 `Some` 走 Silero VAD 臂，`None` 走 RMS 降级臂。
pub(crate) struct SpeechGate {
    vad: Option<VoiceActivityDetector>,
    rms_threshold: f32,
}

impl SpeechGate {
    /// 构造门控：VAD 模型在位且采样率匹配则启用，否则降级 RMS。
    ///
    /// 不做网络下载（避免会话启动引入 IO 延迟/失败路径），降级打 warn 提示改善方式。
    pub(crate) fn new(asr_cfg: &ResolvedAsrConfig, rms_threshold: f32) -> Self {
        let vad = try_build_vad(asr_cfg);
        if vad.is_none() {
            tracing::warn!(
                "说完判定降级 RMS 门限（threshold={rms_threshold}）：silero VAD 不可用\
                 （模型未安装/采样率不匹配/创建失败），媒体与键盘噪声可能被当作说话；\
                 安装 silero VAD 模型后可改善"
            );
        } else {
            tracing::info!("说完判定启用 Silero VAD 门控（人声检测，抗非人声噪声）");
        }
        Self { vad, rms_threshold }
    }

    /// 测试构造：强制 RMS 降级臂（不探测模型）。
    #[cfg(test)]
    pub(crate) fn new_rms_only(rms_threshold: f32) -> Self {
        Self {
            vad: None,
            rms_threshold,
        }
    }

    /// 一块音频是否「在说话」。
    ///
    /// VAD 臂自带尾静音滞回（语音结束后 `min_silence_duration` 内仍判在说话）；
    /// RMS 臂即历史行为 `chunk_rms > threshold`。
    pub(crate) fn is_speech(&self, chunk: &[f32]) -> bool {
        match &self.vad {
            Some(v) => {
                v.accept_waveform(chunk);
                let speech = v.detected();
                // 语音段只当状态用（不消费音频），排空防内部缓冲溢出
                if !v.is_empty() {
                    v.clear();
                }
                speech
            }
            None => chunk_rms(chunk) > self.rms_threshold,
        }
    }

    /// 进入新一轮聆听前复位（清 VAD 流内状态，防跨句残留）。
    pub(crate) fn reset(&self) {
        if let Some(v) = &self.vad {
            v.reset();
            v.clear();
        }
    }

    /// 当前是否走 VAD 臂（测试用；生产侧模式日志在 [`Self::new`] 内打）。
    #[cfg(test)]
    pub(crate) fn uses_vad(&self) -> bool {
        self.vad.is_some()
    }
}

/// 尝试构造 Silero VAD：模型在位且 ASR 采样率为 16k 才启用（VAD 固定 16k 输入）。
fn try_build_vad(asr_cfg: &ResolvedAsrConfig) -> Option<VoiceActivityDetector> {
    if !dictate::vad_model_present() {
        return None;
    }
    if asr_cfg.sample_rate != dictate::DICTATE_MODEL_SAMPLE_RATE {
        tracing::warn!(
            "ASR 采样率 {} 与 Silero VAD 所需 16k 不匹配，说完判定降级 RMS 门限",
            asr_cfg.sample_rate
        );
        return None;
    }
    let vad_cfg = DictateConfig::new(dictate::vad_model_path()).with_runtime(asr_cfg);
    let detector =
        VoiceActivityDetector::create(&dictate::build_vad_config(&vad_cfg), VAD_BUFFER_SECONDS);
    if detector.is_none() {
        tracing::warn!("Silero VAD 创建失败，说完判定降级 RMS 门限");
    }
    detector
}

/// 计算一段 f32 mono 音频的 RMS（均方根）音量，用于「真正说话」门控与静音累计。
pub(crate) fn chunk_rms(chunk: &[f32]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: f32 = chunk.iter().map(|s| s * s).sum();
    (sum / chunk.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::config::DEFAULT_VAD_SILENCE_THRESHOLD;

    /// 真模型测试前置：构造 VAD 门控，模型不可用则返回 None（用例内跳过）。
    fn vad_gate_or_skip() -> Option<SpeechGate> {
        let gate = SpeechGate::new(&ResolvedAsrConfig::default(), DEFAULT_VAD_SILENCE_THRESHOLD);
        if !gate.uses_vad() {
            eprintln!("跳过：silero VAD 模型未安装");
            return None;
        }
        Some(gate)
    }

    /// 3200 样本 @16k = 0.2s 块（与会话 chunk_size 默认一致）。
    fn chunk_of(mut sample_fn: impl FnMut(f32) -> f32) -> Vec<f32> {
        (0..3200).map(|i| sample_fn(i as f32 / 16_000.0)).collect()
    }

    #[test]
    fn test_rms_arm_loud_vs_silent() {
        let gate = SpeechGate::new_rms_only(0.02);
        let quiet = chunk_of(|_| 0.0);
        assert!(!gate.is_speech(&quiet));
        let loud = chunk_of(|_| 0.1);
        assert!(gate.is_speech(&loud), "RMS 0.1 > 0.02 应判在说话");
        assert!(!gate.is_speech(&[]));
    }

    #[test]
    fn test_chunk_rms_values() {
        assert_eq!(chunk_rms(&[]), 0.0);
        assert_eq!(chunk_rms(&[0.0, 0.0, 0.0]), 0.0);
        // 恒定振幅 → RMS = 该振幅
        assert!((chunk_rms(&[0.5, 0.5]) - 0.5).abs() < 1e-6);
        // 峰值 1.0 正弦 → RMS ≈ 0.707
        let sine: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * i as f32 / 1600.0).sin())
            .collect();
        assert!((chunk_rms(&sine) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        // 幅度更大 → RMS 更大（用于阈值门控判断）
        assert!(chunk_rms(&sine) < chunk_rms(&sine.iter().map(|s| s * 2.0).collect::<Vec<_>>()));
    }

    /// 核心回归：持续非人声响音（白噪声，RMS 远超 RMS 门限）不应被 VAD 判为说话
    /// ——RMS 门控会误判，导致说完判定永不达成（无限聆听根因）。
    #[test]
    fn test_vad_arm_classifies_noise_as_non_speech() {
        let Some(gate) = vad_gate_or_skip() else {
            return;
        };
        // LCG 白噪声 @0.3 振幅（RMS ≈ 0.17，RMS 门控必判「说话」）
        let mut seed = 0x1234_5678u32;
        let mut noise = chunk_of(move |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.3 * (seed as f32 / u32::MAX as f32 - 0.5) * 2.0
        });
        // 喂 2 秒（10 块），任一块判「说话」即失败
        let any_speech = (0..10).any(|_| {
            let speech = gate.is_speech(&noise);
            // 翻转符号避免 LCG 退化成常量
            noise.iter_mut().for_each(|s| *s = -*s);
            speech
        });
        assert!(
            !any_speech,
            "持续白噪声不应被 VAD 判为说话（RMS 门控会误判）"
        );
    }

    /// 真人语音正向：模型自带示例音频分块喂入，应出现「说话」判定。
    #[test]
    fn test_vad_arm_detects_real_speech() {
        let Some(gate) = vad_gate_or_skip() else {
            return;
        };
        let asr_cfg = crate::asr::config::resolve(None, None).unwrap();
        let Some(wav) = crate::asr::default_test_wav(&asr_cfg.model_dir) else {
            eprintln!("跳过：ASR 模型目录无示例音频");
            return;
        };
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).unwrap();
        let samples = if wave.sample_rate() == 16_000 {
            wave.samples().to_vec()
        } else {
            let mut rs = crate::audio::Resampler::new(wave.sample_rate(), 16_000).unwrap();
            rs.process(wave.samples(), true)
        };
        let any_speech = samples
            .chunks(3200)
            .filter(|c| c.len() == 3200)
            .any(|c| gate.is_speech(c));
        assert!(any_speech, "真人语音示例应被 VAD 判为说话");
    }

    /// reset 清流内状态：语音后 reset，短静音不应残留「在说话」判定。
    #[test]
    fn test_vad_arm_reset_clears_state() {
        let Some(gate) = vad_gate_or_skip() else {
            return;
        };
        let asr_cfg = crate::asr::config::resolve(None, None).unwrap();
        let Some(wav) = crate::asr::default_test_wav(&asr_cfg.model_dir) else {
            eprintln!("跳过：ASR 模型目录无示例音频");
            return;
        };
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).unwrap();
        for c in wave.samples().chunks(3200).filter(|c| c.len() == 3200) {
            gate.is_speech(c);
        }
        gate.reset();
        let quiet = chunk_of(|_| 0.0);
        // 尾静音滞回最长 min_silence_duration(0.5s)=2.5 块，喂 1s（5 块）必已回落
        let stuck = (0..5).any(|_| gate.is_speech(&quiet));
        assert!(!stuck, "reset 后静音不应仍判「说话」");
    }
}
