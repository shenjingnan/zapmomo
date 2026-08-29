/// 语音会话事件与默认 CLI 输出。
///
/// `VoiceEvent` 是编排器的统一事件输出（替代 `VoiceSession` 内散落的
/// `println!/print!/eprintln!`）。宿主注入一个 `Box<dyn Fn(VoiceEvent) + Send>`：
/// CLI 用 [`cli_sink`]（逐字节复刻原有控制台输出，`zapmomo voice run` 行为不变），
/// Tauri 用 `app.emit` sink（转发为 `voice-session-*` 事件给前端）。
use crate::voice::state::SessionState;
use serde::Serialize;

/// 语音会话事件（`Serialize` 供 Tauri 跨进程转发给前端）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceEvent {
    /// 会话开始（`[会话] 开始...`）
    Started,
    /// 状态迁移（`[会话] 状态 -> {:?}`）
    State { state: SessionState },
    /// 唤醒词命中（`[唤醒] 检测到: {keyword}`）
    Wake { keyword: String },
    /// ASR 转写（部分/最终；最终对应 `[用户]`）
    Transcript { text: String, is_final: bool },
    /// LLM 流式可见文本增量（思考块已被过滤）
    Token { delta: String },
    /// 切句入队合成（`[合成] {sentence}`）
    ReplySentence { sentence: String },
    /// 合成结果开始播放（`[播放] {sentence}`）
    PlaySentence { sentence: String },
    /// 一轮回复生成结束（`[回复完成] {reason}`）；`text` 为该轮完整可见回复
    /// （思考块已过滤，`None` = 空回复），供宿主持久化对话记录。
    ReplyFinished {
        reason: String,
        text: Option<String>,
    },
    /// 回复播完，进入跟听聆听（`[跟听] 继续说...`；前端不消费）
    FollowUp,
    /// 错误（LLM / 合成 / 打断）
    Error { kind: ErrorKind, message: String },
    /// 打断（`source` 区分来源：唤醒词 / 语音 / 快捷键 / 文字输入）
    BargeIn { source: BargeInSource },
    /// 会话停止（结束 / 达最大轮数）
    Stopped { reason: StoppedReason, turns: u32 },
}

/// 打断来源（决定打断后的去向与用户提示文案：唤醒词/快捷键回待唤醒，
/// 语音打断直接进聆听接话，文字输入随后处理文本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BargeInSource {
    /// 唤醒词命中（Thinking/Speaking 期间 KWS 监听）
    WakeWord,
    /// 语音打断（ASR partial 判定用户在说话，回声过滤通过）
    Voice,
    /// 全局快捷键（宿主置位共享标志）
    Hotkey,
    /// 文字输入到达（输入条窗口）
    Text,
}

/// 错误来源。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Llm,
    Synth,
    BargeIn,
}

/// 停止原因。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StoppedReason {
    MaxTurns { max: u32 },
    Manual,
}

/// 把语音会话事件镜像写入 tracing 日志（文件层 `~/.zapmomo/logs/app.log`，info+ 可见）。
///
/// CLI sink 与 Tauri sink 在转发事件时都调用本函数，保证两种模式下语音会话的
/// **状态切换 / ASR 识别 / TTS 合成**内容都能离线回溯，便于排查状态机问题。
/// 分级：关键信息（状态/唤醒/ASR 最终文本/TTS 合成/错误/停止）用 `info`（进文件），
/// 高频中间态（ASR 流式字幕 / LLM token / 播放时序）用 `debug`（不进文件防刷屏）。
pub fn log_voice_event(ev: &VoiceEvent) {
    match ev {
        VoiceEvent::Started => tracing::info!("[voice] 会话开始"),
        VoiceEvent::State { state } => tracing::info!("[voice] 状态切换 -> {state:?}"),
        VoiceEvent::Wake { keyword } => tracing::info!("[voice] 唤醒词命中: {keyword}"),
        VoiceEvent::Transcript { text, is_final } => {
            if *is_final {
                tracing::info!("[voice] ASR 最终识别: {text}");
            } else if !text.is_empty() {
                tracing::debug!("[voice] ASR 流式字幕: {text}");
            }
        }
        VoiceEvent::Token { delta } => {
            if !delta.is_empty() {
                tracing::debug!("[voice] LLM token: {delta}");
            }
        }
        VoiceEvent::ReplySentence { sentence } => {
            tracing::info!("[voice] TTS 合成入队: {sentence}")
        }
        VoiceEvent::PlaySentence { sentence } => {
            tracing::debug!("[voice] TTS 开始播放: {sentence}")
        }
        VoiceEvent::ReplyFinished { reason, text } => {
            tracing::info!("[voice] 回复生成结束: {reason}");
            if let Some(text) = text
                && !text.is_empty()
            {
                tracing::info!("[voice] 回复内容: {text}");
            }
        }
        VoiceEvent::FollowUp => tracing::info!("[voice] 进入跟听聆听"),
        VoiceEvent::Error { kind, message } => {
            tracing::warn!("[voice] 错误 kind={kind:?} message={message}")
        }
        VoiceEvent::BargeIn { source } => match source {
            BargeInSource::WakeWord => tracing::info!("[voice] 唤醒词打断"),
            BargeInSource::Voice => tracing::info!("[voice] 语音打断：检测到用户说话"),
            BargeInSource::Hotkey => tracing::info!("[voice] 快捷键打断"),
            BargeInSource::Text => tracing::info!("[voice] 文字输入打断"),
        },
        VoiceEvent::Stopped { reason, turns } => {
            tracing::info!("[voice] 会话停止 reason={reason:?} turns={turns}")
        }
    }
}

/// 默认 CLI sink：`zapmomo voice run` 的输出格式逐字节复刻原 `println!`。
pub fn cli_sink(ev: VoiceEvent) {
    use std::io::Write;
    log_voice_event(&ev);
    match ev {
        VoiceEvent::Started => println!("[会话] 开始（Ctrl-C 退出）。喊唤醒词开始对话。"),
        VoiceEvent::State { state } => println!("[会话] 状态 -> {state:?}"),
        VoiceEvent::Wake { keyword } => println!("\n[唤醒] 检测到: {keyword}，开始聆听"),
        VoiceEvent::Transcript { text, is_final } => {
            if is_final {
                println!("\n[用户] {text}");
            } else if !text.is_empty() {
                print!("\r[识别] {text}");
                let _ = Write::flush(&mut std::io::stdout());
            }
        }
        VoiceEvent::Token { delta } => {
            print!("{delta}");
            let _ = Write::flush(&mut std::io::stdout());
        }
        VoiceEvent::ReplySentence { sentence } => println!("  [合成] {sentence}"),
        VoiceEvent::PlaySentence { sentence } => println!("  [播放] {sentence}"),
        VoiceEvent::ReplyFinished { reason, .. } => {
            println!(); // 结束 token 流的一行
            println!("[回复完成] {reason}");
        }
        VoiceEvent::FollowUp => println!("\n[跟听] 继续说。"),
        VoiceEvent::Error { kind, message } => match kind {
            ErrorKind::Llm => eprintln!("[LLM 错误] {message}"),
            ErrorKind::Synth => eprintln!("[合成错误] {message}"),
            ErrorKind::BargeIn => eprintln!("[打断] {message}"),
        },
        VoiceEvent::BargeIn { source } => match source {
            BargeInSource::WakeWord => println!("\n[打断] 检测到唤醒词，回到待唤醒"),
            BargeInSource::Voice => println!("\n[打断] 检测到你的声音，请继续说"),
            BargeInSource::Hotkey => println!("\n[打断] 快捷键打断，回到待唤醒"),
            BargeInSource::Text => println!("\n[打断] 收到文字输入"),
        },
        VoiceEvent::Stopped { reason, turns } => match reason {
            StoppedReason::MaxTurns { max } => println!("[会话] 已达最大轮数 {max}，退出"),
            StoppedReason::Manual => println!("[会话] 结束（共 {turns} 轮）"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::make_file_writer;
    use crate::test_util::run_with_temp_home;
    use crate::voice::state::SessionState;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, Registry, fmt};

    #[test]
    fn test_log_voice_event_all_variants_no_panic() {
        // 遍历所有变体调用 log_voice_event（无 subscriber 时 tracing 为 no-op），
        // 确保任何事件都不会在日志镜像层 panic
        let events = vec![
            VoiceEvent::Started,
            VoiceEvent::State {
                state: SessionState::WaitingSpeech,
            },
            VoiceEvent::Wake {
                keyword: "你好小智".to_string(),
            },
            VoiceEvent::Transcript {
                text: "识别中".to_string(),
                is_final: false,
            },
            VoiceEvent::Transcript {
                text: "你好".to_string(),
                is_final: true,
            },
            VoiceEvent::Token {
                delta: "今天".to_string(),
            },
            VoiceEvent::ReplySentence {
                sentence: "今天天气不错。".to_string(),
            },
            VoiceEvent::PlaySentence {
                sentence: "今天天气不错。".to_string(),
            },
            VoiceEvent::ReplyFinished {
                reason: "Eos".to_string(),
                text: None,
            },
            VoiceEvent::FollowUp,
            VoiceEvent::Error {
                kind: ErrorKind::Llm,
                message: "x".to_string(),
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::WakeWord,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Voice,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Hotkey,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Text,
            },
            VoiceEvent::Stopped {
                reason: StoppedReason::Manual,
                turns: 2,
            },
        ];
        for ev in &events {
            log_voice_event(ev);
        }
    }

    #[test]
    fn test_log_voice_event_info_levels_written_to_file() {
        // 验证 info 级别的语音事件（状态切换/ASR 最终/TTS 合成）真的写入日志文件，
        // 而 debug 级别的中间态（流式字幕）被文件层的 info 过滤器丢弃。
        run_with_temp_home(|home| {
            let log_path = home.join(".zapmomo/logs/app.log");
            std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
            let subscriber = Registry::default().with(
                fmt::layer()
                    .with_writer(make_file_writer(log_path.clone()))
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("info")),
            );
            tracing::subscriber::with_default(subscriber, || {
                log_voice_event(&VoiceEvent::State {
                    state: SessionState::Speaking,
                });
                log_voice_event(&VoiceEvent::Transcript {
                    text: "今天天气不错".to_string(),
                    is_final: true,
                });
                log_voice_event(&VoiceEvent::ReplySentence {
                    sentence: "确实不错。".to_string(),
                });
                // debug 级别中间态：不应出现在 info 过滤后的文件里
                log_voice_event(&VoiceEvent::Transcript {
                    text: "中间识别".to_string(),
                    is_final: false,
                });
            });
            let content = std::fs::read_to_string(&log_path).unwrap();
            assert!(
                content.contains("状态切换 -> Speaking"),
                "状态切换应写入文件"
            );
            assert!(
                content.contains("ASR 最终识别: 今天天气不错"),
                "ASR 最终文本应写入文件"
            );
            assert!(
                content.contains("TTS 合成入队: 确实不错。"),
                "TTS 输入应写入文件"
            );
            assert!(!content.contains("中间识别"), "debug 流式字幕不应写入文件");
        });
    }

    #[test]
    fn test_voice_event_serialize_shape() {
        let cases = [
            (
                VoiceEvent::State {
                    state: SessionState::Armed,
                },
                r#"{"type":"state","state":"armed"}"#,
            ),
            (
                VoiceEvent::Transcript {
                    text: "今天天气".to_string(),
                    is_final: false,
                },
                r#"{"type":"transcript","text":"今天天气","is_final":false}"#,
            ),
            (
                VoiceEvent::ReplyFinished {
                    reason: "Eos".to_string(),
                    text: Some("好的。".to_string()),
                },
                r#"{"type":"reply_finished","reason":"Eos","text":"好的。"}"#,
            ),
            (
                VoiceEvent::ReplyFinished {
                    reason: "Eos".to_string(),
                    text: None,
                },
                r#"{"type":"reply_finished","reason":"Eos","text":null}"#,
            ),
            (
                VoiceEvent::Error {
                    kind: ErrorKind::Llm,
                    message: "加载失败".to_string(),
                },
                r#"{"type":"error","kind":"llm","message":"加载失败"}"#,
            ),
            (VoiceEvent::FollowUp, r#"{"type":"follow_up"}"#),
            (
                VoiceEvent::BargeIn {
                    source: BargeInSource::WakeWord,
                },
                r#"{"type":"barge_in","source":"wake_word"}"#,
            ),
            (
                VoiceEvent::BargeIn {
                    source: BargeInSource::Voice,
                },
                r#"{"type":"barge_in","source":"voice"}"#,
            ),
            (
                VoiceEvent::Stopped {
                    reason: StoppedReason::MaxTurns { max: 5 },
                    turns: 5,
                },
                r#"{"type":"stopped","reason":{"reason":"max_turns","max":5},"turns":5}"#,
            ),
        ];
        for (ev, expected) in cases {
            assert_eq!(serde_json::to_string(&ev).unwrap(), expected);
        }
    }

    #[test]
    fn test_cli_sink_all_variants_no_panic() {
        // 遍历所有变体调用 cli_sink，确保不 panic、不抛错
        let events = vec![
            VoiceEvent::Started,
            VoiceEvent::State {
                state: SessionState::Listening,
            },
            VoiceEvent::Wake {
                keyword: "你好小智".to_string(),
            },
            VoiceEvent::Transcript {
                text: "你".to_string(),
                is_final: false,
            },
            VoiceEvent::Transcript {
                text: "你好".to_string(),
                is_final: true,
            },
            VoiceEvent::Token {
                delta: "今天".to_string(),
            },
            VoiceEvent::ReplySentence {
                sentence: "今天天气不错。".to_string(),
            },
            VoiceEvent::PlaySentence {
                sentence: "今天天气不错。".to_string(),
            },
            VoiceEvent::ReplyFinished {
                reason: "Eos".to_string(),
                text: Some("好的。".to_string()),
            },
            VoiceEvent::ReplyFinished {
                reason: "Eos".to_string(),
                text: None,
            },
            VoiceEvent::FollowUp,
            VoiceEvent::Error {
                kind: ErrorKind::Llm,
                message: "x".to_string(),
            },
            VoiceEvent::Error {
                kind: ErrorKind::Synth,
                message: "x".to_string(),
            },
            VoiceEvent::Error {
                kind: ErrorKind::BargeIn,
                message: "x".to_string(),
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::WakeWord,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Voice,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Hotkey,
            },
            VoiceEvent::BargeIn {
                source: BargeInSource::Text,
            },
            VoiceEvent::Stopped {
                reason: StoppedReason::Manual,
                turns: 3,
            },
            VoiceEvent::Stopped {
                reason: StoppedReason::MaxTurns { max: 3 },
                turns: 3,
            },
        ];
        for ev in events {
            cli_sink(ev);
        }
    }
}
