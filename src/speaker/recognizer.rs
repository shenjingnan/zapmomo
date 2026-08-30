//! 声纹识别决策逻辑与 [`SpeakerRecognizer`] 门面实现。
//!
//! 结果类型与阈值判定都在这里收敛；业务层只跟 [`SpeakerRecognizer`] 打交道，
//! 不接触 sherpa-onnx 底层对象。识别（1:N）与验证（1:1）共用同一判定原语：
//! `get_best_matches` 拿全量分数表（扫描阈值 `-1.0`，见 [`MATCH_SCAN_THRESHOLD`]），
//! 再在 Rust 侧按配置阈值判 matched —— 这样即使未过阈值也能拿到 best/second
//! 分数，便于分析误识别。

use std::sync::Mutex;
use std::time::Instant;

use sherpa_onnx::SpeakerEmbeddingManager;

use crate::speaker::config::ResolvedSpeakerConfig;
use crate::speaker::embedding::EmbeddingEngine;
use crate::speaker::profiles::{self, ProfileSample, SpeakerProfile};

/// 分数表扫描阈值：取负值让 `get_best_matches` 返回全部说话人（含低分），
/// matched 由调用方按配置阈值判定。若上游不接受负阈值，真机验收（Case 1–5）会暴露。
const MATCH_SCAN_THRESHOLD: f32 = -1.0;

/// 一条说话人得分（识别结果分数表的一行）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SpeakerScore {
    pub speaker_id: String,
    pub score: f32,
}

/// 延迟统计（毫秒），供性能测试与误识别分析。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyInfo {
    /// 输入音频时长
    pub audio_duration_ms: f64,
    /// embedding 提取耗时
    pub embedding_ms: f64,
    /// 分数表匹配耗时
    pub matching_ms: f64,
    /// 端到端耗时（含守卫与拷贝）
    pub total_ms: f64,
}

/// 跳过识别的原因。
#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    /// 语音短于 `min_audio_duration_secs`（防「嗯/啊」短促声误识别）
    TooShort { duration_ms: f64, min_ms: f64 },
    /// 尚无任何注册说话人
    NoRegisteredSpeakers,
}

/// 识别（1:N）结果。
#[derive(Debug, Clone)]
pub struct SpeakerIdentification {
    pub matched: bool,
    /// `None` = unknown（未过阈值或被跳过）
    pub speaker_id: Option<String>,
    /// 最高分（低于阈值也返回，便于分析；被跳过或无注册时为 `None`）
    pub score: Option<f32>,
    /// 全量分数表（降序；`owner 0.83 / user_2 0.51 / ...`）
    pub scores: Vec<SpeakerScore>,
    pub threshold: f32,
    pub skipped: Option<SkipReason>,
    pub latency: LatencyInfo,
}

/// 验证（1:1）结果。
#[derive(Debug, Clone)]
pub struct SpeakerVerification {
    pub matched: bool,
    /// 命中的说话人（matched 时等于被验证者；unknown 为 `None`）
    pub speaker_id: Option<String>,
    pub score: Option<f32>,
    pub threshold: f32,
    pub skipped: Option<SkipReason>,
    pub latency: LatencyInfo,
}

/// 注册摘要。
#[derive(Debug, Clone)]
pub struct EnrollSummary {
    pub speaker_id: String,
    pub sample_count: usize,
    pub dim: i32,
    pub embedding_ms: f64,
}

/// 已注册说话人信息（`speaker list` 用）。
#[derive(Debug, Clone)]
pub struct SpeakerInfo {
    pub speaker_id: String,
    pub sample_count: usize,
    pub model: String,
    pub dim: usize,
    pub updated_at: String,
}

/// 输入音频时长（毫秒）。
pub(crate) fn audio_duration_ms(samples_len: usize, sample_rate: u32) -> f64 {
    if sample_rate == 0 {
        return 0.0;
    }
    samples_len as f64 / sample_rate as f64 * 1000.0
}

/// 最短语音守卫：短于 `min_secs` 返回 [`SkipReason::TooShort`]。
pub(crate) fn check_min_duration(duration_ms: f64, min_secs: f32) -> Result<(), SkipReason> {
    let min_ms = min_secs as f64 * 1000.0;
    if duration_ms < min_ms {
        Err(SkipReason::TooShort {
            duration_ms,
            min_ms,
        })
    } else {
        Ok(())
    }
}

/// 阈值判定：分数表降序排序后取最高分与配置阈值比较。
///
/// 返回 `(matched, best_speaker_id, best_score)`；空表返回全 `None`。
/// 未过阈值时 best_score 仍返回（供 `scores` 表输出与误识别分析）。
pub(crate) fn decide(
    scores: &mut [SpeakerScore],
    threshold: f32,
) -> (bool, Option<String>, Option<f32>) {
    scores.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    match scores.first() {
        Some(top) => {
            let matched = top.score >= threshold;
            (
                matched,
                matched.then(|| top.speaker_id.clone()),
                Some(top.score),
            )
        }
        None => (false, None, None),
    }
}

/// 声纹识别器：模型 + speaker 索引 + 配置的门面。
///
/// 线程安全说明：extractor 只读共享；`SpeakerEmbeddingManager` 上游仅承诺
/// 「单对象线程安全」、未文档化并发读写，故单独包一把 `Mutex`（仅索引操作
/// 持锁，embedding 推理在锁外）。**不把整个识别器包进 Arc<Mutex>。**
pub struct SpeakerRecognizer {
    engine: EmbeddingEngine,
    manager: Mutex<SpeakerEmbeddingManager>,
    cfg: ResolvedSpeakerConfig,
}

impl SpeakerRecognizer {
    /// 创建识别器：ensure 模型（缺失时可自动下载）→ 加载引擎 → 从磁盘载入
    /// 全部声纹档案进内存索引。
    pub fn new(cfg: ResolvedSpeakerConfig) -> Result<Self, String> {
        crate::speaker::ensure_model(&cfg)?;
        let engine = EmbeddingEngine::new(&cfg)?;
        let dim = engine.dim();
        let manager = SpeakerEmbeddingManager::create(dim)
            .ok_or_else(|| format!("创建 SpeakerEmbeddingManager 失败（dim={dim}）"))?;
        let recognizer = Self {
            engine,
            manager: Mutex::new(manager),
            cfg,
        };
        recognizer.load_profiles()?;
        Ok(recognizer)
    }

    /// embedding 维度。
    pub fn dim(&self) -> i32 {
        self.engine.dim()
    }

    /// 配置。
    pub fn config(&self) -> &ResolvedSpeakerConfig {
        &self.cfg
    }

    /// 从磁盘载入全部档案；模型或维度不匹配的档案跳过并 warn（跨模型守卫）。
    fn load_profiles(&self) -> Result<(), String> {
        for profile in profiles::list()? {
            if profile.model != self.engine.model_name() || profile.dim != self.dim() as usize {
                tracing::warn!(
                    "声纹档案 {} 属于模型 {}（dim={}），与当前模型 {}（dim={}）不匹配，跳过加载；重新注册可迁移",
                    profile.speaker_id,
                    profile.model,
                    profile.dim,
                    self.engine.model_name(),
                    self.dim()
                );
                continue;
            }
            let embeddings: Vec<Vec<f32>> = profile
                .samples
                .iter()
                .map(|s| s.embedding.clone())
                .collect();
            let ok = self
                .manager
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .add_list(&profile.speaker_id, &embeddings);
            if !ok {
                tracing::warn!("声纹档案 {} 载入索引失败，跳过", profile.speaker_id);
            }
        }
        Ok(())
    }

    /// 注册（覆盖语义：同名先移除旧索引再写入，档案整体覆盖保存）。
    ///
    /// 每段音频独立提取 embedding；短于 `min_audio_duration_secs` 的段报错。
    pub fn enroll(
        &self,
        speaker_id: &str,
        samples: &[(Vec<f32>, u32)],
    ) -> Result<EnrollSummary, String> {
        profiles::validate_speaker_id(speaker_id)?;
        if samples.is_empty() {
            return Err("注册至少需要一段音频".to_string());
        }
        let started = Instant::now();
        let mut profile_samples = Vec::with_capacity(samples.len());
        for (idx, (pcm, rate)) in samples.iter().enumerate() {
            let duration_ms = audio_duration_ms(pcm.len(), *rate);
            check_min_duration(duration_ms, self.cfg.min_audio_duration_secs).map_err(|_| {
                format!(
                    "第 {} 段音频太短（{duration_ms:.0}ms，最少 {}ms），请提供更长的注册音频",
                    idx + 1,
                    self.cfg.min_audio_duration_secs as f64 * 1000.0
                )
            })?;
            let t = Instant::now();
            let embedding = self.engine.compute(pcm, *rate)?;
            profile_samples.push(ProfileSample {
                embedding,
                duration_ms,
                enrolled_at: crate::datetime::iso_timestamp_now(),
            });
            tracing::debug!("enroll 段 {} embedding 提取耗时 {:?}", idx + 1, t.elapsed());
        }
        let embeddings: Vec<Vec<f32>> = profile_samples
            .iter()
            .map(|s| s.embedding.clone())
            .collect();
        {
            // sherpa manager 方法全为 &self（C++ 侧内部可变），无需 mut 绑定
            let manager = self.manager.lock().unwrap_or_else(|e| e.into_inner());
            if manager.contains(speaker_id) {
                manager.remove(speaker_id);
            }
            if !manager.add_list(speaker_id, &embeddings) {
                return Err(format!("说话人 {speaker_id} 写入索引失败（维度不匹配？）"));
            }
        }
        let profile = SpeakerProfile {
            version: profiles::PROFILE_VERSION,
            speaker_id: speaker_id.to_string(),
            model: self.engine.model_name().to_string(),
            dim: self.dim() as usize,
            updated_at: crate::datetime::iso_timestamp_now(),
            samples: profile_samples,
        };
        profiles::save(&profile)?;
        Ok(EnrollSummary {
            speaker_id: speaker_id.to_string(),
            sample_count: embeddings.len(),
            dim: self.dim(),
            embedding_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    /// 识别（1:N）：返回最高分说话人；低于阈值或被跳过时 `speaker_id = None`（unknown）。
    pub fn identify(
        &self,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<SpeakerIdentification, String> {
        let started = Instant::now();
        let duration_ms = audio_duration_ms(samples.len(), sample_rate);
        let threshold = self.cfg.threshold;
        let finish = |matched: bool,
                      speaker_id: Option<String>,
                      score: Option<f32>,
                      scores: Vec<SpeakerScore>,
                      skipped: Option<SkipReason>,
                      embedding_ms: f64,
                      matching_ms: f64| {
            let latency = LatencyInfo {
                audio_duration_ms: duration_ms,
                embedding_ms,
                matching_ms,
                total_ms: started.elapsed().as_secs_f64() * 1000.0,
            };
            tracing::debug!(
                "identify: audio {:.0}ms, embed {:.1}ms, match {:.1}ms, total {:.1}ms",
                latency.audio_duration_ms,
                latency.embedding_ms,
                latency.matching_ms,
                latency.total_ms
            );
            SpeakerIdentification {
                matched,
                speaker_id,
                score,
                scores,
                threshold,
                skipped,
                latency,
            }
        };

        if self.num_registered() == 0 {
            return Ok(finish(
                false,
                None,
                None,
                Vec::new(),
                Some(SkipReason::NoRegisteredSpeakers),
                0.0,
                0.0,
            ));
        }
        if let Err(reason) = check_min_duration(duration_ms, self.cfg.min_audio_duration_secs) {
            return Ok(finish(
                false,
                None,
                None,
                Vec::new(),
                Some(reason),
                0.0,
                0.0,
            ));
        }

        let t = Instant::now();
        let embedding = self.engine.compute(samples, sample_rate)?;
        let embedding_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let (matched, speaker_id, score, scores) = self.scan_matches(&embedding, threshold);
        let matching_ms = t.elapsed().as_secs_f64() * 1000.0;
        Ok(finish(
            matched,
            speaker_id,
            score,
            scores,
            None,
            embedding_ms,
            matching_ms,
        ))
    }

    /// 验证（1:1）：判断输入是否为指定说话人。未注册的 speaker_id 返回 `Err`。
    pub fn verify(
        &self,
        speaker_id: &str,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<SpeakerVerification, String> {
        let started = Instant::now();
        let duration_ms = audio_duration_ms(samples.len(), sample_rate);
        let threshold = self.cfg.threshold;
        if !self.is_registered(speaker_id) {
            return Err(format!("说话人 {speaker_id} 未注册，请先 enroll"));
        }
        if let Err(reason) = check_min_duration(duration_ms, self.cfg.min_audio_duration_secs) {
            return Ok(SpeakerVerification {
                matched: false,
                speaker_id: None,
                score: None,
                threshold,
                skipped: Some(reason),
                latency: LatencyInfo {
                    audio_duration_ms: duration_ms,
                    embedding_ms: 0.0,
                    matching_ms: 0.0,
                    total_ms: started.elapsed().as_secs_f64() * 1000.0,
                },
            });
        }

        let t = Instant::now();
        let embedding = self.engine.compute(samples, sample_rate)?;
        let embedding_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let (matched, speaker_id, score, _table) = self.scan_matches(&embedding, threshold);
        let matching_ms = t.elapsed().as_secs_f64() * 1000.0;
        Ok(SpeakerVerification {
            matched,
            speaker_id,
            score,
            threshold,
            skipped: None,
            latency: LatencyInfo {
                audio_duration_ms: duration_ms,
                embedding_ms,
                matching_ms,
                total_ms: started.elapsed().as_secs_f64() * 1000.0,
            },
        })
    }

    /// 移除注册（索引 + 磁盘档案）；返回是否确实删除了档案。
    pub fn remove_speaker(&self, speaker_id: &str) -> Result<bool, String> {
        profiles::validate_speaker_id(speaker_id)?;
        self.manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(speaker_id);
        profiles::delete(speaker_id)
    }

    /// 列出全部已注册说话人（读磁盘档案）。
    pub fn list_speakers(&self) -> Result<Vec<SpeakerInfo>, String> {
        Ok(profiles::list()?
            .into_iter()
            .map(|p| SpeakerInfo {
                speaker_id: p.speaker_id,
                sample_count: p.samples.len(),
                model: p.model,
                dim: p.dim,
                updated_at: p.updated_at,
            })
            .collect())
    }

    /// 已注册说话人数（读内存索引）。
    pub fn num_registered(&self) -> usize {
        self.manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .num_speakers() as usize
    }

    /// 指定说话人是否已注册（读内存索引）。
    pub fn is_registered(&self, speaker_id: &str) -> bool {
        self.manager
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(speaker_id)
    }

    /// 取全量分数表并按配置阈值判定。返回 `(matched, best_id, best_score, 降序表)`。
    fn scan_matches(
        &self,
        embedding: &[f32],
        threshold: f32,
    ) -> (bool, Option<String>, Option<f32>, Vec<SpeakerScore>) {
        let manager = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        let n = manager.num_speakers();
        let mut scores: Vec<SpeakerScore> = manager
            .get_best_matches(embedding, MATCH_SCAN_THRESHOLD, n)
            .into_iter()
            .map(|m| SpeakerScore {
                speaker_id: m.name,
                score: m.score,
            })
            .collect();
        let (matched, id, score) = decide(&mut scores, threshold);
        (matched, id, score, scores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 纯决策函数 ----

    #[test]
    fn test_audio_duration_ms() {
        assert!((audio_duration_ms(16_000, 16_000) - 1000.0).abs() < 1e-9);
        assert!((audio_duration_ms(8_000, 8_000) - 1000.0).abs() < 1e-9);
        assert_eq!(audio_duration_ms(0, 16_000), 0.0);
    }

    #[test]
    fn test_check_min_duration() {
        assert!(check_min_duration(1000.0, 1.0).is_ok());
        assert!(check_min_duration(5000.0, 1.0).is_ok());
        match check_min_duration(900.0, 1.0) {
            Err(SkipReason::TooShort {
                duration_ms,
                min_ms,
                ..
            }) => {
                assert!((duration_ms - 900.0).abs() < 1e-9);
                assert!((min_ms - 1000.0).abs() < 1e-9);
            }
            other => panic!("应 TooShort，实际 {other:?}"),
        }
    }

    #[test]
    fn test_decide_matched_above_threshold() {
        let mut scores = vec![
            SpeakerScore {
                speaker_id: "user_2".into(),
                score: 0.51,
            },
            SpeakerScore {
                speaker_id: "owner".into(),
                score: 0.83,
            },
        ];
        let (matched, id, score) = decide(&mut scores, 0.6);
        assert!(matched);
        assert_eq!(id.as_deref(), Some("owner"));
        assert_eq!(score, Some(0.83));
        // 表已降序排序（供输出）
        assert_eq!(scores[0].speaker_id, "owner");
        assert_eq!(scores[1].speaker_id, "user_2");
    }

    #[test]
    fn test_decide_unknown_below_threshold() {
        let mut scores = vec![
            SpeakerScore {
                speaker_id: "owner".into(),
                score: 0.38,
            },
            SpeakerScore {
                speaker_id: "user_1".into(),
                score: 0.21,
            },
        ];
        let (matched, id, score) = decide(&mut scores, 0.6);
        assert!(!matched);
        // 分数仍然可见（best_score 供分析），只是不命中
        assert_eq!(id.as_deref(), None);
        assert_eq!(score, Some(0.38));
    }

    #[test]
    fn test_decide_empty_table() {
        let mut scores: Vec<SpeakerScore> = Vec::new();
        let (matched, id, score) = decide(&mut scores, 0.6);
        assert!(!matched);
        assert_eq!(id, None);
        assert_eq!(score, None);
    }
}

/// 真模型端到端验收（Case 1–5）。
///
/// 依赖：声纹模型（`cargo run -- speaker install-model`）与网络（release 附带的
/// 说话人样例 wav，体积 ~1MB，测试时下载到临时目录）。模型/网络缺失时优雅跳过。
#[cfg(test)]
mod real_model_tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    /// sherpa-onnx 官方 speaker recognition release（tag 拼写如此）。
    const SR_BASE: &str =
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models";
    const SR_WAVS: [&str; 8] = [
        "fangjun-sr-1.wav",
        "fangjun-sr-2.wav",
        "fangjun-sr-3.wav",
        "leijun-sr-1.wav",
        "leijun-sr-2.wav",
        "fangjun-test-sr-1.wav",
        "leijun-test-sr-1.wav",
        "liudehua-test-sr-1.wav",
    ];

    fn download_wavs(dir: &std::path::Path) -> Result<(), String> {
        for name in SR_WAVS {
            let dest = dir.join(name);
            if dest.is_file() {
                continue;
            }
            let resp = ureq::get(format!("{SR_BASE}/{name}"))
                .call()
                .map_err(|e| e.to_string())?;
            let mut reader = resp.into_body().into_reader();
            let mut file = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
            std::io::copy(&mut reader, &mut file).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 确定性伪噪声（1.5s，方波混合，非人声）。
    fn noise_samples() -> Vec<f32> {
        (0..24_000)
            .map(|i| if (i / 37) % 2 == 0 { 0.4 } else { -0.3 })
            .collect()
    }

    #[test]
    #[ignore = "需要模型与网络：先 cargo run -- speaker install-model，样例 wav 测试时下载"]
    fn test_real_model_case1_to_case5() {
        if !crate::kws::model::speaker_user_model_path().is_file() {
            eprintln!("跳过：声纹模型未安装（先运行 cargo run -- speaker install-model）");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        if let Err(e) = download_wavs(tmp.path()) {
            eprintln!("跳过：样例 wav 下载失败（{e}）");
            return;
        }
        let read = |name: &str| {
            sherpa_onnx::Wave::read(tmp.path().join(name).to_str().unwrap())
                .unwrap_or_else(|| panic!("读取样例 {name} 失败"))
        };
        let enroll_samples = |names: &[&str]| -> Vec<(Vec<f32>, u32)> {
            names
                .iter()
                .map(|n| {
                    let w = read(n);
                    (w.samples().to_vec(), w.sample_rate() as u32)
                })
                .collect()
        };

        run_with_temp_home(|_home| {
            let rec = SpeakerRecognizer::new(ResolvedSpeakerConfig::default()).unwrap();
            assert_eq!(rec.num_registered(), 0);

            // 无注册时 identify → Skipped(NoRegisteredSpeakers)
            let probe = read("fangjun-test-sr-1.wav");
            let id = rec
                .identify(probe.samples(), probe.sample_rate() as u32)
                .unwrap();
            assert_eq!(id.skipped, Some(SkipReason::NoRegisteredSpeakers));

            // Case 5 准备：注册 fangjun(3 段) 与 leijun(2 段)
            rec.enroll(
                "fangjun",
                &enroll_samples(&["fangjun-sr-1.wav", "fangjun-sr-2.wav", "fangjun-sr-3.wav"]),
            )
            .unwrap();
            rec.enroll(
                "leijun",
                &enroll_samples(&["leijun-sr-1.wav", "leijun-sr-2.wav"]),
            )
            .unwrap();
            assert_eq!(rec.num_registered(), 2);

            // Case 1：同人 → 高分命中 + verify 通过
            let id = rec
                .identify(probe.samples(), probe.sample_rate() as u32)
                .unwrap();
            assert!(id.matched, "同人识别应命中: {id:?}");
            assert_eq!(id.speaker_id.as_deref(), Some("fangjun"));
            assert!(id.score.unwrap() >= id.threshold);
            let v = rec
                .verify("fangjun", probe.samples(), probe.sample_rate() as u32)
                .unwrap();
            assert!(v.matched, "同人验证应通过: {v:?}");
            assert_eq!(v.speaker_id.as_deref(), Some("fangjun"));

            // Case 2：未注册陌生人 → unknown（分数低于阈值且分数表可见）
            let liu = read("liudehua-test-sr-1.wav");
            let id = rec
                .identify(liu.samples(), liu.sample_rate() as u32)
                .unwrap();
            assert!(!id.matched, "陌生人不应命中: {id:?}");
            assert_eq!(id.speaker_id, None);
            let best = id.score.expect("未知时也应返回 best_score");
            assert!(
                best < id.threshold,
                "best {best} 应低于阈值 {}",
                id.threshold
            );
            assert_eq!(id.scores.len(), 2, "分数表应覆盖全部注册说话人");

            // Case 3：背景噪声 → unknown
            let noise = noise_samples();
            let id = rec.identify(&noise, 16_000).unwrap();
            assert!(!id.matched, "噪声不应命中: {id:?}");
            assert_eq!(id.speaker_id, None);

            // Case 4：超短音频 → Skipped(TooShort)，不计分
            let id = rec.identify(&vec![0.1_f32; 3_200], 16_000).unwrap();
            assert_eq!(
                id.skipped,
                Some(SkipReason::TooShort {
                    duration_ms: 200.0,
                    min_ms: 1000.0
                })
            );
            assert_eq!(id.speaker_id, None);

            // Case 5：多人注册下识别另一注册者 → leijun
            let lj = read("leijun-test-sr-1.wav");
            let id = rec.identify(lj.samples(), lj.sample_rate() as u32).unwrap();
            assert!(id.matched, "已注册者应命中: {id:?}");
            assert_eq!(id.speaker_id.as_deref(), Some("leijun"));

            // 移除注册：索引与档案同步清除
            assert!(rec.remove_speaker("fangjun").unwrap());
            assert!(!rec.is_registered("fangjun"));
            assert_eq!(rec.num_registered(), 1);
        });
    }
}
