/// 语音会话 ASR 后端：先按 `backend` 分派 audiocpp sidecar，sherpa 再按
/// `model_type` 分派流式（zipformer/paraformer）/ 离线（SenseVoice/Whisper/Qwen3-ASR ONNX）。
///
/// Streaming variant 严格透传 `AsrEngine` 现有调用（feed/decode_loop/create_stream/尾静音 flush），
/// 保证流式路径行为零变化；Offline/Audiocpp variant 累积 16k PCM 整段，复用 RMS 说完判定后整句转写。
use crate::asr::config::ResolvedAsrConfig;
use crate::asr::offline::OfflineAsrEngine;
use crate::asr::{AsrEngine, AsrReaction};
use sherpa_onnx::OnlineStream;

/// ASR 后端（会话持有单一字段，非法状态不可表示）。
pub(crate) enum AsrBackend {
    /// 流式 zipformer/paraformer：逐块 feed + decode_loop（边说边出）。
    Streaming {
        engine: AsrEngine,
        stream: OnlineStream,
    },
    /// 离线 SenseVoice/Whisper/Qwen3-ASR（sherpa）：缓冲整段 + 说完后整句转写。
    Offline {
        engine: OfflineAsrEngine,
        pcm: OfflinePcm,
    },
    /// audio.cpp sidecar（Qwen3-ASR GGUF）：与 Offline 臂同构的整段语义
    /// （缓冲 + 说完判定），finalize 走 HTTP `/v1/audio/transcriptions`。
    Audiocpp {
        engine: crate::audiocpp::client::AudiocppAsr,
        pcm: OfflinePcm,
    },
}

impl AsrBackend {
    /// 按后端 + 模型族构造对应后端（`backend` 优先于 `is_streaming` 分派；
    /// sherpa 路径行为零变化）。
    pub(crate) fn new(cfg: &ResolvedAsrConfig) -> Result<Self, String> {
        if cfg.backend == crate::asr::config::AsrBackendKind::Audiocpp {
            let engine = crate::audiocpp::client::AudiocppAsr::new(cfg.clone())?;
            return Ok(Self::Audiocpp {
                engine,
                pcm: OfflinePcm::default(),
            });
        }
        if cfg.model_type.is_streaming() {
            let engine = AsrEngine::new(cfg.clone())?;
            let stream = engine.create_stream(cfg.hotwords.as_deref());
            Ok(Self::Streaming { engine, stream })
        } else {
            let engine = OfflineAsrEngine::new(cfg.clone())?;
            Ok(Self::Offline {
                engine,
                pcm: OfflinePcm::default(),
            })
        }
    }

    /// 测试构造：audiocpp 臂直连 stub server（不 preflight、不 spawn 进程）。
    #[cfg(test)]
    pub(crate) fn new_audiocpp_with_base_url(cfg: &ResolvedAsrConfig, base_url: &str) -> Self {
        Self::Audiocpp {
            engine: crate::audiocpp::client::AudiocppAsr::new_with_base_url(cfg.clone(), base_url),
            pcm: OfflinePcm::default(),
        }
    }

    /// 喂一块 16k chunk。`speech_active` 表示该块是否超过 RMS 音量门限（仅离线用于 `speech_seen` 守卫）。
    pub(crate) fn feed_chunk(&mut self, chunk: &[f32], speech_active: bool) {
        match self {
            Self::Streaming { engine, stream } => engine.feed(stream, chunk),
            Self::Offline { pcm, .. } | Self::Audiocpp { pcm, .. } => {
                pcm.push(chunk, speech_active)
            }
        }
    }

    /// 处理已喂入的音频（流式：decode_loop 产出 partial/final；离线：空操作，无 partial）。
    pub(crate) fn decode_into(&self, reaction: &mut dyn AsrReaction) {
        if let Self::Streaming { engine, stream } = self {
            let _ = engine.decode_loop(stream, reaction);
        }
    }

    /// 强制结束当前句取最终文本：流式走原 force_finalize 逻辑；离线/sidecar 走整段转写。
    pub(crate) fn finalize(&mut self, cfg: &ResolvedAsrConfig) -> String {
        match self {
            Self::Streaming { engine, stream } => {
                let tail = vec![0.0f32; (cfg.sample_rate as usize) / 2];
                engine.feed(stream, &tail);
                engine.finish(stream);
                while engine.is_ready(stream) {
                    engine.decode(stream);
                }
                engine
                    .get_result(stream)
                    .map(|r| engine.punctuate_text(&r.text))
                    .unwrap_or_default()
            }
            Self::Offline { engine, pcm } => {
                // 跟听窗口静音时不该跑推理：缓冲无语音 → 直接清空返回（成本可忽略）
                if !pcm.has_speech() {
                    pcm.clear();
                    return String::new();
                }
                let samples = pcm.take();
                match engine.transcribe_samples(&samples, cfg.sample_rate) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::debug!("[voice] 离线整句转写失败，保持聆听: {e}");
                        String::new()
                    }
                }
            }
            Self::Audiocpp { engine, pcm } => {
                // 与 Offline 臂同款「无语音直接清空」守卫；网络/进程抖动不杀会话
                // （对齐 Offline 臂容错语义，debug 日志后保持聆听）
                if !pcm.has_speech() {
                    pcm.clear();
                    return String::new();
                }
                let samples = pcm.take();
                match engine.transcribe(&samples, cfg.sample_rate) {
                    Ok(text) => text,
                    Err(e) => {
                        tracing::debug!("[voice] audiocpp 整句转写失败，保持聆听: {e}");
                        String::new()
                    }
                }
            }
        }
    }

    /// 新一句前复位：流式重建流（丢上轮识别状态）；离线/sidecar 清空缓冲。
    pub(crate) fn reset(&mut self, cfg: &ResolvedAsrConfig) {
        match self {
            Self::Streaming { engine, stream } => {
                *stream = engine.create_stream(cfg.hotwords.as_deref());
            }
            Self::Offline { pcm, .. } | Self::Audiocpp { pcm, .. } => pcm.clear(),
        }
    }
}

/// 离线会话 PCM 缓冲（16k mono）：纯逻辑、可无引擎单测。
#[derive(Default)]
pub(crate) struct OfflinePcm {
    pending: Vec<f32>,
    /// 缓冲期间是否出现过超过 RMS 门限的说话块（防静音重复转写空段）
    speech_seen: bool,
}

impl OfflinePcm {
    pub(crate) fn push(&mut self, chunk: &[f32], speech_active: bool) {
        self.pending.extend_from_slice(chunk);
        self.speech_seen |= speech_active;
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.speech_seen = false;
    }

    /// 取走缓冲（转写用），同时复位 `speech_seen`（消费后即复位）。
    pub(crate) fn take(&mut self) -> Vec<f32> {
        self.speech_seen = false;
        std::mem::take(&mut self.pending)
    }

    pub(crate) fn has_speech(&self) -> bool {
        self.speech_seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::config::{AsrBackendKind, AsrModelKind, ResolvedAsrConfig};
    use std::path::PathBuf;

    fn cfg_with(kind: AsrModelKind) -> ResolvedAsrConfig {
        // 所有文件字段都指向不存在的路径，避免默认值指向真实模型目录导致误走 create
        let base = PathBuf::from("/nonexistent-asr");
        ResolvedAsrConfig {
            model_type: kind,
            model_dir: base.clone(),
            model: Some(base.join("model.onnx")),
            encoder: base.join("encoder.onnx"),
            decoder: base.join("decoder.onnx"),
            joiner: base.join("joiner.onnx"),
            tokens: base.join("tokens.txt"),
            ..ResolvedAsrConfig::default()
        }
    }

    /// 离线族（SenseVoice）：应走 OfflineAsrEngine（报「缺少模型文件」而非「不支持流式」）。
    #[test]
    fn test_asr_backend_new_offline_sensevoice() {
        let cfg = cfg_with(AsrModelKind::SenseVoice);
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(err.contains("缺少模型文件"), "err: {err}");
    }

    /// 离线族（Whisper）：同 SenseVoice，走离线引擎。
    #[test]
    fn test_asr_backend_new_offline_whisper() {
        let cfg = cfg_with(AsrModelKind::Whisper);
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(err.contains("缺少模型文件"), "err: {err}");
    }

    /// 流式族（Zipformer）：应走 AsrEngine（报 install-model 提示下载）。
    #[test]
    fn test_asr_backend_new_streaming_zipformer() {
        let cfg = cfg_with(AsrModelKind::Zipformer);
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(err.contains("install-model"), "err: {err}");
    }

    /// 离线族（Qwen3-ASR）：族自适应自动走 Offline 臂（报缺少 tokenizer 目录，
    /// 而非流式引擎的「不支持实时流式」）——钉住 is_streaming 白名单不含 qwen3。
    #[test]
    fn test_asr_backend_new_offline_qwen3() {
        let cfg = cfg_with(AsrModelKind::Qwen3Asr);
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(
            err.contains("tokenizer") || err.contains("缺少模型文件"),
            "qwen3 应走离线引擎并报模型缺失，实际: {err}"
        );
    }

    /// 流式族（Paraformer）：同 Zipformer 走 AsrEngine 流式分支。
    #[test]
    fn test_asr_backend_new_streaming_paraformer() {
        let cfg = cfg_with(AsrModelKind::Paraformer);
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(err.contains("install-model"), "err: {err}");
    }

    /// audiocpp 后端（Qwen3-ASR GGUF）：backend 优先于族分派——走 Audiocpp 臂，
    /// preflight 报缺 GGUF（而非 sherpa 离线臂的 tokenizer 报错或流式臂的
    /// 「不支持流式」），钉住 audiocpp 不落入 sherpa 两臂。
    #[test]
    fn test_asr_backend_new_audiocpp_qwen3() {
        let mut cfg = cfg_with(AsrModelKind::Qwen3Asr);
        cfg.backend = AsrBackendKind::Audiocpp;
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(err.contains("缺少模型文件"), "err: {err}");
        assert!(!err.contains("tokenizer"), "不应走 sherpa 离线臂: {err}");

        // 流式族 + audiocpp 后端同样先走 Audiocpp 臂（resolve 层会 fail-fast
        // 拦组合，这里钉住后端优先的分派顺序）
        let mut cfg = cfg_with(AsrModelKind::Zipformer);
        cfg.backend = AsrBackendKind::Audiocpp;
        let err = AsrBackend::new(&cfg).err().unwrap();
        assert!(
            err.contains("不支持 audiocpp 后端") || err.contains("缺少模型文件"),
            "err: {err}"
        );
    }

    /// stub server 全链路：feed_chunk 缓冲 → finalize 走 transcriptions 返回文本；
    /// 无语音块时 finalize 直接清空返回空（不发请求）。
    #[test]
    fn test_audiocpp_backend_finalize_via_stub() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(request.as_reader(), &mut body);
                let _ = request.respond(tiny_http::Response::from_string(
                    r#"{"text":" stub 识别文本 ","timing":{}}"#,
                ));
            }
        });

        let mut cfg = cfg_with(AsrModelKind::Qwen3Asr);
        cfg.backend = AsrBackendKind::Audiocpp;
        let mut backend =
            AsrBackend::new_audiocpp_with_base_url(&cfg, &format!("http://127.0.0.1:{port}"));

        // 静音缓冲 → 不发请求直接返回空
        backend.feed_chunk(&[0.0, 0.0], false);
        assert_eq!(backend.finalize(&cfg), "", "无语音守卫");

        // 语音缓冲 → finalize 返回 stub 文本（trim 后）
        backend.feed_chunk(&[0.1, 0.2, 0.3], true);
        assert_eq!(backend.finalize(&cfg), "stub 识别文本");

        // reset 清空缓冲：再次 finalize（无新语音）返回空
        backend.feed_chunk(&[0.5], true);
        backend.reset(&cfg);
        assert_eq!(backend.finalize(&cfg), "");
    }

    /// 真机：读用户已装 audiocpp Qwen3-ASR 模型，喂示例音频 → finalize 应得非空文本。
    #[test]
    #[ignore = "需要已安装 audiocpp Qwen3-ASR 模型（settings 配置为当前）"]
    fn test_audiocpp_backend_finalize_transcribes_real_model() {
        use crate::asr::config;
        let cfg = config::resolve(None, None).unwrap();
        if cfg.backend != AsrBackendKind::Audiocpp || cfg.model_type != AsrModelKind::Qwen3Asr {
            eprintln!("跳过：当前 ASR 不是 audiocpp Qwen3-ASR");
            return;
        }
        let mut backend = AsrBackend::new(&cfg).unwrap();
        let Some(wav) = crate::asr::default_test_wav(&cfg.model_dir) else {
            eprintln!("跳过：模型目录无示例音频");
            return;
        };
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).unwrap();
        backend.feed_chunk(wave.samples(), true);
        let text = backend.finalize(&cfg);
        assert!(
            !text.trim().is_empty(),
            "audiocpp 整句转写应非空，实际: {text}"
        );
    }

    /// OfflinePcm 纯逻辑：push 累积 / speech_seen OR / clear / take 排空复位。
    #[test]
    fn test_offline_pcm_logic() {
        let mut pcm = OfflinePcm::default();
        assert!(!pcm.has_speech());
        assert_eq!(pcm.take().len(), 0);

        pcm.push(&[1.0, 2.0], true);
        pcm.push(&[3.0], false);
        assert!(pcm.has_speech());
        assert_eq!(pcm.take(), vec![1.0, 2.0, 3.0]);
        // take 后 speech_seen 复位、缓冲排空
        assert!(!pcm.has_speech());
        assert_eq!(pcm.take().len(), 0);

        // 静音块不置 speech_seen
        pcm.push(&[0.0], false);
        assert!(!pcm.has_speech());

        // clear 复位
        pcm.push(&[9.0], true);
        pcm.clear();
        assert!(!pcm.has_speech());
        assert_eq!(pcm.take().len(), 0);
    }

    /// 真机：读用户已装 SenseVoice 模型，分块喂示例音频 → finalize 应得非空文本。
    #[test]
    #[ignore = "需要已安装 SenseVoice 模型（settings 配置为当前）"]
    fn test_offline_backend_finalize_transcribes_real_model() {
        use crate::asr::config;
        let cfg = config::resolve(None, None).unwrap();
        if cfg.model_type != AsrModelKind::SenseVoice {
            eprintln!("跳过：当前 ASR 模型不是 SenseVoice");
            return;
        }
        let mut backend = AsrBackend::new(&cfg).unwrap();
        let wav = crate::asr::default_test_wav(&cfg.model_dir).expect("模型自带示例音频");
        let wave = sherpa_onnx::Wave::read(&wav.to_string_lossy()).unwrap();
        // 整段当说话块喂入（模拟一次完整说话）
        backend.feed_chunk(wave.samples(), true);
        let text = backend.finalize(&cfg);
        assert!(!text.trim().is_empty(), "离线整句转写应非空，实际: {text}");
    }
}
