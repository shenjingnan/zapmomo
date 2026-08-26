use std::cell::Cell;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use super::AudiocppError;
use super::families::{AudiocppFamilyDesc, VoiceSemantics, family_desc};
use crate::tts::TtsVoiceParams;
use crate::tts::config::ResolvedTtsConfig;
use base64::Engine as _;

/// 连接超时（server 未起/已退出时快速失败）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 合成请求超时（长文本合成可达数秒，留足余量）。流式请求同为总时长上限
/// （单句长度有限，不需区分首块/整段超时）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// 错误响应体截断上限（避免超长 body 刷屏）。
const ERROR_BODY_MAX: usize = 500;
/// SSE 流式文本分块粒度（`options.text_chunk_size`）。上游默认 160 时句长
/// ≤160 字只切一块，流式退化为整段一次性返回（阶段 1 实测首块 ≈ 总耗时）；
/// 实测 40 时 120 字长句首块延迟 auto -77% / clone -64%，20 与 40 等价
/// （server 侧有效下限），总耗时 +21~27%（每块固定启动开销）。
const STREAM_TEXT_CHUNK_SIZE: i32 = 40;

/// audio.cpp TTS 后端：经 sidecar HTTP 合成。
///
/// 生产构造（[`Self::new`]）持有 server 租约（进程随租约生命周期）；测试构造
/// （[`Self::new_with_base_url`]）直连 stub server，绕过进程管理。
pub struct AudiocppTts {
    cfg: ResolvedTtsConfig,
    /// 模型族描述（构造时查表一次；请求体/采样率/音色映射都来自它）
    desc: &'static AudiocppFamilyDesc,
    base_url: String,
    /// 持有租约（保活 server）；Drop 释放。测试构造（直连 stub）为 None。
    _lease: Option<super::server::ServerLease>,
    client: reqwest::blocking::Client,
    /// 输出采样率缓存：初值为族固定采样率（如 24000），首响应 wav 头校准
    sample_rate: Cell<i32>,
}

impl AudiocppTts {
    /// 生产构造：查模型族表 → 定位引擎 → lease server（含 spawn + 健康检查）。
    pub fn new(cfg: ResolvedTtsConfig) -> Result<Self, String> {
        let desc = lookup_desc(&cfg)?;
        let lease = super::server::lease(&cfg).map_err(|e| e.to_user_message())?;
        let base_url = lease.base_url();
        Ok(Self {
            cfg,
            desc,
            base_url,
            _lease: Some(lease),
            client: build_client()?,
            sample_rate: Cell::new(desc.sample_rate),
        })
    }

    /// 测试构造：直连指定 base_url 的 stub server（不 spawn 进程、不持租约）。
    pub fn new_with_base_url(cfg: ResolvedTtsConfig, base_url: &str) -> Self {
        let desc = lookup_desc(&cfg).expect("测试构造要求合法 audiocpp 模型族");
        Self {
            cfg,
            desc,
            base_url: base_url.to_string(),
            _lease: None,
            client: build_client().expect("构建 HTTP 客户端"),
            sample_rate: Cell::new(desc.sample_rate),
        }
    }

    /// 后端配置快照（`TtsEngine::config` 门面转发）。
    pub fn config(&self) -> &ResolvedTtsConfig {
        &self.cfg
    }

    /// 输出采样率（初值为族固定值，合成后按响应 wav 头校准）。
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate.get()
    }

    /// 合成文本为 PCM（f32 mono）。语速处理与 sherpa 后端一致：模型按 1.0
    /// 合成，输出重采样实现（复用 `tts::apply_speed_to_samples`）。
    ///
    /// 音色字段按族语义映射（见 [`apply_voice_fields`]）。
    pub fn synthesize(
        &self,
        text: &str,
        speed: f32,
        voice: &TtsVoiceParams,
    ) -> Result<Vec<f32>, String> {
        let mut body = serde_json::json!({
            "model": self.desc.model_id,
            "input": text,
        });
        // 族差异请求选项（voxcpm2 的 retry_badcase 对整段路径同样是硬约束）
        let options = self.desc.request_options();
        if options.as_object().is_some_and(|m| !m.is_empty()) {
            body["options"] = options;
        }
        apply_voice_fields(&mut body, self.desc, voice).map_err(|e| e.to_user_message())?;
        let resp = self
            .client
            .post(format!("{}/v1/audio/speech", self.base_url))
            .json(&body)
            .send()
            .map_err(|e| AudiocppError::Connection(e.to_string()).to_user_message())?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().unwrap_or_default();
            return Err(AudiocppError::HttpStatus {
                status: status.as_u16(),
                body: truncate(&body_text, ERROR_BODY_MAX),
            }
            .to_user_message());
        }
        let bytes = resp
            .bytes()
            .map_err(|e| AudiocppError::Connection(e.to_string()).to_user_message())?;
        let (samples, rate) = decode_wav(&bytes).map_err(|e| e.to_user_message())?;
        self.sample_rate.set(rate);
        crate::tts::apply_speed_to_samples(&samples, rate, speed)
    }

    /// 该模型族是否支持 SSE 流式合成（族静态；`TtsEngine` 门面转发）。
    pub fn supports_streaming(&self) -> bool {
        self.desc.supports_streaming
    }

    /// SSE 流式合成：`speech.audio.delta`（base64 PCM 16-bit LE mono）逐块回调，
    /// `speech.audio.done` → `data: [DONE]` 结束。
    ///
    /// - `on_chunk(samples, sample_rate)` 返回 `false` 时**停止读取**并返回
    ///   `Ok(())`（协作取消：drop Response 断开连接，server 停止后续块生成）；
    ///   正常完成（含取消）与流结束同返回 `Ok(())`，由调用方区分语义。
    /// - 采样率：事件无采样率字段（阶段 1 实测），用族默认值（`sample_rate()`
    ///   缓存的校准值优先）；后续若校准变化由整段路径 wav 头负责。
    /// - 流内 `{"type":"error"}` 事件（如 busy_timeout）→ [`AudiocppError::StreamEvent`]。
    /// - SSE 解析对齐 `llm::http` 既有 blocking 先例（`BufReader::lines` + `data: ` 前缀）。
    pub fn synthesize_streaming(
        &self,
        text: &str,
        voice: &TtsVoiceParams,
        on_chunk: &mut dyn FnMut(&[f32], i32) -> bool,
    ) -> Result<(), AudiocppError> {
        if !self.desc.supports_streaming {
            return Err(AudiocppError::StreamingUnsupported(
                self.desc.model_id.to_string(),
            ));
        }
        // 流式 options = 族差异项（voxcpm2 的 retry_badcase 等）+ 文本分块粒度
        // （粒度是伪流式族的收益充要条件；帧级流式族忽略亦无害）
        let mut options = self.desc.request_options();
        options["text_chunk_size"] = serde_json::json!(STREAM_TEXT_CHUNK_SIZE);
        let mut body = serde_json::json!({
            "model": self.desc.model_id,
            "input": text,
            "response_format": "pcm",
            "stream_format": "sse",
            "options": options,
        });
        apply_voice_fields(&mut body, self.desc, voice)?;
        let resp = self
            .client
            .post(format!("{}/v1/audio/speech", self.base_url))
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .map_err(|e| AudiocppError::Connection(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().unwrap_or_default();
            return Err(AudiocppError::HttpStatus {
                status: status.as_u16(),
                body: truncate(&body_text, ERROR_BODY_MAX),
            });
        }
        let rate = self.sample_rate.get();
        for line in BufReader::new(resp).lines() {
            let line = line.map_err(|e| AudiocppError::Connection(e.to_string()))?;
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }
            let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            match event["type"].as_str() {
                // 音频增量：载荷键为 audio（阶段 1 实测事件 dump 确认）
                Some("speech.audio.delta") => {
                    if let Some(b64) = event["audio"].as_str().filter(|s| !s.is_empty()) {
                        let samples = decode_pcm_chunk(b64)?;
                        if !on_chunk(&samples, rate) {
                            // 协作取消：停止读取，drop Response 断连接
                            return Ok(());
                        }
                    }
                }
                Some("speech.audio.done") => {} // 等待 [DONE] 收尾
                Some(other) if other.contains("error") => {
                    return Err(AudiocppError::StreamEvent(truncate(data, ERROR_BODY_MAX)));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// 把音色参数按族语义映射进请求体（整段与流式两条路径共用）。
///
/// - 固定具名音色（pocket）：`Named`/`Sid` → `voice`；`Reference` 报错；
/// - 参考音频克隆（omnivoice/voxcpm2）：`Reference` → `voice_ref`+`reference_text`
///   （本地路径，sidecar 同机可读）；`Named` → 视 `allows_named_voice` 透传
///   `voice`（omnivoice 走 server 端 preset/voice_dir 通道）或提前拦截（voxcpm2
///   上游仅接受 speaker reference）；`Sid` → 省略 voice 字段（server auto voice）。
fn apply_voice_fields(
    body: &mut serde_json::Value,
    desc: &AudiocppFamilyDesc,
    voice: &TtsVoiceParams,
) -> Result<(), AudiocppError> {
    match (desc.voice_semantics, voice) {
        (VoiceSemantics::FixedNamed(_), TtsVoiceParams::Named(v)) => {
            body["voice"] = serde_json::json!(v);
        }
        (VoiceSemantics::FixedNamed(default), TtsVoiceParams::Sid(_)) => {
            body["voice"] = serde_json::json!(default);
        }
        (VoiceSemantics::FixedNamed(default), TtsVoiceParams::Reference { .. }) => {
            return Err(AudiocppError::UnsupportedVoice(format!(
                "该 audio.cpp 模型为固定音色（{default}），不支持参考音频克隆"
            )));
        }
        (
            VoiceSemantics::ReferenceClone,
            TtsVoiceParams::Reference {
                wav_path,
                reference_text,
            },
        ) => {
            body["voice_ref"] = serde_json::json!(wav_path.to_string_lossy());
            body["reference_text"] = serde_json::json!(reference_text);
        }
        (
            VoiceSemantics::ReferenceCloneRequired,
            TtsVoiceParams::Reference {
                wav_path,
                reference_text,
            },
        ) => {
            body["voice_ref"] = serde_json::json!(wav_path.to_string_lossy());
            body["reference_text"] = serde_json::json!(reference_text);
        }
        (VoiceSemantics::ReferenceCloneRequired, TtsVoiceParams::Sid(_)) => {
            return Err(AudiocppError::UnsupportedVoice(
                "Qwen3-TTS 需要克隆音色：请先在音色库选择或录制一个音色".to_string(),
            ));
        }
        (VoiceSemantics::ReferenceCloneRequired, TtsVoiceParams::Named(_)) => {
            return Err(AudiocppError::UnsupportedVoice(format!(
                "{} 仅支持参考音频克隆（speaker reference），不支持具名音色",
                desc.model_id
            )));
        }
        (VoiceSemantics::ReferenceClone, TtsVoiceParams::Named(v)) => {
            if desc.allows_named_voice {
                body["voice"] = serde_json::json!(v);
            } else {
                return Err(AudiocppError::UnsupportedVoice(format!(
                    "{} 仅支持参考音频克隆（speaker reference），不支持具名音色",
                    desc.model_id
                )));
            }
        }
        // 省略 voice 字段 → server auto voice（无显式音色时的可用兜底）
        (VoiceSemantics::ReferenceClone, TtsVoiceParams::Sid(_)) => {}
    }
    Ok(())
}

/// 查模型族描述；sherpa-only kind 配 audiocpp 后端的非法组合报错。
fn lookup_desc(cfg: &ResolvedTtsConfig) -> Result<&'static AudiocppFamilyDesc, String> {
    family_desc(cfg.model_type).ok_or_else(|| {
        format!(
            "模型类型 {} 不支持 audiocpp 后端（请检查 [tts].model_type 与 backend 组合）",
            cfg.model_type.as_str()
        )
    })
}

fn build_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        // sidecar 恒为 127.0.0.1 回环：禁用系统/环境代理（否则代理会拦
        // localhost 请求返回 5xx）
        .no_proxy()
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败: {e}"))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// 解码 wav bytes 为 (f32 mono 样本, 采样率)。
///
/// 支持 16-bit PCM 与 f32（audio.cpp server 实测返回 16-bit PCM mono 24kHz）。
pub(crate) fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, i32), AudiocppError> {
    let reader = hound::WavReader::new(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|e| AudiocppError::DecodeWav(format!("非 wav 数据: {e}")))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
        hound::SampleFormat::Int => {
            let max = (1i32 << (spec.bits_per_sample.saturating_sub(1))).max(1) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max).unwrap_or(0.0))
                .collect()
        }
    };
    if samples.is_empty() {
        return Err(AudiocppError::DecodeWav("wav 数据为空".to_string()));
    }
    Ok((samples, spec.sample_rate as i32))
}

/// 解码 SSE 流式 delta 载荷：base64 → bytes → i16 LE mono → f32（/32768 归一）。
///
/// 位宽为阶段 1 实测结论（i16 RMS 落在合理语音电平、按 f32 解释为 NaN），
/// 与 wav 路径（audio.cpp server 返回 16-bit PCM mono）同源。
fn decode_pcm_chunk(b64: &str) -> Result<Vec<f32>, AudiocppError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| AudiocppError::DecodeWav(format!("base64 解码失败: {e}")))?;
    if bytes.len() % 2 != 0 {
        return Err(AudiocppError::DecodeWav(format!(
            "pcm 分块长度 {} 不是 2 字节对齐（i16）",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::config::TtsBackendKind;

    fn audiocpp_cfg() -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Pocket,
            ..ResolvedTtsConfig::default()
        }
    }

    fn omnivoice_cfg() -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Omnivoice,
            ..ResolvedTtsConfig::default()
        }
    }

    fn voxcpm2_cfg() -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Voxcpm2,
            ..ResolvedTtsConfig::default()
        }
    }

    fn qwen3_06_cfg() -> ResolvedTtsConfig {
        ResolvedTtsConfig {
            backend: TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Qwen3Tts06,
            ..ResolvedTtsConfig::default()
        }
    }

    // ---------- 纯函数：wav 解码 ----------

    #[test]
    fn test_decode_wav_roundtrip_from_write_wav() {
        // 用项目写方向生成 wav → 解码往返（f32 写 f32，误差 ≤ 1e-4）
        let base = tempfile::tempdir().unwrap();
        let path = base.path().join("t.wav");
        let samples: Vec<f32> = vec![0.0, 0.25, -0.25, 0.5, -0.5, 1.0, -1.0, 0.123, -0.456, 0.789];
        crate::audio::write_wav_f32(&path, 24000, &samples).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (out, rate) = decode_wav(&bytes).unwrap();
        assert_eq!(rate, 24000);
        assert_eq!(out.len(), samples.len());
        // hound 及时差：写入 IEEE-754 位模式 → 读取 → 可能会有最低位数误差
        for (a, b) in out.iter().zip(&samples) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    #[test]
    fn test_decode_wav_rejects_garbage() {
        let err = decode_wav(b"not a wav at all").unwrap_err();
        assert!(matches!(err, AudiocppError::DecodeWav(_)));
        assert!(err.to_user_message().contains("解码合成音频失败"));
        // 空 wav 数据（RIFF 头但无 fmt/data chunk）
        let err = decode_wav(b"RIFF\x00\x00\x00\x00WAVE").unwrap_err();
        assert!(matches!(err, AudiocppError::DecodeWav(_)));
    }

    #[test]
    fn test_truncate_respects_char_boundary() {
        assert_eq!(truncate("abc", 10), "abc");
        let t = truncate("你好世界你好世界", 9); // 9 落在多字节字符中间
        assert!(t.ends_with('…'));
        assert!(t.chars().count() < 7);
    }

    // ---------- 集成：tiny_http stub server（无真实引擎） ----------

    /// 起 stub server：/v1/audio/speech 返回固定 wav；其余路径 404。
    /// 返回 (base_url, 收到的请求体列表, 服务线程句柄)。
    fn spawn_stub() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        std::thread::JoinHandle<()>,
    ) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = received.clone();
        let handle = std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let url = request.url().to_string();
                let json: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                received_clone.lock().unwrap().push(json);
                let response = if url == "/v1/audio/speech" {
                    let samples = vec![0.2f32; 2400];
                    let base = tempfile::tempdir().unwrap();
                    let path = base.path().join("resp.wav");
                    crate::audio::write_wav_f32(&path, 24000, &samples).unwrap();
                    let bytes = std::fs::read(&path).unwrap();
                    // tempdir 在块结束时销毁，但 bytes 已读出
                    tiny_http::Response::from_data(bytes)
                } else {
                    tiny_http::Response::from_string("Not Found").with_status_code(404)
                };
                let _ = request.respond(response);
            }
        });
        (format!("http://127.0.0.1:{port}"), received, handle)
    }

    #[test]
    fn test_synthesize_against_stub_full_request_flow() {
        let (base_url, received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(audiocpp_cfg(), &base_url);
        let out = tts
            .synthesize("hello world", 1.0, &TtsVoiceParams::Named("alba".into()))
            .unwrap();
        assert_eq!(out.len(), 2400);
        assert_eq!(tts.sample_rate(), 24000, "首响应校准采样率");
        // 请求体断言：OpenAI 风格三件套
        let reqs = received.lock().unwrap();
        let last = reqs.last().unwrap();
        assert_eq!(last["model"], "pocket-tts-english");
        assert_eq!(last["input"], "hello world");
        assert_eq!(last["voice"], "alba");
    }

    #[test]
    fn test_synthesize_voice_param_mapping() {
        let (base_url, _received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(audiocpp_cfg(), &base_url);
        // Sid → 后端默认音色 alba
        tts.synthesize("x", 1.0, &TtsVoiceParams::Sid(0)).unwrap();
        // Reference → UnsupportedVoice 错误（连接前拦截，无需 stub 也可测）
        let err = tts
            .synthesize(
                "x",
                1.0,
                &TtsVoiceParams::Reference {
                    wav_path: std::path::PathBuf::from("/r.wav"),
                    reference_text: "t".into(),
                },
            )
            .unwrap_err();
        assert!(err.contains("固定音色"), "err: {err}");
    }

    /// omnivoice（克隆族）请求体三态：Reference → voice_ref+reference_text；
    /// Named → voice 透传；Sid → 无 voice 字段（server auto voice）。
    #[test]
    fn test_synthesize_omnivoice_request_body() {
        let (base_url, received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base_url);

        tts.synthesize(
            "x",
            1.0,
            &TtsVoiceParams::Reference {
                wav_path: std::path::PathBuf::from("/voices/me.wav"),
                reference_text: "参考转写".into(),
            },
        )
        .unwrap();
        let reqs = received.lock().unwrap();
        let last = reqs.last().unwrap();
        assert_eq!(last["model"], "omnivoice", "model 字段按族");
        assert_eq!(
            last["voice_ref"].as_str().unwrap().replace('\\', "/"),
            "/voices/me.wav"
        );
        assert_eq!(last["reference_text"], "参考转写");
        assert!(last.get("voice").is_none(), "Reference 不应带 voice 字段");
        drop(reqs);

        tts.synthesize("x", 1.0, &TtsVoiceParams::Named("demo_01_man".into()))
            .unwrap();
        let last = received.lock().unwrap().last().unwrap().clone();
        assert_eq!(last["voice"], "demo_01_man", "Named 透传 voice");
        assert!(last.get("voice_ref").is_none());

        tts.synthesize("x", 1.0, &TtsVoiceParams::Sid(0)).unwrap();
        let last = received.lock().unwrap().last().unwrap().clone();
        assert!(
            last.get("voice").is_none() && last.get("voice_ref").is_none(),
            "Sid 省略全部音色字段（auto voice）"
        );
    }

    /// qwen3_tts（强制克隆族）：Reference 正常映射；Sid/Named 提前拦截。
    #[test]
    fn test_synthesize_qwen3_tts_voice_semantics() {
        let (base_url, received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(qwen3_06_cfg(), &base_url);

        // Reference -> voice_ref + reference_text（与 omnivoice 同款映射）
        tts.synthesize(
            "你好",
            1.0,
            &TtsVoiceParams::Reference {
                wav_path: std::path::PathBuf::from("/voices/me.wav"),
                reference_text: "参考转写".into(),
            },
        )
        .unwrap();
        let body = received.lock().unwrap().last().unwrap().clone();
        assert_eq!(body["model"], "qwen3-tts-0.6b");
        assert_eq!(
            body["voice_ref"].as_str().unwrap().replace('\\', "/"),
            "/voices/me.wav"
        );
        assert_eq!(body["reference_text"], "参考转写");

        // Sid -> 提前拦截（上游 Base 版无 auto voice）
        let err = tts
            .synthesize("x", 1.0, &TtsVoiceParams::Sid(0))
            .unwrap_err();
        assert!(err.contains("需要克隆音色"), "err: {err}");

        // Named -> 提前拦截（Base 版仅接受 speaker reference）
        let err = tts
            .synthesize("x", 1.0, &TtsVoiceParams::Named("v".into()))
            .unwrap_err();
        assert!(err.contains("参考音频克隆"), "err: {err}");
    }

    /// 非法组合（sherpa kind + audiocpp 后端）在构造期报错，不发起连接。
    #[test]
    fn test_new_rejects_sherpa_kind() {
        let mut cfg = ResolvedTtsConfig::default();
        cfg.backend = TtsBackendKind::Audiocpp; // model_type 缺省 Zipvoice
        let err = lookup_desc(&cfg).unwrap_err();
        assert!(err.contains("不支持 audiocpp 后端"), "err: {err}");
    }

    #[test]
    fn test_synthesize_speed_applies_resampling() {
        let (base_url, _received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(audiocpp_cfg(), &base_url);
        // stub 返回 2400 样本（24000Hz 即 0.1s）；speed 2.0 → ≈1200 样本
        let out = tts.synthesize("x", 2.0, &TtsVoiceParams::Sid(0)).unwrap();
        assert!(
            (out.len() as i64 - 1200).abs() <= 8,
            "speed 2.0 len={}",
            out.len()
        );
    }

    #[test]
    fn test_synthesize_connection_refused_reports_connection() {
        // 连一个必然未监听的端口 → Connection 错误文案
        let tts = AudiocppTts::new_with_base_url(audiocpp_cfg(), "http://127.0.0.1:1");
        let err = tts
            .synthesize("x", 1.0, &TtsVoiceParams::Sid(0))
            .unwrap_err();
        assert!(err.contains("无法连接 audiocpp_server"), "err: {err}");
    }

    #[test]
    fn test_error_variants_user_messages() {
        // 各错误分支的用户文案锚点（HttpStatus/DecodeWav/ModelNotListed 等）
        let e = AudiocppError::HttpStatus {
            status: 500,
            body: "boom".to_string(),
        };
        assert!(e.to_user_message().contains("HTTP 500"));
        assert!(e.to_user_message().contains("boom"));
        let e = AudiocppError::ModelNotListed {
            model_id: "m".to_string(),
        };
        assert!(e.to_user_message().contains("未加载模型 m"));
        let e = AudiocppError::SpawnFailed("x".to_string());
        assert!(e.to_user_message().contains("启动 audiocpp_server 失败"));
        let e = AudiocppError::StartupTimeout {
            timeout_secs: 3,
            stderr_tail: "tail".to_string(),
        };
        assert!(e.to_user_message().contains("启动超时（3s）"));
        assert!(e.to_user_message().contains("tail"));
        let e = AudiocppError::StreamingUnsupported("pocket-tts-english".to_string());
        assert!(e.to_user_message().contains("不支持流式合成"));
        let e = AudiocppError::StreamEvent("busy".to_string());
        assert!(e.to_user_message().contains("流式合成被服务端中断"));
    }

    // ---------- SSE 流式（tiny_http stub，事件格式对齐阶段 1 实测 dump） ----------

    /// i16 样本 → SSE delta 事件行（`{"type":"speech.audio.delta","audio":<base64>}`）。
    fn sse_delta(samples: &[i16]) -> String {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!(
            "data: {}\n\n",
            serde_json::json!({"type": "speech.audio.delta", "audio": b64})
        )
    }

    /// 起 SSE stub：/v1/audio/speech 返回固定事件流（Content-Type: text/event-stream），
    /// 记录（请求体, Accept 头）。返回 (base_url, 记录列表, 线程句柄)。
    fn spawn_stub_sse(
        events: String,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<(serde_json::Value, Option<String>)>>>,
        std::thread::JoinHandle<()>,
    ) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let received_clone = received.clone();
        let handle = std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let accept = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Accept"))
                    .map(|h| h.value.as_str().to_string());
                let json: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
                received_clone.lock().unwrap().push((json, accept));
                if request.url() == "/v1/audio/speech" {
                    let header = tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/event-stream"[..],
                    )
                    .unwrap();
                    let _ = request.respond(
                        tiny_http::Response::from_string(events.clone()).with_header(header),
                    );
                } else {
                    let _ = request
                        .respond(tiny_http::Response::from_string("nf").with_status_code(404));
                }
            }
        });
        (format!("http://127.0.0.1:{port}"), received, handle)
    }

    /// 三块 PCM + done + [DONE] 的标准事件流（块样本数递增便于断言块序）。
    fn standard_events() -> String {
        let mut s = String::new();
        for n in [240u16, 480, 960] {
            s.push_str(&sse_delta(&vec![100i16; n as usize]));
        }
        s.push_str("data: {\"type\":\"speech.audio.done\",\"timing\":{\"ttft_ms\":1}}\n\n");
        s.push_str("data: [DONE]\n\n");
        s
    }

    /// 全链路：请求体（stream 字段/粒度/音色省略）+ Accept 头 + 块序 + 采样率。
    #[test]
    fn test_synthesize_streaming_full_flow_omnivoice() {
        let (base_url, received, _handle) = spawn_stub_sse(standard_events());
        let tts = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base_url);
        assert!(tts.supports_streaming());

        let mut chunks = Vec::new();
        tts.synthesize_streaming("你好", &TtsVoiceParams::Sid(0), &mut |samples, rate| {
            chunks.push((samples.len(), rate));
            true
        })
        .unwrap();

        // 块序即 SSE 事件序，样本数按 i16 归一（每 i16 → 1 f32）
        assert_eq!(
            chunks,
            vec![(240, 24000), (480, 24000), (960, 24000)],
            "三块按序回调"
        );
        let reqs = received.lock().unwrap();
        let (body, accept) = reqs.last().unwrap();
        assert_eq!(body["model"], "omnivoice");
        assert_eq!(body["input"], "你好");
        assert_eq!(body["response_format"], "pcm");
        assert_eq!(body["stream_format"], "sse");
        assert_eq!(body["options"]["text_chunk_size"], 40, "粒度是收益充要条件");
        assert!(
            body.get("voice").is_none() && body.get("voice_ref").is_none(),
            "Sid 省略全部音色字段"
        );
        assert_eq!(accept.as_deref(), Some("text/event-stream"));
    }

    /// 克隆语义映射复用：Reference → voice_ref + reference_text。
    #[test]
    fn test_synthesize_streaming_voice_ref_mapping() {
        let (base_url, received, _handle) = spawn_stub_sse(standard_events());
        let tts = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base_url);
        tts.synthesize_streaming(
            "克隆",
            &TtsVoiceParams::Reference {
                wav_path: std::path::PathBuf::from("/voices/me.wav"),
                reference_text: "参考转写".into(),
            },
            &mut |_, _| true,
        )
        .unwrap();
        let (body, _) = received.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            body["voice_ref"].as_str().unwrap().replace('\\', "/"),
            "/voices/me.wav"
        );
        assert_eq!(body["reference_text"], "参考转写");
    }

    /// 协作取消：回调首块返回 false → Ok(()) 且不再回调（drop Response 断连接）。
    #[test]
    fn test_synthesize_streaming_cooperative_cancel() {
        let (base_url, _received, _handle) = spawn_stub_sse(standard_events());
        let tts = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base_url);
        let mut calls = 0;
        tts.synthesize_streaming("x", &TtsVoiceParams::Sid(0), &mut |_, _| {
            calls += 1;
            false
        })
        .unwrap();
        assert_eq!(calls, 1, "首块取消后不再回调");
    }

    /// 流内错误事件（busy_timeout 等）→ StreamEvent。
    #[test]
    fn test_synthesize_streaming_error_event() {
        let events = format!(
            "{}data: {}\n\n",
            sse_delta(&vec![0i16; 8]),
            serde_json::json!({"type": "error", "message": "busy timeout"})
        );
        let (base_url, _received, _handle) = spawn_stub_sse(events);
        let tts = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base_url);
        let err = tts
            .synthesize_streaming("x", &TtsVoiceParams::Sid(0), &mut |_, _| true)
            .unwrap_err();
        assert!(matches!(err, AudiocppError::StreamEvent(_)));
        assert!(err.to_user_message().contains("busy timeout"));
    }

    /// 非 2xx（offline-mode server 拒绝 SSE 的实测形态）→ HttpStatus。
    #[test]
    fn test_synthesize_streaming_http_error() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let _ = request
                    .respond(tiny_http::Response::from_string("no stream").with_status_code(500));
            }
        });
        let tts =
            AudiocppTts::new_with_base_url(omnivoice_cfg(), &format!("http://127.0.0.1:{port}"));
        let err = tts
            .synthesize_streaming("x", &TtsVoiceParams::Sid(0), &mut |_, _| true)
            .unwrap_err();
        assert!(matches!(err, AudiocppError::HttpStatus { status: 500, .. }));
    }

    /// 非流式族（pocket）：连接前拦截 → StreamingUnsupported。
    #[test]
    fn test_synthesize_streaming_pocket_unsupported() {
        let tts = AudiocppTts::new_with_base_url(audiocpp_cfg(), "http://127.0.0.1:1");
        assert!(!tts.supports_streaming());
        let err = tts
            .synthesize_streaming("x", &TtsVoiceParams::Sid(0), &mut |_, _| true)
            .unwrap_err();
        assert!(matches!(err, AudiocppError::StreamingUnsupported(_)));
    }

    /// pcm 解码往返：已知 i16 LE 字节 → base64 → f32 ≈ v/32768。
    #[test]
    fn test_decode_pcm_chunk_roundtrip() {
        let samples: Vec<i16> = vec![0, 1, -1, 32767, -32768, 12345];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let out = decode_pcm_chunk(&b64).unwrap();
        assert_eq!(out.len(), samples.len());
        for (a, b) in out.iter().zip(&samples) {
            assert!((a - *b as f32 / 32768.0).abs() < 1e-9);
        }
    }

    /// pcm 解码拒绝坏输入：坏 base64 / 奇数字节长度。
    #[test]
    fn test_decode_pcm_chunk_rejects_garbage() {
        let err = decode_pcm_chunk("!!!not-base64!!!").unwrap_err();
        assert!(matches!(err, AudiocppError::DecodeWav(_)));
        // "QQ==" = 1 字节 0x41（非 2 字节对齐）
        let err = decode_pcm_chunk("QQ==").unwrap_err();
        assert!(matches!(err, AudiocppError::DecodeWav(_)));
    }

    // ---------- VoxCPM2：retry_badcase 硬约束 + Named 拦截 + 48kHz ----------

    /// voxcpm2 流式请求体：族选项 retry_badcase=false 与 text_chunk_size 并存，
    /// 分块回调携带族采样率 48kHz。
    #[test]
    fn test_synthesize_streaming_voxcpm2_options() {
        let (base_url, received, _handle) = spawn_stub_sse(standard_events());
        let tts = AudiocppTts::new_with_base_url(voxcpm2_cfg(), &base_url);
        let mut rates = Vec::new();
        tts.synthesize_streaming("你好", &TtsVoiceParams::Sid(0), &mut |_s, rate| {
            rates.push(rate);
            true
        })
        .unwrap();
        assert!(rates.iter().all(|&r| r == 48_000), "voxcpm2 族采样率 48k");
        let (body, _) = received.lock().unwrap().last().unwrap().clone();
        assert_eq!(body["model"], "voxcpm2");
        assert_eq!(body["options"]["retry_badcase"], false, "上游硬约束");
        assert_eq!(body["options"]["text_chunk_size"], 40);
        // omnivoice 不应携带 retry_badcase（族差异项隔离）
        let (base2, recv2, _h2) = spawn_stub_sse(standard_events());
        let omni = AudiocppTts::new_with_base_url(omnivoice_cfg(), &base2);
        omni.synthesize_streaming("x", &TtsVoiceParams::Sid(0), &mut |_, _| true)
            .unwrap();
        let (body2, _) = recv2.lock().unwrap().last().unwrap().clone();
        assert!(body2["options"].get("retry_badcase").is_none());
    }

    /// voxcpm2 整段请求体也带 retry_badcase（streaming-mode server 下非流式
    /// 同样必须，阶段 1 实测 500）；Named 具名音色提前拦截。
    #[test]
    fn test_synthesize_voxcpm2_plain_options_and_named_intercept() {
        let (base_url, received, _handle) = spawn_stub();
        let tts = AudiocppTts::new_with_base_url(voxcpm2_cfg(), &base_url);
        tts.synthesize("你好", 1.0, &TtsVoiceParams::Sid(0))
            .unwrap();
        let body = received.lock().unwrap().last().unwrap().clone();
        assert_eq!(body["options"]["retry_badcase"], false, "整段路径同样必带");
        // Named → 提前拦截（上游仅接受 speaker reference，阶段 1 实测 server 拒绝）
        let err = tts
            .synthesize("x", 1.0, &TtsVoiceParams::Named("demo".into()))
            .unwrap_err();
        assert!(err.contains("仅支持参考音频克隆"), "err: {err}");
    }
}
