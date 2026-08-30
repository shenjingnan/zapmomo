/// 语音会话状态机。
///
/// 纯函数 `transition` 描述状态迁移；编排器把跨线程信号（KWS 唤醒、ASR 最终文本、
/// 打断、LLM 事件、合成结果）先汇成 [`SessionEvent`] 再调用它，状态迁移集中在
/// one place，便于单测。
///
/// ```text
/// Idle --Start--> Armed --KeywordDetected--> Greeting --WelcomeDone--> WaitingSpeech
///   ▲              │                              │  ▲   (WaitingSpeech 检测真说话)
///   │ Stop          │                              │  │        │
///   │               └──(WaitingSpeech)──SpeechDetected──► Listening
///   │              ┌───────────────────────────────────► Listening
///   │              │ (FollowUp: 回复播完直接进 ASR 聆听，跟听免唤醒)
///   │              │                                     │
///   └──────────────┼───────────────────────────► Thinking ◄─ UserUtteranceFinal
///                   │                              │        │ FirstSentenceEnqueued
///                   └──────────────────────────────┴──► Speaking ──┘
///   Armed <--WaitTimeout-- WaitingSpeech（第一轮无人说话回待唤醒）
///   Armed <--BargeIn------- Thinking|Speaking（打断）
///   Listening <--VoiceBargeIn-- Thinking|Speaking（语音打断，直接接话）
///   Listening 空识别 → 保持聆听不退出（session 内重建 ASR 流，不回 Armed）
/// ```
use serde::Serialize;

/// `Armed`（待唤醒）是 KWS 门控：只有命中唤醒词才进入 `Listening`（ASR），
/// 否则不消费用户话语——避免「不说话也一直在识别」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// 未运行（初始/停止）
    Idle,
    /// 待唤醒：KWS 监听唤醒词
    Armed,
    /// 播欢迎语音（不监听麦克风，防回声）
    Greeting,
    /// 等用户真正说话（RMS 门控）
    WaitingSpeech,
    /// 聆听用户（ASR 识别）
    Listening,
    /// 模型思考（LLM 生成中）
    Thinking,
    /// 播报（TTS 句级播放中，可能仍在生成后续句）
    Speaking,
}

/// 触发状态迁移的会话事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    /// 会话开始（Idle → Armed）
    Start,
    /// 命中唤醒词（Armed → Greeting）
    KeywordDetected,
    /// 欢迎语播完（Greeting → WaitingSpeech）
    WelcomeDone,
    /// RMS 检测到真说话（WaitingSpeech → Listening）
    SpeechDetected,
    /// 等待超时无结果（WaitingSpeech|Listening → Armed）
    WaitTimeout,
    /// 一句话说完（Listening → Thinking；文字输入也可从 Armed|WaitingSpeech 触发）
    UserUtteranceFinal,
    /// 首个句子已入队合成（Thinking → Speaking）
    FirstSentenceEnqueued,
    /// 回复播完 / 无内容可播（Thinking|Speaking → Armed）
    ReplyFinished,
    /// 回复播完，进跟听聆听（Thinking|Speaking → Listening，区别于回待唤醒）
    FollowUp,
    /// 打断（Thinking|Speaking → Armed）
    BargeIn,
    /// 语音打断（Thinking|Speaking → Listening，直接接话；区别于 BargeIn 回待唤醒）
    VoiceBargeIn,
    /// 停止（任意 → Idle）
    Stop,
}

/// 状态迁移函数。非法迁移返回 Err（含两个状态，便于定位编排逻辑错误）。
pub fn transition(state: SessionState, ev: SessionEvent) -> Result<SessionState, String> {
    use SessionEvent::*;
    use SessionState::*;
    let next = match (state, ev) {
        (Idle, Start) => Armed,
        // 唤醒 → 先播欢迎语
        (Armed, KeywordDetected) => Greeting,
        (Greeting, WelcomeDone) => WaitingSpeech,
        // 真说话门控通过 → ASR
        (WaitingSpeech, SpeechDetected) => Listening,
        // 等待超时（无人说话 / 无有效文本）→ 回待唤醒
        (WaitingSpeech | Listening, WaitTimeout) => Armed,
        // 文字输入（输入条窗口）绕过唤醒门控：待唤醒/等说话/说话完毕都直接进思考
        (Listening, UserUtteranceFinal) => Thinking,
        (Armed | WaitingSpeech, UserUtteranceFinal) => Thinking,
        (Thinking, FirstSentenceEnqueued) => Speaking,
        // 播完 / 思考阶段未切出任何句子（空回复）→ 回到待唤醒
        (Speaking, ReplyFinished) => Armed,
        (Thinking, ReplyFinished) => Armed,
        // 播完 → 直接进 ASR 聆听（跟听，免唤醒；空识别时 session 保持 Listening 不退出）
        (Thinking | Speaking, FollowUp) => Listening,
        // 打断 → 回到待唤醒（Greeting：文字输入到达时停掉欢迎语）
        (Thinking | Speaking, BargeIn) => Armed,
        (Greeting, BargeIn) => Armed,
        // 语音打断（ASR 识别到用户说话）→ 直接进聆听接话（不回待唤醒）
        (Thinking | Speaking, VoiceBargeIn) => Listening,
        // Stop 从任意状态（含 Idle）回到 Idle
        (_, Stop) => Idle,
        (s, ev) => {
            return Err(format!("非法状态迁移: {s:?} --{ev:?}--> ?"));
        }
    };
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_happy_path_transitions() {
        use SessionEvent::*;
        use SessionState::*;
        assert_eq!(transition(Idle, Start).unwrap(), Armed);
        // 唤醒 → 欢迎语 → 等真说话 → ASR
        assert_eq!(transition(Armed, KeywordDetected).unwrap(), Greeting);
        assert_eq!(transition(Greeting, WelcomeDone).unwrap(), WaitingSpeech);
        assert_eq!(
            transition(WaitingSpeech, SpeechDetected).unwrap(),
            Listening
        );
        assert_eq!(transition(Listening, UserUtteranceFinal).unwrap(), Thinking);
        assert_eq!(
            transition(Thinking, FirstSentenceEnqueued).unwrap(),
            Speaking
        );
        assert_eq!(transition(Speaking, ReplyFinished).unwrap(), Armed);
        // 思考中未切出句子（空回复）→ 回到待唤醒
        assert_eq!(transition(Thinking, ReplyFinished).unwrap(), Armed);
        // 播完 → 直接进 ASR 聆听（跟听，免唤醒）
        assert_eq!(transition(Speaking, FollowUp).unwrap(), Listening);
        assert_eq!(transition(Thinking, FollowUp).unwrap(), Listening);
        // 思考中/播报中打断 → 回到待唤醒
        assert_eq!(transition(Thinking, BargeIn).unwrap(), Armed);
        assert_eq!(transition(Speaking, BargeIn).unwrap(), Armed);
    }

    #[test]
    fn test_text_input_transitions() {
        use SessionEvent::*;
        use SessionState::*;
        // 文字输入绕过唤醒门控：待唤醒/等说话直接进思考
        assert_eq!(transition(Armed, UserUtteranceFinal).unwrap(), Thinking);
        assert_eq!(
            transition(WaitingSpeech, UserUtteranceFinal).unwrap(),
            Thinking
        );
        // 欢迎语播放中收到文字 → 允许打断停掉欢迎语
        assert_eq!(transition(Greeting, BargeIn).unwrap(), Armed);
    }

    #[test]
    fn test_voice_barge_in_goes_listening() {
        use SessionEvent::*;
        use SessionState::*;
        // 语音打断：思考中/播报中 → 直接进聆听（不回待唤醒）
        assert_eq!(transition(Thinking, VoiceBargeIn).unwrap(), Listening);
        assert_eq!(transition(Speaking, VoiceBargeIn).unwrap(), Listening);
        // roundtrip：打断 → 聆听 → 说完 → 新一轮思考（接话语义闭环）
        let s = transition(Speaking, VoiceBargeIn).unwrap();
        assert_eq!(transition(s, UserUtteranceFinal).unwrap(), Thinking);
    }

    #[test]
    fn test_follow_up_roundtrip() {
        use SessionEvent::*;
        use SessionState::*;
        // 回复播完 → 直接进 ASR 聆听（跟听，免唤醒）→ 识别 → 第二轮
        let mut s = transition(Speaking, FollowUp).unwrap();
        assert_eq!(s, Listening);
        s = transition(s, UserUtteranceFinal).unwrap();
        assert_eq!(s, Thinking);
        s = transition(s, FirstSentenceEnqueued).unwrap();
        assert_eq!(s, Speaking);
        // 第二轮播完 → 再次进聆听（持续对话循环）
        assert_eq!(transition(Speaking, FollowUp).unwrap(), Listening);
        // 第一轮欢迎语后仍走 WaitingSpeech 门控；无人说话 → 回待唤醒
        assert_eq!(
            transition(WaitingSpeech, SpeechDetected).unwrap(),
            Listening
        );
        assert_eq!(transition(WaitingSpeech, WaitTimeout).unwrap(), Armed);
    }

    #[test]
    fn test_wait_timeout_goes_armed() {
        use SessionEvent::*;
        use SessionState::*;
        // 等说话超时 / ASR 无文本超时 → 回待唤醒
        assert_eq!(transition(WaitingSpeech, WaitTimeout).unwrap(), Armed);
        assert_eq!(transition(Listening, WaitTimeout).unwrap(), Armed);
    }

    #[test]
    fn test_stop_from_any_state_goes_idle() {
        use SessionEvent::*;
        use SessionState::*;
        for s in [
            Idle,
            Armed,
            Greeting,
            WaitingSpeech,
            Listening,
            Thinking,
            Speaking,
        ] {
            assert_eq!(transition(s, Stop).unwrap(), Idle);
        }
    }

    #[test]
    fn test_invalid_transitions_error() {
        use SessionEvent::*;
        use SessionState::*;
        let invalid: &[(SessionState, SessionEvent)] = &[
            (Idle, KeywordDetected),
            (Idle, WelcomeDone),
            (Idle, SpeechDetected),
            (Idle, WaitTimeout),
            (Idle, UserUtteranceFinal),
            (Idle, FirstSentenceEnqueued),
            (Idle, ReplyFinished),
            (Idle, BargeIn),
            (Idle, VoiceBargeIn),
            (Idle, FollowUp),
            (Armed, Start),
            (Armed, WelcomeDone),
            (Armed, SpeechDetected),
            (Armed, WaitTimeout),
            (Armed, FirstSentenceEnqueued),
            (Armed, ReplyFinished),
            (Armed, BargeIn),
            (Armed, VoiceBargeIn),
            (Armed, FollowUp),
            (Greeting, KeywordDetected),
            (Greeting, SpeechDetected),
            (Greeting, WaitTimeout),
            (Greeting, UserUtteranceFinal),
            (Greeting, FirstSentenceEnqueued),
            (Greeting, ReplyFinished),
            (Greeting, FollowUp),
            (Greeting, VoiceBargeIn),
            (WaitingSpeech, KeywordDetected),
            (WaitingSpeech, WelcomeDone),
            (WaitingSpeech, FirstSentenceEnqueued),
            (WaitingSpeech, ReplyFinished),
            (WaitingSpeech, BargeIn),
            (WaitingSpeech, FollowUp),
            (WaitingSpeech, VoiceBargeIn),
            (Listening, Start),
            (Listening, KeywordDetected),
            (Listening, WelcomeDone),
            (Listening, SpeechDetected),
            (Listening, FirstSentenceEnqueued),
            (Listening, ReplyFinished),
            (Listening, BargeIn),
            (Listening, FollowUp),
            (Listening, VoiceBargeIn),
            (Thinking, Start),
            (Thinking, KeywordDetected),
            (Thinking, WelcomeDone),
            (Thinking, SpeechDetected),
            (Thinking, WaitTimeout),
            (Thinking, UserUtteranceFinal),
            (Speaking, Start),
            (Speaking, KeywordDetected),
            (Speaking, WelcomeDone),
            (Speaking, SpeechDetected),
            (Speaking, WaitTimeout),
            (Speaking, UserUtteranceFinal),
            (Speaking, FirstSentenceEnqueued),
        ];
        for (s, ev) in invalid {
            let err = transition(*s, *ev).unwrap_err();
            assert!(err.contains("非法状态迁移"), "err: {err}");
        }
    }

    #[test]
    fn test_transition_roundtrip() {
        use SessionEvent::*;
        use SessionState::*;
        // 一次完整对话轮次的状态序列（含欢迎语 + 门控）
        let mut s = transition(Idle, Start).unwrap();
        assert_eq!(s, Armed);
        s = transition(s, KeywordDetected).unwrap();
        assert_eq!(s, Greeting);
        s = transition(s, WelcomeDone).unwrap();
        assert_eq!(s, WaitingSpeech);
        s = transition(s, SpeechDetected).unwrap();
        assert_eq!(s, Listening);
        s = transition(s, UserUtteranceFinal).unwrap();
        assert_eq!(s, Thinking);
        s = transition(s, FirstSentenceEnqueued).unwrap();
        assert_eq!(s, Speaking);
        s = transition(s, ReplyFinished).unwrap();
        assert_eq!(s, Armed);
    }
}
