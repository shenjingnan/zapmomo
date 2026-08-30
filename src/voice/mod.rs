/// 语音会话（KWS→ASR→LLM→TTS 全链路编排）。
///
/// 把四个能力模块串成一条对话链路：唤醒词 → 识别 → 思考 → 句级流式播报 → 回听，
/// 播报/思考期间保持唤醒词监听以支持打断（barge-in）。
///
/// sherpa-onnx 的 TTS 只有整句一次性合成（无流式 TTS API），因此「流式输出」由
/// 句级流水线近似：LLM 流式 token → `splitter` 切句 → 独立合成线程逐句合成 →
/// rodio `Sink` 边合成边播放。
pub mod asr_backend;
pub mod bargein;
pub mod config;
pub mod events;
pub mod listen;
pub mod player;
pub mod records;
pub mod sanitizer;
pub mod session;
pub mod splitter;
pub mod state;
pub mod synthesizer;
pub(crate) mod thinking;

pub use config::{CliOverrides, ResolvedSessionConfig};
pub use events::{ErrorKind, VoiceEvent, cli_sink};
pub use listen::{ChunkAccumulator, MicEvent, MicLoop};
pub use player::{AudioPlayer, MockPlayer, Speaker};
pub use records::{ConversationRecord, RecordRole};
pub use sanitizer::{TtsSanitizer, sanitize_for_tts};
pub use session::{ReplyAccumulator, TtsSwap, TtsSwapSlot, VoiceSession};
pub use synthesizer::{SynthHandle, SynthResult};

/// CLI 入口：解析配置 → 应用角色包覆盖 → 构造会话 → 运行（Ctrl-C 优雅退出）。
pub async fn run_cli(overrides: CliOverrides) -> Result<(), String> {
    let settings = crate::config::settings::load_settings()?;
    let mut cfg = config::resolve(settings.as_ref(), &overrides)?;
    config::apply_companion_overrides(&mut cfg);
    let mut session = VoiceSession::new(cfg)?;
    let running = session.running.clone();
    ctrlc::set_handler(move || {
        running.store(false, std::sync::atomic::Ordering::Relaxed);
    })
    .map_err(|e| format!("无法注册 Ctrl-C 处理器: {e}"))?;
    session.run()
}
