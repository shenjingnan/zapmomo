use std::cell::Cell;
use std::time::Duration;

use super::AudiocppError;
use super::families::{AudiocppFamilyDesc, VoiceSemantics, family_desc};
use crate::tts::TtsVoiceParams;
use crate::tts::config::ResolvedTtsConfig;

/// 连接超时（server 未起/已退出时快速失败）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 合成请求超时（长文本合成可达数秒，留足余量）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// 错误响应体截断上限（避免超长 body 刷屏）。
const ERROR_BODY_MAX: usize = 500;

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
    /// 音色字段按族语义映射（`families::VoiceSemantics`）：
    /// - 固定具名音色（pocket）：`Named`/`Sid` → `voice`；`Reference` 报错；
    /// - 参考音频克隆（omnivoice）：`Reference` → `voice_ref`+`reference_text`
    ///   （本地路径，sidecar 同机可读）；`Named` → 透传 `voice`（server 端
    ///   preset/voice_dir 二期）；`Sid` → 省略 voice 字段（server auto voice）。
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
        match (self.desc.voice_semantics, voice) {
            (VoiceSemantics::FixedNamed(_), TtsVoiceParams::Named(v)) => {
                body["voice"] = serde_json::json!(v);
            }
            (VoiceSemantics::FixedNamed(default), TtsVoiceParams::Sid(_)) => {
                body["voice"] = serde_json::json!(default);
            }
            (VoiceSemantics::FixedNamed(default), TtsVoiceParams::Reference { .. }) => {
                return Err(AudiocppError::UnsupportedVoice(format!(
                    "该 audio.cpp 模型为固定音色（{default}），不支持参考音频克隆"
                ))
                .to_user_message());
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
            (VoiceSemantics::ReferenceClone, TtsVoiceParams::Named(v)) => {
                body["voice"] = serde_json::json!(v);
            }
            // 省略 voice 字段 → server auto voice（无显式音色时的可用兜底）
            (VoiceSemantics::ReferenceClone, TtsVoiceParams::Sid(_)) => {}
        }
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
    }
}
