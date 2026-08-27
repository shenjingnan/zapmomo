/// 关键词唤醒词检测（KWS）。
///
/// 使用 sherpa-onnx 的 `KeywordSpotter`（zipformer 唤醒词模型）实现：
/// 离线对 wav 检测（`run_offline`）与实时麦克风监听（`run_realtime`）。
pub mod config;
pub mod english;
pub mod model;
pub mod reaction;
pub mod token;

use crate::audio::Resampler;
use config::ResolvedKwsConfig;
use sherpa_onnx::{KeywordSpotter, KeywordSpotterConfig, OnlineStream, Wave};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub use reaction::{CollectReaction, ConsoleReaction, KwsResult, Reaction, ReactionOutcome};

/// 关键词检测引擎。
///
/// 持有 `KeywordSpotter`。所有方法接收 `&self`（sherpa 的 is_ready/decode/
/// get_result/reset 均只读），因此引擎可复用，不需要 `&mut`。
pub struct KwsEngine {
    spotter: KeywordSpotter,
    cfg: ResolvedKwsConfig,
}

impl KwsEngine {
    /// 构造引擎，先校验所有模型文件存在。
    pub fn new(cfg: ResolvedKwsConfig) -> Result<Self, String> {
        let required = [
            ("encoder", &cfg.encoder),
            ("decoder", &cfg.decoder),
            ("joiner", &cfg.joiner),
            ("tokens", &cfg.tokens),
            ("keywords_file", &cfg.keywords_file),
        ];
        for (name, path) in required {
            if !path.is_file() {
                return Err(format!(
                    "缺少模型文件 {name}: {}\n请运行 `zapmomo kws install-model`（源码仓库亦可运行 scripts/download-kws-model.sh）下载模型。",
                    path.display()
                ));
            }
        }

        let mut config = KeywordSpotterConfig::default();
        config.feat_config.sample_rate = cfg.sample_rate;
        config.model_config.transducer.encoder = Some(cfg.encoder.to_string_lossy().to_string());
        config.model_config.transducer.decoder = Some(cfg.decoder.to_string_lossy().to_string());
        config.model_config.transducer.joiner = Some(cfg.joiner.to_string_lossy().to_string());
        config.model_config.tokens = Some(cfg.tokens.to_string_lossy().to_string());
        config.model_config.provider = Some(cfg.provider.clone());
        config.model_config.num_threads = cfg.num_threads;
        config.model_config.debug = cfg.debug;
        config.keywords_file = Some(cfg.keywords_file.to_string_lossy().to_string());
        config.keywords_score = cfg.keywords_score;
        config.keywords_threshold = cfg.keywords_threshold;

        let spotter = KeywordSpotter::create(&config)
            .ok_or_else(|| "无法创建 KeywordSpotter，请检查模型文件与配置。".to_string())?;
        Ok(Self { spotter, cfg })
    }

    pub fn config(&self) -> &ResolvedKwsConfig {
        &self.cfg
    }

    /// 用 `keywords_file` 中的关键词创建流。
    pub fn create_stream(&self) -> OnlineStream {
        self.spotter.create_stream()
    }

    /// 运行时附加关键词（tokenized 格式，多个用 `/` 分隔）。
    pub fn create_stream_with_keywords(&self, keywords: &str) -> OnlineStream {
        self.spotter.create_stream_with_keywords(keywords)
    }

    /// 喂入一帧音频（采样率 = `cfg.sample_rate`）。
    pub fn feed(&self, stream: &OnlineStream, samples: &[f32]) {
        stream.accept_waveform(self.cfg.sample_rate, samples);
    }

    /// 标记输入结束（离线路径 flush 出尾部结果）。
    pub fn finish(&self, stream: &OnlineStream) {
        stream.input_finished();
    }

    /// 标准检测循环：`while is_ready { decode; get_result; reset }`。
    /// 命中唤醒词时调用 reaction，返回 `Stop` 则立即退出。
    pub fn detect(&self, stream: &OnlineStream, reaction: &mut dyn Reaction) -> ReactionOutcome {
        let mut outcome = ReactionOutcome::Continue;
        while self.spotter.is_ready(stream) {
            self.spotter.decode(stream);
            if let Some(r) = self.spotter.get_result(stream)
                && !r.keyword.is_empty()
            {
                outcome = reaction.on_keyword(&KwsResult::from(&r));
                self.spotter.reset(stream);
                if outcome == ReactionOutcome::Stop {
                    break;
                }
            }
        }
        outcome
    }
}

/// 离线检测 wav 文件中的关键词（不依赖麦克风）。
///
/// 用于验证模型与整条链路：对模型自带 `test_wavs/zh_3.wav` 应检出「文森特卡索」「法国」。
pub fn run_offline(
    cfg: &ResolvedKwsConfig,
    wav: &Path,
    keywords: Option<&str>,
) -> Result<(), String> {
    let engine = KwsEngine::new(cfg.clone())?;
    let stream = match keywords {
        Some(k) => {
            // 原始中文会自动转成模型可编码的 ppinyin token；校验失败返回清晰错误
            let encoded = token::encode_custom_keywords(k, &cfg.tokens)?;
            engine.create_stream_with_keywords(&encoded)
        }
        None => engine.create_stream(),
    };
    let wave = Wave::read(&wav.to_string_lossy())
        .ok_or_else(|| format!("无法读取 wav: {}", wav.display()))?;

    // 若 wav 采样率 != 模型采样率，先重采样（test_wavs 是 16k，一般直接走 else）
    if wave.sample_rate() != cfg.sample_rate {
        let mut rs = Resampler::new(wave.sample_rate(), cfg.sample_rate)?;
        let out = rs.process(wave.samples(), true);
        engine.feed(&stream, &out);
    } else {
        engine.feed(&stream, wave.samples());
    }
    // 尾部补 0.5s 静音，让模型 flush 出最后一个结果
    let tail = vec![0.0f32; (cfg.sample_rate as usize) / 2];
    engine.feed(&stream, &tail);
    engine.finish(&stream);

    let mut reaction = ConsoleReaction;
    engine.detect(&stream, &mut reaction);
    Ok(())
}

/// 实时监听麦克风，检测唤醒词（默认反应 + 不可取消，供 CLI 使用）。
///
/// 等价于 `run_realtime_with(cfg, device, duration, keywords, ConsoleReaction, None)`。
pub fn run_realtime(
    cfg: &ResolvedKwsConfig,
    device: Option<&str>,
    duration: Option<u64>,
    keywords: Option<&str>,
) -> Result<(), String> {
    let mut reaction = ConsoleReaction;
    run_realtime_with(cfg, device, duration, keywords, &mut reaction, None)
}

/// 实时监听麦克风，检测唤醒词。
///
/// 线程模型：cpal 采集在系统音频线程，经 `mpsc` 送到调用线程；
/// 调用线程内做重采样 + 检测循环（sherpa 类型不跨线程）。
///
/// `reaction` 为可插拔唤醒反应（GUI 可实现自己的 `Reaction` 发事件给前端）；
/// `should_stop` 为非空时，每次迭代检查该标志，置位则干净退出（返回 `Ok(())`），
/// 供桌面 GUI 的「停止监听」使用。
pub fn run_realtime_with(
    cfg: &ResolvedKwsConfig,
    device: Option<&str>,
    duration: Option<u64>,
    keywords: Option<&str>,
    reaction: &mut dyn Reaction,
    should_stop: Option<&AtomicBool>,
) -> Result<(), String> {
    let engine = KwsEngine::new(cfg.clone())?;
    let stream = match keywords {
        Some(k) => {
            // 原始中文会自动转成模型可编码的 ppinyin token；校验失败返回清晰错误
            let encoded = token::encode_custom_keywords(k, &cfg.tokens)?;
            engine.create_stream_with_keywords(&encoded)
        }
        None => engine.create_stream(),
    };

    let mut mic = crate::audio::start_capture(device)?;
    let mut resampler = Resampler::new(mic.device_sample_rate() as i32, cfg.sample_rate)?;
    let mut pending: Vec<f32> = Vec::with_capacity(cfg.chunk_size * 2);
    let start = std::time::Instant::now();
    let deadline = duration.map(|secs| start + std::time::Duration::from_secs(secs));

    // should_stop 标志语义为 `running`（true = 正在监听）；因此「应停止」= 标志为 false。
    // CLI 传 None 时恒为 false（不主动停止，由 Ctrl-C / --duration 控制）。
    let stop_requested = || should_stop.is_some_and(|f| !f.load(Ordering::Relaxed));

    println!("开始监听唤醒词... (Ctrl-C 退出; --duration 可限制时长)");
    let mut chunks_received: u64 = 0;
    let mut process =
        |raw: Vec<f32>, engine: &KwsEngine, stream: &OnlineStream| -> Result<bool, String> {
            let out = resampler.process(&raw, false);
            pending.extend_from_slice(&out);
            while pending.len() >= cfg.chunk_size {
                let chunk: Vec<f32> = pending.drain(..cfg.chunk_size).collect();
                engine.feed(stream, &chunk);
                if engine.detect(stream, reaction) == ReactionOutcome::Stop {
                    return Ok(true); // 应停止
                }
                if stop_requested() {
                    return Ok(true); // GUI 请求停止
                }
            }
            Ok(false)
        };

    loop {
        if stop_requested() {
            tracing::warn!("KWS 监听退出：收到停止请求（共收到 {chunks_received} 块）");
            break;
        }
        let raw = if let Some(dl) = deadline {
            if std::time::Instant::now() >= dl {
                break;
            }
            let timeout = dl
                .saturating_duration_since(std::time::Instant::now())
                .min(std::time::Duration::from_millis(500));
            match mic.recv_chunk_timeout(timeout) {
                Ok(raw) => Some(raw),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    tracing::warn!("KWS 监听退出：麦克风通道断开（共收到 {chunks_received} 块）");
                    break;
                }
            }
        } else {
            mic.recv_chunk()
        };

        let Some(raw) = raw else {
            tracing::warn!("KWS 监听退出：麦克风返回 None（共收到 {chunks_received} 块）");
            break;
        };
        chunks_received += 1;
        if process(raw, &engine, &stream)? {
            return Ok(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_engine_new_missing_model_errors() {
        // 用一个不存在的模型目录，KwsEngine::new 应报错提示下载模型
        let mut cfg = ResolvedKwsConfig::default();
        cfg.model_dir = PathBuf::from("/nonexistent/model");
        cfg.encoder = cfg.model_dir.join("encoder.onnx");
        let err = KwsEngine::new(cfg.clone()).err().unwrap();
        assert!(err.contains("install-model"), "err: {err}");
    }

    #[test]
    #[ignore = "需要先运行 scripts/download-kws-model.sh 下载模型"]
    fn test_offline_detects_bundled_keyword() {
        let cfg = config::resolve(None, None).unwrap();
        if !cfg.encoder.is_file() {
            eprintln!("跳过：模型未下载，请运行 scripts/download-kws-model.sh");
            return;
        }
        let engine = KwsEngine::new(cfg.clone()).unwrap();
        let stream = engine.create_stream();
        let wave = Wave::read(&cfg.model_dir.join("test_wavs/zh_3.wav").to_string_lossy()).unwrap();
        engine.feed(&stream, wave.samples());
        engine.feed(&stream, &vec![0.0; (cfg.sample_rate as usize) / 2]);
        engine.finish(&stream);

        let mut collect = CollectReaction::new();
        engine.detect(&stream, &mut collect);
        assert!(
            collect.results.iter().any(|r| r.keyword.contains("文森特")),
            "应检测到捆绑关键词，实际: {:?}",
            collect
                .results
                .iter()
                .map(|r| r.keyword.clone())
                .collect::<Vec<_>>()
        );
    }
}
