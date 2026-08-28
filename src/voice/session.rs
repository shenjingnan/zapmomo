/// 语音会话编排核心（`VoiceSession`）。
///
/// 把 KWS → ASR → LLM → TTS 串成一条**唤醒门控 + 句级流式 + 唤醒词打断 + 跟听续聊**的对话链路：
///
/// ```text
/// Idle --Start--> Armed --唤醒词--> Greeting --欢迎播完--> WaitingSpeech --真说话--> Listening
///   ▲                            （下次唤醒）  │            │ 超时→Armed         │
///   │                                           └─────────────────────┐         │
///   │           Armed ◄──BargeIn─── Thinking|Speaking ◄──FirstSentenceEnqueued──┘
///   │              ▲            │（回复播完）                          │
///   └──────────────┘◄─WaitTimeout│  FollowUp → 直接进 Listening（跟听免唤醒）  │
///                              └──────────────────────────────────────────────┘
/// ```
///
/// `Armed`（待唤醒）是 KWS 门控：命中唤醒词才进入 `Listening`（ASR 识别），
/// 否则不消费用户话语。第一轮欢迎语后仍走 `WaitingSpeech` RMS 门控（无人说话超时回
/// 待唤醒）。**回复播完默认直接进 `Listening` 跟听**（`follow_up` 开启，免唤醒），
/// 识别到内容即进入下一轮；ASR 空识别时**保持聆听不退出**（重建流继续听），直到
/// 达 `max_turns` 或手动停止——形成无需再喊唤醒词的持续对话循环。
///
/// 线程模型：编排循环在**调用线程**运行，持有全部 sherpa 引擎/流与 rodio 播放器
/// （`Sink`/`Player` 不跨线程）。唯一后台线程是 [`SynthHandle`] 的 TTS 合成线程。
/// 整个会话只开一次麦克风（[`MicLoop`]），按状态把 chunk 喂给 KWS 流（待唤醒/
/// 播报/思考）或 ASR 流（聆听）——KWS/ASR 各自 `start_capture` 会在同设备冲突。
///
/// 打断序列：KWS 命中 → `llm.cancel()` + `speaker.stop()` + `synth.cancel_all()` +
/// `current_gen += 1`（作废在途合成结果）+ 回 Armed 前 `skip_for` 丢回声尾巴。
use crate::asr::{AsrReaction, AsrResult};
use crate::kws::{KwsEngine, KwsResult, Reaction, ReactionOutcome};
use crate::llm::LlmEngine;
use crate::llm::LlmEvent;
use crate::llm::types::{ChatMessage, ChatRole, InputItem};
use crate::tts::TtsEngine;
use crate::voice::asr_backend::AsrBackend;
use crate::voice::config::ResolvedSessionConfig;
use crate::voice::events::{ErrorKind, StoppedReason, VoiceEvent};
use crate::voice::listen::{MicEvent, MicLoop};
use crate::voice::player::AudioPlayer;
use crate::voice::splitter::SentenceSplitter;
use crate::voice::state::{SessionEvent, SessionState};
use crate::voice::synthesizer::{SynthHandle, SynthResult};
use sherpa_onnx::OnlineStream;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// 编排循环单次轮询麦克风的最长等待（块间隔远小于此，不影响实时性）。
const MIC_POLL: Duration = Duration::from_millis(100);
/// 打断后回听前跳过的音频时长（丢弃回声尾巴，避免把上一条回答喂给 ASR）。
const SKIP_AFTER_BARGE_IN: Duration = Duration::from_millis(300);
/// 回复播完进入跟听窗口前跳过的音频时长（丢弃 TTS 回声尾巴，避免误判为用户说话）。
const SKIP_AFTER_REPLY: Duration = Duration::from_millis(300);
/// LLM 模型加载超时（首次加载大模型较慢）。
const LLM_LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// 待换 TTS 引擎包裹：写方（`set_current_model` 的 TTS 事务臂）在 `spawn_blocking`
/// 里构造新引擎后塞入 [`TtsSwapSlot`] 邮箱，会话编排循环每轮开头取走并**句间**
/// 换入合成线程（`SynthHandle::swap_engine`，当前句不打断）。
///
/// 与 LLM 共享槽（`Arc<LlmEngine>` + ptr_eq 比较）的关键差异：`TtsEngine` 仅保证
/// `Send`（sherpa `OfflineTts` 无 `Sync` 保证），不能跨线程共享引用，只能**转移
/// 所有权**（邮箱 take 语义）；且引擎必须替换进合成线程（按值拥有），而非编排
/// 线程重绑定。`cfg` 随行，供会话侧用**会话语境**（voice_id）解析新音色。
pub struct TtsSwap {
    pub engine: TtsEngine,
    pub cfg: crate::tts::config::ResolvedTtsConfig,
    /// 写方代际（连续切换防旧覆盖新 + 日志追溯）。
    pub generation: u64,
}

/// TTS 热切换邮箱（宿主 `VoiceSessionState` 与会话各持一份 Arc）。
pub type TtsSwapSlot = Arc<Mutex<Option<TtsSwap>>>;

/// 语音会话编排器。
pub struct VoiceSession {
    cfg: ResolvedSessionConfig,
    kws: KwsEngine,
    /// ASR 后端（按 `model_type` 分派流式 zipformer / 离线 SenseVoice/Whisper）
    asr: AsrBackend,
    llm: Arc<LlmEngine>,
    /// 会话订阅的 LLM 事件流（与 GUI 的 forward 订阅互不抢事件）
    llm_rx: Receiver<LlmEvent>,
    /// 文字输入收件箱（Tauri 宿主注入，输入条窗口的消息经此进入会话；CLI 为 `None`）。
    /// 文字与 ASR 最终文本等价，走同一个 [`Self::handle_user_final`] 入口。
    text_rx: Option<Receiver<String>>,
    /// 待处理文字队列：收到时若引擎仍在生成（打断后 cancel 未落地）或状态不允许
    /// 直接处理，先排队，每轮循环重试
    pending_texts: VecDeque<String>,
    /// 宿主（Tauri LlmState）共享引擎槽：运行时引擎可能被外部切换（set_current_model /
    /// load），会话在每次编排循环开头检查槽内引擎是否变化并重新绑定，从而感知新模型。
    /// `None` = CLI 自建引擎，不感知外部切换。
    llm_slot: Option<Arc<Mutex<Option<Arc<LlmEngine>>>>>,
    /// TTS 热切换邮箱（见 [`TtsSwap`]）：宿主（Tauri VoiceSessionState）持有的待换
    /// 引擎，编排循环每轮开头取走并句间换入。`None` = CLI，不感知外部切换。
    tts_swap_slot: Option<TtsSwapSlot>,
    speaker: Box<dyn AudioPlayer>,
    synth: SynthHandle,
    mic: MicLoop,
    kws_stream: OnlineStream,

    state: SessionState,
    /// true = 运行中（CLI 用 Ctrl-C / Tauri 用 stop 命令置 false 优雅退出）
    pub running: Arc<AtomicBool>,
    /// 打断标志（KWS reaction 在 Thinking/Speaking 期间置位，编排循环每轮检查）
    barge_in: Arc<AtomicBool>,

    history: Vec<InputItem>,
    reply: ReplyAccumulator,
    reply_done: bool,
    current_gen: u64,
    synth_enqueued: u64,
    synth_consumed: u64,
    turns: u32,
    first_sentence: bool,
    /// 与合成入队顺序对应的句子文本（播放时弹出打印 `[播放]`，打断时清空）
    pending_speech: VecDeque<String>,
    /// 事件输出（CLI 用 stdout sink / Tauri 用 app.emit sink）
    emit: Box<dyn Fn(VoiceEvent) + Send>,
    /// Listening 阶段连续静音累计（秒），达到 `asr_max_trailing_silence` 判定说完
    silence_accum: f32,
    /// WaitingSpeech 阶段连续说话块计数（防瞬时噪音误触发）
    speech_hits: u32,
    /// WaitingSpeech 阶段等待时长累计（秒），超时回待唤醒
    speech_wait_accum: f32,
    /// 欢迎语是否已播放（合成失败也置位，跳过欢迎不卡流程）
    welcome_played: bool,
    /// 流式句「首块已消费」跟踪（见 [`SentencePlayGate`]）：首块弹
    /// `pending_speech` 并发 `PlaySentence`，后续块沿用，零块句由终态补弹。
    /// 非流式句不触碰（行为等价于 gate 恒 false）。
    stream_gate: SentencePlayGate,
    /// 首响打点（验收量化用；`start_reply` 重置、`do_barge_in` 清空）：
    /// 唤醒 → 首句入队 → 首块播放，首次播放时打一条分段耗时日志
    /// （`mark_first_audio`；非流式路径同样打点供 A/B 对比）。
    t_wake: Option<Instant>,
    t_reply_start: Option<Instant>,
    t_first_sentence: Option<Instant>,
    t_first_audio: Option<Instant>,
}

impl VoiceSession {
    /// 构造会话（CLI 默认：stdout sink + 自建 running 标志 + 自建 LLM 引擎）。
    pub fn new(cfg: ResolvedSessionConfig) -> Result<Self, String> {
        Self::new_with_parts(
            cfg,
            Box::new(crate::voice::events::cli_sink),
            Arc::new(AtomicBool::new(true)),
            None,
            None,
            None,
        )
    }

    /// 构造会话（供 Tauri 等宿主注入事件 sink 与外部停止标志）。
    ///
    /// 校验并创建各引擎 + 打开麦克风与音频输出，任一失败返回带安装提示的错误。
    ///
    /// - `llm_slot`：`Some` 复用宿主（Tauri `LlmState`）的共享引擎槽（**只加载一份模型**），
    ///   运行时引擎被外部切换时会动态感知并重新绑定；`None` 自建（CLI `voice run`）。
    /// - `tts_swap_slot`：`Some` 接宿主（Tauri `VoiceSessionState`）的 TTS 热切换邮箱，
    ///   运行中切 TTS 模型时句间换引擎（见 [`TtsSwap`]）；`None` 不感知（CLI）。
    /// - `text_rx`：`Some` 接宿主的文字输入收件箱（输入条窗口），文字与 ASR 最终文本
    ///   等价进入对话链路；`None` 仅语音（CLI）。
    /// - **说完判定由 session 内 RMS 静音统一控制**，因此这里强制禁用 sherpa endpoint
    ///   （避免其 rule1/rule2 的 1.2s/2.4s 尾静音在期望的 3s 前提前断句并 reset 流）。
    pub fn new_with_parts(
        mut cfg: ResolvedSessionConfig,
        emit: Box<dyn Fn(VoiceEvent) + Send>,
        running: Arc<AtomicBool>,
        llm_slot: Option<Arc<Mutex<Option<Arc<LlmEngine>>>>>,
        tts_swap_slot: Option<TtsSwapSlot>,
        text_rx: Option<Receiver<String>>,
    ) -> Result<Self, String> {
        cfg.asr.enable_endpoint = false;
        let kws = KwsEngine::new(cfg.kws.clone())?;
        // 按 model_type 构造 ASR 后端：zipformer 走流式、SenseVoice/Whisper 走离线
        let asr = AsrBackend::new(&cfg.asr)?;
        let llm = if let Some(slot) = &llm_slot {
            // 宿主共享引擎：槽内应有引擎（start_voice_session_impl 已确保）；为空则自建兜底
            match slot
                .lock()
                .map_err(|_| "llm lock poisoned".to_string())?
                .clone()
            {
                Some(e) => e,
                None => Arc::new(LlmEngine::new(cfg.llm.clone()).map_err(|e| e.to_string())?),
            }
        } else {
            Arc::new(LlmEngine::new(cfg.llm.clone()).map_err(|e| e.to_string())?)
        };
        let llm_rx = llm.subscribe();
        let tts = TtsEngine::new(cfg.tts.clone())?;
        // 合成音色参数统一解析（角色包克隆音色 > zipvoice/omnivoice 克隆 > 缺省兜底）
        let voice = crate::tts::voice::resolve_voice_params(
            &cfg.tts,
            cfg.voice_id.as_deref(),
            None,
            cfg.character_voice.as_ref().map(|v| v.wav.as_path()),
            cfg.character_voice.as_ref().map(|v| v.text.as_str()),
        )?;
        let synth = SynthHandle::new(tts, voice, cfg.speed);
        let mic = MicLoop::new(
            cfg.mic_device.as_deref(),
            cfg.asr.sample_rate,
            cfg.asr.chunk_size,
        )?;
        let speaker = Box::new(crate::voice::player::Speaker::try_new()?);
        let kws_stream = Self::make_kws_stream(&kws, &cfg)?;

        Ok(Self {
            cfg,
            kws,
            asr,
            llm,
            llm_rx,
            text_rx,
            pending_texts: VecDeque::new(),
            llm_slot,
            tts_swap_slot,
            speaker,
            synth,
            mic,
            kws_stream,
            state: SessionState::Idle,
            running,
            barge_in: Arc::new(AtomicBool::new(false)),
            history: Vec::new(),
            reply: ReplyAccumulator::new(),
            reply_done: false,
            current_gen: 0,
            synth_enqueued: 0,
            synth_consumed: 0,
            turns: 0,
            first_sentence: false,
            pending_speech: VecDeque::new(),
            emit,
            silence_accum: 0.0,
            speech_hits: 0,
            speech_wait_accum: 0.0,
            welcome_played: false,
            stream_gate: SentencePlayGate::default(),
            t_wake: None,
            t_reply_start: None,
            t_first_sentence: None,
            t_first_audio: None,
        })
    }

    /// 构造 KWS 流（自定义唤醒词需先编码为 token）。
    fn make_kws_stream(
        kws: &KwsEngine,
        cfg: &ResolvedSessionConfig,
    ) -> Result<OnlineStream, String> {
        match cfg.keywords.as_deref() {
            Some(k) => {
                let encoded = crate::kws::token::encode_custom_keywords(k, &cfg.kws.tokens)?;
                Ok(kws.create_stream_with_keywords(&encoded))
            }
            None => Ok(kws.create_stream()),
        }
    }

    /// 外部打断标志的克隆：宿主（Tauri 全局快捷键）持有并置位后，
    /// 会话编排循环在 Thinking/Speaking 阶段执行 `do_barge_in`（停生成/合成/播放，回 Armed）。
    pub fn barge_in_flag(&self) -> Arc<AtomicBool> {
        self.barge_in.clone()
    }

    /// 运行会话主循环（阻塞直到停止）。
    pub fn run(&mut self) -> Result<(), String> {
        // 共享引擎已加载（Tauri LlmState）则跳过；CLI 自建引擎未加载则阻塞加载
        if !self.llm.is_ready() {
            self.llm.load_blocking(LLM_LOAD_TIMEOUT)?;
        }
        (self.emit)(VoiceEvent::Started);
        self.set_state(SessionEvent::Start)?;

        loop {
            if !self.running.load(Ordering::Relaxed) {
                break;
            }
            // 共享引擎可能被外部切换（set_current_model / load）：每轮开头检查并重新绑定，
            // 使 voice 在空闲（待唤醒）时也能跟随用户切换的模型
            self.refresh_llm_if_switched()?;
            // TTS 热切换（set_current_model TTS 事务臂塞邮箱）：句间换入合成线程
            self.refresh_tts_if_switched();
            // 打断优先于状态推进
            if self.barge_in.load(Ordering::Relaxed)
                && matches!(self.state, SessionState::Thinking | SessionState::Speaking)
            {
                self.do_barge_in();
                continue;
            }
            // 文字输入（输入条窗口）：每轮开头轮询收件箱，与 LLM/TTS 热切换同模式
            self.poll_text_input()?;
            match self.state {
                SessionState::Idle => break,
                SessionState::Armed => self.step_armed()?,
                SessionState::Greeting => self.step_greeting()?,
                SessionState::WaitingSpeech => self.step_waiting_speech()?,
                SessionState::Listening => self.step_listening()?,
                SessionState::Thinking => self.step_thinking()?,
                SessionState::Speaking => self.step_speaking()?,
            }
        }
        (self.emit)(VoiceEvent::Stopped {
            reason: StoppedReason::Manual,
            turns: self.turns,
        });
        Ok(())
    }

    /// 检查共享引擎槽内引擎是否已被外部切换（`set_current_model` / `load`），是则重新
    /// 绑定当前引擎与事件订阅。空闲（待唤醒）时切换是安全的；生成中由宿主侧保护阻止切换。
    fn refresh_llm_if_switched(&mut self) -> Result<(), String> {
        let Some(slot) = &self.llm_slot else {
            return Ok(()); // CLI 自建引擎：无外部切换
        };
        let current = slot
            .lock()
            .map_err(|_| "llm lock poisoned".to_string())?
            .clone();
        let Some(new_llm) = current else {
            return Ok(()); // 槽被清空（unload）：维持当前引擎，下次生成仍可用
        };
        if Arc::ptr_eq(&new_llm, &self.llm) {
            return Ok(()); // 引擎未变
        }
        self.llm = new_llm;
        self.llm_rx = self.llm.subscribe();
        if !self.llm.is_ready() {
            self.llm.load_blocking(LLM_LOAD_TIMEOUT)?;
        }
        tracing::info!("语音会话已切换到新 LLM 引擎");
        Ok(())
    }

    /// TTS 热切换感知：取走邮箱中的新引擎（[`TtsSwap`]），用**会话语境**（voice_id）
    /// 解析新音色后句间换入合成线程。当前在途句子用旧引擎完成（零中断），后续句
    /// 全部用新引擎。音色解析失败时按族兜底（见 [`hot_swap_voice_fallback`]）：
    /// 强制克隆族（qwen3_tts Base）无 auto voice，不换入新引擎（保留旧引擎）；
    /// 其余族兜底 `Sid(0)` 并告警（Sid 对 omnivoice 等克隆族是 auto voice、对
    /// 固定音色族是默认音色，均为可用语义）。语速沿用会话构造值（`set_tts_params`
    /// 的语速调整属于下一会话；避免句间变速跳变）。
    fn refresh_tts_if_switched(&mut self) {
        let Some(slot) = &self.tts_swap_slot else {
            return;
        };
        let swap = slot.lock().ok().and_then(|mut guard| guard.take());
        let Some(swap) = swap else {
            return;
        };
        let voice = match crate::tts::voice::resolve_voice_params(
            &swap.cfg,
            self.cfg.voice_id.as_deref(),
            None,
            self.cfg.character_voice.as_ref().map(|v| v.wav.as_path()),
            self.cfg.character_voice.as_ref().map(|v| v.text.as_str()),
        ) {
            Ok(v) => v,
            Err(e) => match hot_swap_voice_fallback(swap.cfg.model_type) {
                Some(fallback) => {
                    tracing::warn!("TTS 热切换音色解析失败（兜底 sid 0）：{e}");
                    fallback
                }
                None => {
                    // 换入一个每句必报错的新引擎比保留旧引擎更糟：取消本次切换
                    tracing::warn!("TTS 热切换取消：{e}（保留当前引擎，请先在音色库选择克隆音色）");
                    return;
                }
            },
        };
        // 会话 cfg 快照同步（供后续读取一致；speed 由 SynthHandle 线程持有不变）
        self.cfg.tts = swap.cfg;
        tracing::info!("语音会话 TTS 热切换生效（gen {}）", swap.generation);
        self.synth.swap_engine(swap.engine, voice);
    }

    /// 状态迁移 + 进入特定状态时复位对应累计器（ASR 流重建 / 等待计时清零）。
    ///
    /// - 进 `Listening`：重建 ASR 流（丢弃上轮累积的识别状态）+ 清零静音累计。
    /// - 进 `WaitingSpeech`：清零等待计时与说话命中计数，保证**每轮跟听/欢迎都从完整
    ///   超时窗口起算**（否则多轮跟听时 `speech_wait_accum` 残留会逐轮缩短窗口）。
    fn set_state(&mut self, ev: SessionEvent) -> Result<(), String> {
        let next = crate::voice::state::transition(self.state, ev)?;
        self.state = next;
        match next {
            SessionState::Listening => {
                // 复位后端：流式重建流（丢上轮识别状态）；离线清空 PCM 缓冲
                self.asr.reset(&self.cfg.asr);
                self.silence_accum = 0.0;
            }
            SessionState::WaitingSpeech => {
                self.speech_wait_accum = 0.0;
                self.speech_hits = 0;
            }
            _ => {}
        }
        (self.emit)(VoiceEvent::State { state: next });
        Ok(())
    }

    /// Armed：待唤醒，喂 KWS 检测唤醒词；命中 → 合成欢迎语并切到 Greeting（播欢迎语）。
    fn step_armed(&mut self) -> Result<(), String> {
        let chunk = match self.mic.next(MIC_POLL)? {
            MicEvent::Chunk(c) => c,
            MicEvent::Timeout => return Ok(()),
            MicEvent::Disconnected => return Err("麦克风已断开".to_string()),
        };
        self.kws.feed(&self.kws_stream, &chunk);
        let mut reaction = WakeReaction::default();
        let _ = self.kws.detect(&self.kws_stream, &mut reaction);
        if let Some(keyword) = reaction.keyword {
            (self.emit)(VoiceEvent::Wake { keyword });
            // 首响打点：唤醒时刻（含欢迎语流程；回复轮的起点在 start_reply）
            self.t_wake = Some(Instant::now());
            // 唤醒 → 合成并播放欢迎语（复用 SynthHandle；进入 Greeting 等结果）
            self.synth.clear_cancel();
            self.synth
                .enqueue(self.cfg.welcome_text.clone(), self.current_gen);
            self.welcome_played = false;
            self.set_state(SessionEvent::KeywordDetected)?; // → Greeting
        }
        Ok(())
    }

    /// Greeting：播放欢迎语音（复用 SynthHandle 合成，期间不喂麦克风防回声）。
    fn step_greeting(&mut self) -> Result<(), String> {
        // 消费麦克风（丢弃，不喂 ASR/KWS），保持采集活跃、避免回声被拾入
        match self.mic.next(MIC_POLL)? {
            MicEvent::Chunk(_) | MicEvent::Timeout => {}
            MicEvent::Disconnected => return Err("麦克风已断开".to_string()),
        }
        // 等欢迎语合成结果并播放
        while let Some(result) = self.synth.try_recv() {
            match result {
                SynthResult::Done {
                    gen_id,
                    samples,
                    sample_rate,
                } => {
                    if gen_id == self.current_gen {
                        self.speaker.play(samples, sample_rate);
                        self.welcome_played = true;
                    }
                }
                // 流式欢迎语：逐块播放；welcome_played 只在**终态**置位——
                // 块间空窗 rodio drained() 可能瞬时为 true，首块置位会误迁移
                SynthResult::StreamChunk {
                    gen_id,
                    samples,
                    sample_rate,
                } => {
                    if gen_id == self.current_gen {
                        self.speaker.play(samples, sample_rate);
                    }
                }
                SynthResult::StreamDone { gen_id } => {
                    if gen_id == self.current_gen {
                        self.welcome_played = true;
                    }
                }
                SynthResult::Error { message, .. } => {
                    tracing::warn!("欢迎语合成失败，跳过: {message}");
                    self.welcome_played = true; // 跳过欢迎，不卡流程
                }
            }
        }
        // 欢迎语播放完（或合成失败跳过）→ 进 WaitingSpeech
        if self.welcome_played && self.speaker.drained() {
            self.mic.skip_for(SKIP_AFTER_BARGE_IN); // 丢回声尾巴
            self.set_state(SessionEvent::WelcomeDone)?;
        }
        Ok(())
    }

    /// WaitingSpeech：RMS 门控，等用户真正说话（连续超阈值块）才进 ASR。
    fn step_waiting_speech(&mut self) -> Result<(), String> {
        let chunk = match self.mic.next(MIC_POLL)? {
            MicEvent::Chunk(c) => Some(c),
            MicEvent::Timeout => None,
            MicEvent::Disconnected => return Err("麦克风已断开".to_string()),
        };
        if let Some(chunk) = chunk {
            if chunk_rms(&chunk) > self.cfg.vad_silence_threshold {
                self.speech_hits += 1;
                if self.speech_hits >= 2 {
                    self.speech_hits = 0;
                    self.set_state(SessionEvent::SpeechDetected)?; // → Listening
                }
            } else {
                self.speech_hits = 0;
            }
        }
        // 等待超时（无人说话）→ 回待唤醒
        self.speech_wait_accum += MIC_POLL.as_secs_f32();
        if self.speech_wait_accum >= self.cfg.welcome_wait_timeout {
            self.speech_wait_accum = 0.0;
            self.speech_hits = 0;
            self.set_state(SessionEvent::WaitTimeout)?;
        }
        Ok(())
    }

    /// Listening：喂 ASR（流式字幕 / 离线 PCM 缓冲）+ RMS 静音累计；连续静音达
    /// `asr_max_trailing_silence` 判定说完（取最终文本）→ 入历史 → 发起 LLM 生成。
    fn step_listening(&mut self) -> Result<(), String> {
        let chunk = match self.mic.next(MIC_POLL)? {
            MicEvent::Chunk(c) => c,
            MicEvent::Timeout => return Ok(()),
            MicEvent::Disconnected => return Err("麦克风已断开".to_string()),
        };
        // RMS 门控：该块是否超过音量门限（离线用此标记缓冲内是否真有语音，防静音空转写）
        let chunk_secs = self.cfg.asr.chunk_size as f32 / self.cfg.asr.sample_rate as f32;
        let speech_active = chunk_rms(&chunk) > self.cfg.vad_silence_threshold;
        self.asr.feed_chunk(&chunk, speech_active);
        let mut collector = AsrCollector::default();
        self.asr.decode_into(&mut collector);
        // 流式字幕：部分识别结果逐步刷新（覆盖同一行）；离线无 partial，天然不发
        if !collector.partial.is_empty() {
            (self.emit)(VoiceEvent::Transcript {
                text: collector.partial.clone(),
                is_final: false,
            });
        }
        // RMS 静音累计：有语音则重置，否则累加当前块时长
        if speech_active {
            self.silence_accum = 0.0;
        } else {
            self.silence_accum += chunk_secs;
        }
        // 结束判定：sherpa is_final（流式兜底）或连续静音达到 asr_max_trailing_silence
        if let Some(text) = collector.final_text {
            self.handle_user_final(text)?;
        } else if self.silence_accum >= self.cfg.asr_max_trailing_silence {
            let text = self.force_finalize_asr();
            self.handle_user_final(text)?;
        }
        Ok(())
    }

    /// 轮询文字输入收件箱（输入条窗口）。文字与 ASR 最终文本等价，复用
    /// [`Self::handle_user_final`] 进入 LLM → TTS → 落盘的完整对话链路。
    ///
    /// 两个时序坑的规避：
    /// - `do_barge_in` 的 `llm.cancel()` 只置标志，worker 释放 `generating` 互斥有延迟，
    ///   此刻发起 `generate` 必得 `Busy`——故生成中只排队，后续轮次（≤100ms 粒度）重试；
    /// - 被打断轮次的迟到 `Finished`/`Token` 会残留在 `llm_rx`，若带进新一轮会让
    ///   `step_thinking` 的 `reply_done` 提前置位形成空轮——故发起前轮前排空。
    fn poll_text_input(&mut self) -> Result<(), String> {
        // 先收集到局部 Vec 再处理（try_iter 的不可变借用不能横跨 &mut self 调用）
        let incoming: Vec<String> = self
            .text_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for text in incoming {
            let text = text.trim().to_string();
            if !text.is_empty() {
                self.pending_texts.push_back(text);
            }
        }
        if self.pending_texts.is_empty() {
            return Ok(());
        }
        // 上一轮 cancel 尚未落地（或 GUI 正在生成）：等下一轮循环
        if self.llm.is_generating() {
            return Ok(());
        }
        match self.state {
            // 欢迎语/生成/播报中：先打断（清理 reply/synth/speaker），文字留队列下轮处理
            SessionState::Greeting | SessionState::Thinking | SessionState::Speaking => {
                self.do_barge_in();
            }
            SessionState::Armed | SessionState::WaitingSpeech | SessionState::Listening => {
                let text = self.pending_texts.pop_front().expect("队列非空已检查");
                // Listening 中 ASR 可能残留 partial 音频，丢弃避免后续误转写混入
                if matches!(self.state, SessionState::Listening) {
                    self.asr.reset(&self.cfg.asr);
                }
                // 排空上一轮遗留的 LLM 事件，再发起新一轮
                while self.llm_rx.try_recv().is_ok() {}
                self.handle_user_final(text)?;
            }
            SessionState::Idle => {
                self.pending_texts.clear();
            }
        }
        Ok(())
    }

    /// 处理一句最终用户文本：非空 → 入历史并发起 LLM；空（没真说话/识别失败）→
    /// **保持聆听不退出**（重建 ASR 流，状态留在 Listening；跟听循环免唤醒持续监听）。
    fn handle_user_final(&mut self, text: String) -> Result<(), String> {
        let text = text.trim().to_string();
        self.silence_accum = 0.0;
        if text.is_empty() {
            // force_finalize 已终结旧句，复位后端后继续聆听（不回待唤醒）
            tracing::debug!("[voice] ASR 空识别，保持聆听");
            self.asr.reset(&self.cfg.asr);
            return Ok(());
        }
        (self.emit)(VoiceEvent::Transcript {
            text: text.clone(),
            is_final: true,
        });
        self.turns += 1;
        self.history
            .push(InputItem::Message(ChatMessage::new(ChatRole::User, text)));
        truncate_history(&mut self.history, self.cfg.history_max);
        self.start_reply();
        let input = build_llm_input(&self.cfg.llm.system_prompt, &self.history);
        match self.llm.generate(input, self.cfg.llm.params.clone()) {
            Ok(()) => {}
            Err(e) => {
                // 生成互斥（GUI 正在生成）或 worker 退出 → 转错误事件，回待唤醒（不崩溃）
                (self.emit)(VoiceEvent::Error {
                    kind: ErrorKind::Llm,
                    message: e.to_string(),
                });
                self.set_state(SessionEvent::WaitTimeout)?;
                return Ok(());
            }
        }
        self.set_state(SessionEvent::UserUtteranceFinal)
    }

    /// 强制结束当前句取最终文本：流式补尾部静音 + drain；离线整段 `transcribe_samples`
    /// （按模型族分派到 `AsrBackend::finalize`）。
    fn force_finalize_asr(&mut self) -> String {
        self.asr.finalize(&self.cfg.asr)
    }

    /// 进入一轮新生成前的重置（gen 递增、清空上一轮回复状态、复位合成取消）。
    fn start_reply(&mut self) {
        self.current_gen += 1;
        self.reply = ReplyAccumulator::new();
        self.reply_done = false;
        self.first_sentence = false;
        self.synth_enqueued = 0;
        self.synth_consumed = 0;
        self.pending_speech.clear();
        self.stream_gate = SentencePlayGate::default();
        // 首响打点：本轮生成起点；首句/首块待入队与播放时置位
        self.t_reply_start = Some(Instant::now());
        self.t_first_sentence = None;
        self.t_first_audio = None;
        self.synth.clear_cancel();
    }

    /// 把一句文本入队合成，并记录其文本（播放时弹出打印）。
    fn enqueue_sentence(&mut self, sentence: String) {
        (self.emit)(VoiceEvent::ReplySentence {
            sentence: sentence.clone(),
        });
        self.pending_speech.push_back(sentence.clone());
        self.synth.enqueue(sentence, self.current_gen);
        self.synth_enqueued += 1;
        if self.t_first_sentence.is_none() {
            self.t_first_sentence = Some(Instant::now());
        }
    }

    /// 消费 LLM 事件流：Token 切句入队合成 / `Finished` 置位 / 错误转发。
    ///
    /// `step_thinking` 与 `step_speaking` 都调用：句级流式下 LLM 在 Speaking 阶段仍在
    /// 产出，`Finished` 可能晚于首句入队才到达（worker 在最后一个 token 后还要采样 EOG
    /// 才广播 `Finished`），若 Speaking 不消费则会永久丢失 → `reply_done` 恒为 false →
    /// 播完卡在 Speaking。`first_sentence` 标志保证 Speaking 阶段收到新句子不会重复
    /// 触发 `FirstSentenceEnqueued` 迁移。
    fn poll_llm_events(&mut self) -> Result<(), String> {
        loop {
            let ev = match self.llm_rx.try_recv() {
                Ok(ev) => ev,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            };
            match ev {
                LlmEvent::Token(delta) => {
                    let (visible, sentences) = self.reply.push_token(&delta.text);
                    // 只输出可见文本（思考块被过滤，不上屏）
                    if !visible.is_empty() {
                        (self.emit)(VoiceEvent::Token { delta: visible });
                    }
                    for s in sentences {
                        self.enqueue_sentence(s);
                        if !self.first_sentence {
                            self.first_sentence = true;
                            self.set_state(SessionEvent::FirstSentenceEnqueued)?;
                        }
                    }
                }
                LlmEvent::Finished(reason) => {
                    self.reply_done = true;
                    if let Some(tail) = self.reply.finish() {
                        self.enqueue_sentence(tail);
                    }
                    // 完整可见回复：入历史（LLM 上下文）+ 随 ReplyFinished 下发（宿主持久化记录）
                    let reply_text = self.reply.take_text();
                    if let Some(reply) = reply_text.clone() {
                        self.history.push(InputItem::Message(ChatMessage::new(
                            ChatRole::Assistant,
                            reply,
                        )));
                        truncate_history(&mut self.history, self.cfg.history_max);
                    }
                    (self.emit)(VoiceEvent::ReplyFinished {
                        reason: format!("{reason:?}"),
                        text: reply_text,
                    });
                }
                LlmEvent::Error(e) => {
                    self.reply_done = true;
                    (self.emit)(VoiceEvent::Error {
                        kind: ErrorKind::Llm,
                        message: e,
                    });
                }
                LlmEvent::Status { .. } => {}
            }
        }
        Ok(())
    }

    /// Thinking：喂 KWS（打断监听）+ 消费 LLM 事件（流式打印 token），把切句入队合成。
    fn step_thinking(&mut self) -> Result<(), String> {
        self.listen_barge_in()?;
        self.poll_llm_events()?;
        // 未切出任何句子（空回复/立即出错）→ 直接回听
        if self.reply_done && self.synth_enqueued == 0 {
            self.finish_reply()?;
        }
        Ok(())
    }

    /// Speaking：喂 KWS（打断监听）+ 消费 LLM 事件（晚到的 token/Finished）+ 按序播放，播完回听。
    ///
    /// 消费计数只计**当前 gen 的终态**（Done/StreamDone/Error）：流式分块不计，
    /// 过期结果（打断后迟到，流式下取消延迟 = chunk 边界）不计数——否则迟到
    /// 终态会把 `synth_consumed` 抬到 `synth_enqueued` 之上，完成判定永不成立。
    fn step_speaking(&mut self) -> Result<(), String> {
        self.listen_barge_in()?;
        self.poll_llm_events()?;
        while let Some(result) = self.synth.try_recv() {
            match result {
                SynthResult::Done {
                    gen_id,
                    samples,
                    sample_rate,
                } => {
                    if gen_id == self.current_gen {
                        self.synth_consumed += 1;
                        // 弹出与入队顺序对应的句子文本（播放状态展示）
                        let text = self.pending_speech.pop_front().unwrap_or_default();
                        (self.emit)(VoiceEvent::PlaySentence {
                            sentence: text.clone(),
                        });
                        self.speaker.play(samples, sample_rate);
                        self.mark_first_audio(false);
                    }
                    // 过期结果（打断后迟到）直接丢弃
                }
                // 流式分块：首块弹 pending_speech + 发 PlaySentence（gate 保证每句
                // 恰一次），后续块沿用直接追加播放（rodio append 按序）
                SynthResult::StreamChunk {
                    gen_id,
                    samples,
                    sample_rate,
                } => {
                    if gen_id == self.current_gen {
                        if self.stream_gate.on_stream_chunk() {
                            let text = self.pending_speech.pop_front().unwrap_or_default();
                            (self.emit)(VoiceEvent::PlaySentence { sentence: text });
                        }
                        self.speaker.play(samples, sample_rate);
                        self.mark_first_audio(true);
                    }
                }
                SynthResult::StreamDone { gen_id } => {
                    if gen_id == self.current_gen {
                        self.synth_consumed += 1;
                        // 零块句（空流）：终态补弹保持 pending_speech 对齐（静默，
                        // 不发 PlaySentence——没有可播内容）
                        if self.stream_gate.on_terminal() {
                            self.pending_speech.pop_front();
                        }
                    }
                }
                SynthResult::Error { gen_id, message } => {
                    if gen_id == self.current_gen {
                        self.synth_consumed += 1;
                        // 已播过块的句子 gate 已弹过；零块句补弹对齐
                        if self.stream_gate.on_terminal() {
                            self.pending_speech.pop_front();
                        }
                        (self.emit)(VoiceEvent::Error {
                            kind: ErrorKind::Synth,
                            message,
                        });
                    }
                }
            }
        }
        // 回复生成完 + 合成全消费 + 播放队列播完 → 回听
        if self.reply_done && self.synth_enqueued == self.synth_consumed && self.speaker.drained() {
            self.finish_reply()?;
        }
        Ok(())
    }

    /// Thinking/Speaking 期间喂麦克风给 KWS（打断词/唤醒词监听）。
    fn listen_barge_in(&mut self) -> Result<(), String> {
        let chunk = match self.mic.next(MIC_POLL)? {
            MicEvent::Chunk(c) => c,
            MicEvent::Timeout => return Ok(()),
            MicEvent::Disconnected => return Err("麦克风已断开".to_string()),
        };
        self.kws.feed(&self.kws_stream, &chunk);
        let mut reaction = BargeInReaction {
            flag: &self.barge_in,
        };
        let _ = self.kws.detect(&self.kws_stream, &mut reaction);
        Ok(())
    }

    /// 回复播完（或无内容可播）→ 依 `decide_finish` 分流：停止 / 进跟听窗口 / 回待唤醒。
    fn finish_reply(&mut self) -> Result<(), String> {
        match decide_finish(self.cfg.follow_up, self.cfg.max_turns, self.turns) {
            FinishAction::Stop { max } => {
                self.running.store(false, Ordering::Relaxed);
                (self.emit)(VoiceEvent::Stopped {
                    reason: StoppedReason::MaxTurns { max },
                    turns: self.turns,
                });
            }
            FinishAction::FollowUp => {
                // 丢 TTS 回声尾巴，避免被 ASR 识别成用户内容
                self.mic.skip_for(SKIP_AFTER_REPLY);
                self.set_state(SessionEvent::FollowUp)?; // → Listening 直接聆听
            }
            FinishAction::ToArmed => self.set_state(SessionEvent::ReplyFinished)?,
        }
        Ok(())
    }

    /// 打断序列：取消 LLM、停播、清合成、作废在途结果、回听并丢回声尾巴。
    fn do_barge_in(&mut self) {
        (self.emit)(VoiceEvent::BargeIn);
        self.llm.cancel();
        self.current_gen += 1;
        self.speaker.stop();
        self.reply = ReplyAccumulator::new();
        self.reply_done = false;
        self.first_sentence = false;
        self.pending_speech.clear();
        self.stream_gate = SentencePlayGate::default();
        self.synth.cancel_all();
        self.synth_enqueued = 0;
        self.synth_consumed = 0;
        self.t_wake = None;
        self.t_reply_start = None;
        self.t_first_sentence = None;
        self.t_first_audio = None;
        self.barge_in.store(false, Ordering::Relaxed);
        if let Err(e) = self.set_state(SessionEvent::BargeIn) {
            (self.emit)(VoiceEvent::Error {
                kind: ErrorKind::BargeIn,
                message: e,
            });
        }
        self.mic.skip_for(SKIP_AFTER_BARGE_IN);
    }

    /// 首次播放打点（一轮回复只打一次）：输出 唤醒→首句 / 生成→首句 /
    /// 首句→首块 三段耗时。`streamed` 标记流式/整段（A/B 验收对照）。
    fn mark_first_audio(&mut self, streamed: bool) {
        if self.t_first_audio.is_some() {
            return; // 只打首轮首块（后续句/块不再打）
        }
        self.t_first_audio = Some(Instant::now());
        let ms = |a: Option<Instant>, b: Option<Instant>| {
            a.zip(b)
                .map(|(a, b)| format!("{}ms", b.duration_since(a).as_millis()))
                .unwrap_or_else(|| "-".to_string())
        };
        tracing::info!(
            "[voice] 首响打点：唤醒→首句 {} 生成→首句 {} 首句→首块 {}（{}）",
            ms(self.t_wake, self.t_first_sentence),
            ms(self.t_reply_start, self.t_first_sentence),
            ms(self.t_first_sentence, self.t_first_audio),
            if streamed { "流式" } else { "整段" }
        );
    }
}

/// 流式句「首块已消费」跟踪（纯逻辑，可单测）。
///
/// 不变量：`pending_speech` 每句恰弹一次——首块弹（并发 `PlaySentence`），
/// 零块句（空流）由终态补弹；终态（`StreamDone`/`Error`）后复位，下一句重新
/// 起算。非流式句不触碰本类型（`Done` 直接弹，行为等价于 gate 恒 false）。
#[derive(Default, Debug)]
struct SentencePlayGate {
    started: bool,
}

impl SentencePlayGate {
    /// 流式分块到达：首块返回 true（调用方弹 pending_speech + 发 PlaySentence），
    /// 后续块返回 false。
    fn on_stream_chunk(&mut self) -> bool {
        if self.started {
            false
        } else {
            self.started = true;
            true
        }
    }

    /// 终态到达：返回「是否尚未弹过」（零块句需补弹），并复位供下一句。
    fn on_terminal(&mut self) -> bool {
        let pending = !self.started;
        self.started = false;
        pending
    }
}

/// 一句话的回复累积：过滤思考块 → 拼接可见文本 + 切句（供合成入队）。
///
/// 独立成可测结构：`push_token` 返回（可见文本, 本次切出的句子）；`finish` 冲刷
/// 残余句；`take_text` 取完整可见回复（入历史后即丢弃，打断时直接 new 一个丢弃）。
#[derive(Default)]
pub struct ReplyAccumulator {
    text: String,
    splitter: SentenceSplitter,
    filter: ThinkingFilter,
}

impl ReplyAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 吸收一段 token 增量：过滤思考块后返回（可见文本, 切出的完整句子）。
    ///
    /// 思考块（`<think>...</think>`）内容**不进可见文本**，因此不会被切句合成、
    /// 不会入历史、不会上屏。
    pub fn push_token(&mut self, delta: &str) -> (String, Vec<String>) {
        let visible = self.filter.feed(delta);
        if visible.is_empty() {
            return (String::new(), Vec::new());
        }
        self.text.push_str(&visible);
        let sentences = self.splitter.push(&visible);
        (visible, sentences)
    }

    /// 生成结束：冲刷过滤器残余 → 切句 → 返回最后一句话（`None` = 无残余可合成）。
    pub fn finish(&mut self) -> Option<String> {
        let tail = self.filter.finish();
        if !tail.is_empty() {
            self.text.push_str(&tail);
            self.splitter.push(&tail);
        }
        let rest = self.splitter.finish();
        if rest.is_empty() { None } else { Some(rest) }
    }

    /// 完整可见回复文本（trim 后；空返回 `None`）。
    pub fn take_text(&mut self) -> Option<String> {
        let t = self.text.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    }
}

/// 思考块标签。`<think>` 块内的内容（思考过程）不送 TTS、不入历史、不上屏。
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

/// 流式思考块过滤器：丢弃 `<think>...</think>` 之间的内容。
///
/// 流式 token 可能把标签拆成多段（如 `<th` + `ink>`），因此保留尾部若干字节
/// （最长标签长度 - 1）等待补全；未闭合的思考块在 `finish` 时整体丢弃。
#[derive(Default)]
struct ThinkingFilter {
    in_think: bool,
    buffer: String,
}

impl ThinkingFilter {
    /// 吸收增量，返回过滤掉思考块后的可见文本。
    fn feed(&mut self, delta: &str) -> String {
        self.buffer.push_str(delta);
        let mut out = String::new();
        loop {
            if self.in_think {
                match self.buffer.find(THINK_CLOSE) {
                    Some(pos) => {
                        self.buffer.drain(..pos + THINK_CLOSE.len());
                        self.in_think = false;
                        // 继续处理 </think> 之后的内容
                    }
                    None => {
                        // 思考块内：只保留可能是 `</think>` 前缀的尾部，其余丢弃
                        let keep = tag_prefix_tail(&self.buffer);
                        let cut = keep.unwrap_or(self.buffer.len());
                        self.buffer.drain(..cut);
                        break;
                    }
                }
            } else {
                match self.buffer.find(THINK_OPEN) {
                    Some(pos) => {
                        out.push_str(&self.buffer[..pos]);
                        self.buffer.drain(..pos + THINK_OPEN.len());
                        self.in_think = true;
                    }
                    None => {
                        // 正常文本：只保留可能是 `<think>` 前缀的尾部，其余输出
                        let keep = tag_prefix_tail(&self.buffer);
                        let cut = keep.unwrap_or(self.buffer.len());
                        out.push_str(&self.buffer[..cut]);
                        self.buffer.drain(..cut);
                        break;
                    }
                }
            }
        }
        out
    }

    /// 生成结束：返回可见残余（思考块未闭合则丢弃）。
    fn finish(&mut self) -> String {
        if self.in_think {
            self.buffer.clear();
            String::new()
        } else {
            std::mem::take(&mut self.buffer)
        }
    }
}

/// 若 `buffer` 末尾是 `<think>` / `</think>` 的前缀（跨 token 残片），返回其起始
/// 字节位置（需保留等待补全）；否则返回 `None`（整段可安全输出/丢弃）。
///
/// 只检查标签前缀，因此正常文本**不会**被尾部延迟截断。
fn tag_prefix_tail(buffer: &str) -> Option<usize> {
    const MAX: usize = THINK_CLOSE.len() - 1; // 最长标签 - 1
    let candidates = [THINK_OPEN, THINK_CLOSE];
    // 从后往前枚举 char 边界，检查后缀是否为标签前缀
    for (i, _) in buffer.char_indices().rev() {
        let suffix = &buffer[i..];
        if suffix.len() > MAX {
            break;
        }
        if candidates.iter().any(|tag| tag.starts_with(suffix)) {
            return Some(i);
        }
    }
    None
}

/// 构造传给 LLM 的输入：System prompt + 历史消息（多轮上下文）。
fn build_llm_input(system_prompt: &str, history: &[InputItem]) -> Vec<InputItem> {
    let mut input = vec![InputItem::Message(ChatMessage::new(
        ChatRole::System,
        system_prompt.to_string(),
    ))];
    input.extend(history.iter().cloned());
    input
}

/// 裁剪历史到最近 `max` 条（丢弃最早的多余消息）。
fn truncate_history(history: &mut Vec<InputItem>, max: usize) {
    if history.len() > max {
        let drop = history.len() - max;
        history.drain(..drop);
    }
}

/// 回复播完后的去向。
enum FinishAction {
    /// 达 `max_turns` 上限，结束会话。
    Stop { max: u32 },
    /// 进入跟听窗口（WaitingSpeech，复用 RMS 门控）。
    FollowUp,
    /// 回待唤醒（原行为，需再喊唤醒词）。
    ToArmed,
}

/// 回复播完后去哪：`max_turns` 优先级最高（达上限直接停，不开跟听窗口），
/// 其次按 `follow_up` 开关决定进跟听窗口或回待唤醒。
fn decide_finish(follow_up: bool, max_turns: Option<u32>, turns: u32) -> FinishAction {
    if let Some(max) = max_turns
        && turns >= max
    {
        FinishAction::Stop { max }
    } else if follow_up {
        FinishAction::FollowUp
    } else {
        FinishAction::ToArmed
    }
}

/// 计算一段 f32 mono 音频的 RMS（均方根）音量，用于「真正说话」门控与静音累计。
fn chunk_rms(chunk: &[f32]) -> f32 {
    if chunk.is_empty() {
        return 0.0;
    }
    let sum: f32 = chunk.iter().map(|s| s * s).sum();
    (sum / chunk.len() as f32).sqrt()
}

/// ASR 反应：收集部分（流式字幕）与最终识别文本；一句说完（`is_final`）返回 `Stop`。
#[derive(Default)]
struct AsrCollector {
    final_text: Option<String>,
    partial: String,
}

impl AsrReaction for AsrCollector {
    fn on_result(&mut self, result: &AsrResult) -> ReactionOutcome {
        if result.is_final && !result.text.trim().is_empty() {
            self.final_text = Some(result.text.clone());
            return ReactionOutcome::Stop;
        }
        // 部分结果（未到 endpoint）：更新流式字幕
        if !result.text.is_empty() {
            self.partial = result.text.clone();
        }
        ReactionOutcome::Continue
    }
}

/// TTS 热切换音色解析失败的兜底决策（`refresh_tts_if_switched` 的可测核心）：
/// - 强制克隆族（qwen3_tts Base，上游无 auto voice）→ `None`：不换入新引擎，
///   保留旧引擎（换入一个每句必报错的新引擎比不换更糟）；
/// - 其余族 → `Some(Sid(0))`：omnivoice/voxcpm2 是 server auto voice，固定音色族
///   是默认音色，均为可用语义。
fn hot_swap_voice_fallback(
    kind: crate::tts::config::TtsModelKind,
) -> Option<crate::tts::TtsVoiceParams> {
    let clone_required = crate::audiocpp::families::family_desc(kind).is_some_and(|d| {
        matches!(
            d.voice_semantics,
            crate::audiocpp::families::VoiceSemantics::ReferenceCloneRequired
        )
    });
    if clone_required {
        None
    } else {
        Some(crate::tts::TtsVoiceParams::Sid(0))
    }
}

/// KWS 反应（Armed 待唤醒）：命中唤醒词即停止检测并记录关键词 → 切换 ASR。
#[derive(Default)]
struct WakeReaction {
    keyword: Option<String>,
}

impl Reaction for WakeReaction {
    fn on_keyword(&mut self, result: &KwsResult) -> ReactionOutcome {
        self.keyword = Some(result.keyword.clone());
        ReactionOutcome::Stop
    }
}

/// KWS 反应（Thinking/Speaking 打断监听）：命中即置位打断标志（继续监听，不 Stop）。
struct BargeInReaction<'a> {
    flag: &'a AtomicBool,
}

impl Reaction for BargeInReaction<'_> {
    fn on_keyword(&mut self, _result: &KwsResult) -> ReactionOutcome {
        self.flag.store(true, Ordering::Relaxed);
        ReactionOutcome::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 热切换音色解析失败兜底：强制克隆族（qwen3_tts 两尺寸）不换引擎（None），
    /// 其余族兜底 Sid(0)（auto voice / 默认音色语义）。
    #[test]
    fn test_hot_swap_voice_fallback_by_family() {
        use crate::tts::config::TtsModelKind;
        for kind in [TtsModelKind::Qwen3Tts06, TtsModelKind::Qwen3Tts17] {
            assert!(
                hot_swap_voice_fallback(kind).is_none(),
                "{kind:?} 强制克隆族应保留旧引擎（不换入）"
            );
        }
        for kind in [
            TtsModelKind::Omnivoice,
            TtsModelKind::Voxcpm2,
            TtsModelKind::Zipvoice,
        ] {
            assert!(
                matches!(
                    hot_swap_voice_fallback(kind),
                    Some(crate::tts::TtsVoiceParams::Sid(0))
                ),
                "{kind:?} 应兜底 Sid(0)"
            );
        }
    }

    #[test]
    fn test_reply_accumulator_splits_and_joins() {
        let mut r = ReplyAccumulator::new();
        // 增量 token，无句号前不切出
        let (visible, sentences) = r.push_token("你好，");
        assert_eq!(visible, "你好，");
        assert!(sentences.is_empty());
        assert!(r.push_token("世界").1.is_empty());
        // 句号切出完整句
        let (_, sentences) = r.push_token("。这是第二句");
        assert_eq!(sentences, vec!["你好，世界。".to_string()]);
        // 完整回复文本拼接正确
        assert_eq!(r.take_text().as_deref(), Some("你好，世界。这是第二句"));
    }

    #[test]
    fn test_reply_accumulator_finish_flushes_tail() {
        let mut r = ReplyAccumulator::new();
        r.push_token("没有标点的尾巴");
        // finish 返回残余句
        assert_eq!(r.finish().as_deref(), Some("没有标点的尾巴"));
        // take_text 仍含完整文本
        assert_eq!(r.take_text().as_deref(), Some("没有标点的尾巴"));
    }

    #[test]
    fn test_reply_accumulator_empty() {
        let mut r = ReplyAccumulator::new();
        assert!(r.push_token("  ").1.is_empty());
        assert_eq!(r.finish(), None);
        assert_eq!(r.take_text(), None);
    }

    #[test]
    fn test_thinking_block_filtered_from_tts_and_history() {
        let mut r = ReplyAccumulator::new();
        // 思考块内容不进入可见文本（不切句、不入历史、不上屏）
        let (visible, sentences) = r.push_token("<think>用户问\n");
        assert_eq!(visible, "");
        assert!(sentences.is_empty());
        let (visible, sentences) = r.push_token("我来分析一下。</think>");
        assert_eq!(visible, "");
        assert!(sentences.is_empty());
        // 闭合后的可见内容正常
        let (visible, sentences) = r.push_token("好的，这是回答。");
        assert_eq!(visible, "好的，这是回答。");
        assert_eq!(sentences, vec!["好的，这是回答。".to_string()]);
        // 历史只含可见文本（思考内容不进历史）
        assert_eq!(r.take_text().as_deref(), Some("好的，这是回答。"));
    }

    #[test]
    fn test_thinking_tag_split_across_tokens() {
        let mut r = ReplyAccumulator::new();
        // `<think>` 被拆成多段 token
        assert_eq!(r.push_token("<th").0, "");
        assert_eq!(r.push_token("ink>思考内容").0, "");
        assert_eq!(r.push_token("</th").0, "");
        assert_eq!(r.push_token("ink>答案").0, "答案");
        assert_eq!(r.take_text().as_deref(), Some("答案"));
    }

    #[test]
    fn test_unclosed_thinking_dropped_at_finish() {
        let mut r = ReplyAccumulator::new();
        r.push_token("<think>未闭合的思考");
        // finish 丢弃思考块，无残余
        assert_eq!(r.finish(), None);
        assert_eq!(r.take_text(), None);
    }

    #[test]
    fn test_thinking_without_close_then_normal() {
        let mut r = ReplyAccumulator::new();
        r.push_token("<think>思考");
        let (visible, _) = r.push_token("</think>正式回复");
        assert_eq!(visible, "正式回复");
        let (_, sentences) = r.push_token("。第二句");
        assert_eq!(sentences, vec!["正式回复。".to_string()]);
        assert_eq!(r.take_text().as_deref(), Some("正式回复。第二句"));
    }

    #[test]
    fn test_thinking_filter_multibyte_safety() {
        // 思考块内夹杂多字节中文，切分不得 panic（char 边界安全）
        let mut f = ThinkingFilter::default();
        let out = f.feed("<think>中文思考内容很长很长</think>可见");
        assert_eq!(out, "可见");
    }

    #[test]
    fn test_build_llm_input_prepends_system() {
        let history = vec![
            InputItem::Message(ChatMessage::new(ChatRole::User, "你好")),
            InputItem::Message(ChatMessage::new(ChatRole::Assistant, "你好！")),
        ];
        let input = build_llm_input("你是助手", &history);
        assert_eq!(input.len(), 3);
        assert!(matches!(
            &input[0],
            InputItem::Message(m) if m.role == ChatRole::System && m.content == "你是助手"
        ));
        assert!(matches!(
            &input[1],
            InputItem::Message(m) if m.role == ChatRole::User && m.content == "你好"
        ));
        assert!(matches!(
            &input[2],
            InputItem::Message(m) if m.role == ChatRole::Assistant && m.content == "你好！"
        ));
    }

    #[test]
    fn test_build_llm_input_empty_history() {
        let input = build_llm_input("你是助手", &[]);
        assert_eq!(input.len(), 1);
        assert!(matches!(&input[0], InputItem::Message(m) if m.role == ChatRole::System));
    }

    #[test]
    fn test_truncate_history_keeps_recent() {
        let mut history = vec![
            InputItem::Message(ChatMessage::new(ChatRole::User, "1")),
            InputItem::Message(ChatMessage::new(ChatRole::User, "2")),
            InputItem::Message(ChatMessage::new(ChatRole::User, "3")),
            InputItem::Message(ChatMessage::new(ChatRole::User, "4")),
        ];
        truncate_history(&mut history, 2);
        assert_eq!(history.len(), 2);
        // 保留最近的 3、4
        assert!(matches!(&history[0], InputItem::Message(m) if m.content == "3"));
        assert!(matches!(&history[1], InputItem::Message(m) if m.content == "4"));
    }

    #[test]
    fn test_truncate_history_within_limit_unchanged() {
        let mut history = vec![
            InputItem::Message(ChatMessage::new(ChatRole::User, "1")),
            InputItem::Message(ChatMessage::new(ChatRole::User, "2")),
        ];
        truncate_history(&mut history, 4);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_decide_finish_max_turns_wins_over_follow_up() {
        // 达轮上限直接停，不开跟听窗口
        assert!(matches!(
            decide_finish(true, Some(3), 3),
            FinishAction::Stop { max: 3 }
        ));
        assert!(matches!(
            decide_finish(false, Some(2), 5),
            FinishAction::Stop { max: 2 }
        ));
    }

    #[test]
    fn test_decide_finish_follow_up_when_enabled() {
        // 开启跟听且未达上限 → 进跟听窗口
        assert!(matches!(
            decide_finish(true, None, 3),
            FinishAction::FollowUp
        ));
        assert!(matches!(
            decide_finish(true, Some(5), 3),
            FinishAction::FollowUp
        ));
    }

    #[test]
    fn test_decide_finish_to_armed_when_disabled() {
        // 关闭跟听 → 回待唤醒（原行为）
        assert!(matches!(
            decide_finish(false, None, 3),
            FinishAction::ToArmed
        ));
        assert!(matches!(
            decide_finish(false, Some(5), 3),
            FinishAction::ToArmed
        ));
    }

    #[test]
    fn test_chunk_rms_values() {
        assert_eq!(chunk_rms(&[]), 0.0);
        assert_eq!(chunk_rms(&[0.0, 0.0, 0.0]), 0.0);
        // 恒定振幅 → RMS = 该振幅
        assert!((chunk_rms(&[0.5, 0.5]) - 0.5).abs() < 1e-6);
        // 峰值 1.0 正弦 → RMS ≈ 0.707
        let sine: Vec<f32> = (0..3200)
            .map(|i| ((i as f32 / 3200.0) * std::f32::consts::TAU).sin())
            .collect();
        assert!((chunk_rms(&sine) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        // 幅度更大 → RMS 更大（用于阈值门控判断）
        assert!(chunk_rms(&[0.9, 0.9]) > chunk_rms(&[0.1, 0.1]));
    }

    // ---------- SentencePlayGate：pending_speech 每句恰弹一次的不变量 ----------

    #[test]
    fn test_gate_first_chunk_then_subsequent() {
        let mut gate = SentencePlayGate::default();
        assert!(gate.on_stream_chunk(), "首块应触发弹句");
        assert!(!gate.on_stream_chunk(), "后续块不再触发");
        assert!(!gate.on_stream_chunk());
    }

    #[test]
    fn test_gate_terminal_resets_for_next_sentence() {
        let mut gate = SentencePlayGate::default();
        gate.on_stream_chunk();
        assert!(!gate.on_terminal(), "已弹过的句终态不补弹");
        // 复位后下一句首块重新触发
        assert!(gate.on_stream_chunk(), "终态复位后下一句首块生效");
    }

    #[test]
    fn test_gate_zero_chunk_sentence_needs_pop() {
        let mut gate = SentencePlayGate::default();
        assert!(
            gate.on_terminal(),
            "零块句（空流）终态需补弹保持 pending_speech 对齐"
        );
        // 注：不 assert 二次 on_terminal 为 false——「每句恰一个终态」由合成线程
        // 协议保证，gate 只跟踪首块；双终态序列不存在，无需防御。
    }

    #[test]
    fn test_gate_terminal_after_no_chunks_next_sentence_first_chunk() {
        // 序列：句 1 零块终态（补弹）→ 句 2 首块（应触发）
        let mut gate = SentencePlayGate::default();
        assert!(gate.on_terminal());
        assert!(gate.on_stream_chunk(), "句 2 首块在句 1 补弹复位后正常触发");
        assert!(!gate.on_terminal());
    }
}
