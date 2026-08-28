/// 麦克风泵（`MicLoop`）：把设备音频流按固定 chunk 大小产出。
///
/// 编排线程**整个会话只开一次麦克风**（KWS/ASR 各自 `start_capture` 会在同设备
/// 冲突，且反复开关有几百 ms 延迟），把 chunk 按状态喂给 ASR 流（聆听）或 KWS 流
/// （播报/思考中的打断监听）。
///
/// 职责：`start_capture` → 重采样到目标采样率（复用 `audio::Resampler`）→ 累积到
/// `chunk_size` 抽块（复用 `audio.rs` 中 KWS/ASR `run_realtime_with` 内嵌的同一模式）。
/// 支持「跳过窗口」：打断后回听前丢弃一段回声尾巴，避免把上一条回答喂给 ASR。
use crate::audio;
use std::time::{Duration, Instant};

/// 单次轮询麦克风的最长等待（块间隔远小于此，不影响实时性）。
const POLL: Duration = Duration::from_millis(100);

/// 麦克风事件（`next` 返回值）。
pub enum MicEvent {
    /// 一个完整 chunk（已重采样到目标采样率）
    Chunk(Vec<f32>),
    /// 在 `timeout` 内未凑够一个 chunk（调用方可周期性醒来检查其它信号）
    Timeout,
    /// 麦克风通道断开
    Disconnected,
}

/// 麦克风泵。
pub struct MicLoop {
    mic: audio::MicHandle,
    resampler: audio::Resampler,
    accum: ChunkAccumulator,
    skip_until: Option<Instant>,
}

impl MicLoop {
    /// 打开麦克风并按 `target_rate` 重采样、按 `chunk_size` 抽块。
    pub fn new(device: Option<&str>, target_rate: i32, chunk_size: usize) -> Result<Self, String> {
        let mic = audio::start_capture(device)?;
        let resampler = audio::Resampler::new(mic.device_sample_rate() as i32, target_rate)?;
        Ok(Self {
            mic,
            resampler,
            accum: ChunkAccumulator::new(chunk_size),
            skip_until: None,
        })
    }

    /// 跳过接下来 `d` 时长的音频块（打断后回听前丢弃回声尾巴）。
    pub fn skip_for(&mut self, d: Duration) {
        self.skip_until = Some(Instant::now() + d);
    }

    /// 阻塞至多 `timeout` 取一块。`Timeout` 事件供调用方周期性检查其它信号源。
    pub fn next(&mut self, timeout: Duration) -> Result<MicEvent, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Ok(MicEvent::Timeout);
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(POLL);
            match self.mic.recv_chunk_timeout(wait) {
                Ok(raw) => {
                    let out = self.resampler.process(&raw, false);
                    let mut chunks = Vec::new();
                    self.accum.push(&out, &mut chunks);
                    for chunk in chunks {
                        if self.in_skip_window() {
                            continue; // 丢弃回声尾巴
                        }
                        return Ok(MicEvent::Chunk(chunk));
                    }
                    // 未凑够完整 chunk，继续收
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 等待仍有余量则继续；否则外层 deadline 判断返回 Timeout
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Ok(MicEvent::Disconnected);
                }
            }
        }
    }

    fn in_skip_window(&self) -> bool {
        self.skip_until
            .map(|until| Instant::now() < until)
            .unwrap_or(false)
    }
}

/// 重采样样本 → 固定大小 chunk 的累积器（可独立单测）。
pub struct ChunkAccumulator {
    pending: Vec<f32>,
    chunk_size: usize,
}

impl ChunkAccumulator {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            pending: Vec::with_capacity(chunk_size * 2),
            chunk_size: chunk_size.max(1),
        }
    }

    /// 累积一段样本，产出完整 chunk 追加到 `out`（不足部分留在内部缓冲）。
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<Vec<f32>>) {
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.pending.drain(..self.chunk_size).collect();
            out.push(chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulator_produces_chunks() {
        let mut acc = ChunkAccumulator::new(3200);
        let mut out = Vec::new();
        // 一次喂满 → 一个 chunk，无剩余
        acc.push(&vec![0.0; 3200], &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 3200);
        assert!(acc.pending.is_empty());
    }

    #[test]
    fn test_accumulator_holds_partial() {
        let mut acc = ChunkAccumulator::new(3200);
        let mut out = Vec::new();
        acc.push(&vec![0.0; 1000], &mut out);
        assert!(out.is_empty());
        assert_eq!(acc.pending.len(), 1000);
        // 补足到 3200 → 一个 chunk
        acc.push(&vec![0.0; 2200], &mut out);
        assert_eq!(out.len(), 1);
        assert!(acc.pending.is_empty());
    }

    #[test]
    fn test_accumulator_multiple_chunks_one_push() {
        let mut acc = ChunkAccumulator::new(1600);
        let mut out = Vec::new();
        acc.push(&vec![0.0; 4000], &mut out);
        assert_eq!(out.len(), 2); // 4000 / 1600 = 2 整块
        assert_eq!(out[0].len(), 1600);
        assert_eq!(out[1].len(), 1600);
        assert_eq!(acc.pending.len(), 800);
    }

    #[test]
    fn test_accumulator_straddles_many_pushes() {
        let mut acc = ChunkAccumulator::new(3200);
        let mut out = Vec::new();
        for _ in 0..10 {
            acc.push(&vec![0.0; 1000], &mut out);
        }
        // 累计 10000 样本 → 3 个整块 + 400 剩余
        assert_eq!(out.len(), 3);
        assert_eq!(acc.pending.len(), 400);
    }

    #[test]
    fn test_accumulator_zero_chunk_size_clamped() {
        let mut acc = ChunkAccumulator::new(0);
        let mut out = Vec::new();
        acc.push(&[0.0; 10], &mut out);
        assert_eq!(acc.chunk_size, 1);
        assert_eq!(out.len(), 10);
    }
}
