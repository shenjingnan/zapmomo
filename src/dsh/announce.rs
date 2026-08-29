/// dsh 事件语音播报：独立 worker 线程流式合成 + rodio 流式播放。
///
/// - 与 voice 会话的互斥由调用方（管线）判断「voice 会话是否运行」决定是否调用
/// - 自身防重叠：worker 串行播报；排队容量 1（`SyncSender::try_send`），溢出丢弃——
///   气泡已传达信息，语音只是增强
/// - **流式**：audiocpp 引擎走 `synthesize_streaming` 逐块产出、rodio 队列边收边播
///   （首块即出声，与 voice 会话链路同款体验）；整段一次性合成会让一句长文案
///   等满「音频时长 × RTF」才出声
/// - 合成/播放可注入（测试无音频设备依赖）
use crate::tts;
use crate::voice::player::AudioPlayer;
use crate::voice::state::SessionState;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

/// 语音会话当前是否处于 dsh 播报的可插播空档（空档插播共存策略）。
///
/// 会话未运行可直接播；运行中只有 `Armed`（待唤醒空闲）可插——`Listening` 插话
/// 会被麦克风拾回污染 ASR、`Speaking` 插话与 TTS 重叠、`Thinking`/`Greeting`/
/// `WaitingSpeech` 均为对话进行中的忙态。`Armed` 的回声风险已由 barge-in 架构
/// 消化（Speaking 期间 KWS 本就持续监听 TTS 回声，非唤醒词内容不触发）。
/// `phase = None` 表示宿主未上报状态（防御：视为不可插，等待路径兜底）。
pub fn voice_slot_available(running: bool, phase: Option<SessionState>) -> bool {
    !running || matches!(phase, Some(SessionState::Armed | SessionState::Idle))
}

/// 流式合成器：产出文本的音频块序列（块样本, 采样率）；返回 Err = 合成失败。
type SynthFn = dyn Fn(&str, &mut dyn FnMut(&[f32], i32) -> bool) -> Result<(), String> + Send;

/// 播放器工厂：打开输出设备（每条播报开一次，失败不缓存下次重试）。
type OpenPlayerFn = dyn Fn() -> Result<Box<dyn AudioPlayer>, String> + Send;

pub struct Announcer {
    tx: SyncSender<String>,
    // JoinHandle 非 Sync，用 Mutex 包一层使 Announcer 可跨线程共享（Tauri state 用）；
    // 不 join：drop Announcer -> tx 释放 -> worker 的 recv 出错自然退出（非阻塞）
    _handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Announcer {
    /// 注入合成器/播放器工厂的构造（测试用）。
    pub fn with(synth: Box<SynthFn>, open_player: Box<OpenPlayerFn>) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        // 命名线程便于日志定位（同 voice 合成线程的 voice-tts 命名惯例）
        let handle = std::thread::Builder::new()
            .name("dsh-announce".to_string())
            .spawn(move || run_worker(rx, synth, open_player))
            .expect("spawn dsh-announce 线程失败");
        Self {
            tx,
            _handle: Mutex::new(Some(handle)),
        }
    }

    /// 生产构造：sherpa/audiocpp TTS 流式合成 + rodio Speaker 流式播放
    /// （默认音色/语速）。
    /// TTS 模型未就绪返回 Err（调用方降级为只出气泡；下次事件会重试）。
    pub fn try_new() -> Result<Self, String> {
        let settings = crate::config::settings::load_settings()?;
        let tts_settings = settings.as_ref().and_then(|s| s.tts.clone());
        let cfg = tts::config::resolve(tts_settings.as_ref(), None)?;
        // 用户显式关闭 TTS 时不应播报（与其它语音路径的 enabled 语义一致）
        if !cfg.enabled {
            return Err("TTS 未启用（[tts].enabled = false）".to_string());
        }
        // 逐文件预检（backend 感知，与 synthesize_tts 一致），给出明确的
        // 「模型未就绪」错误而非深层引擎报错
        tts::config::preflight(&cfg).map_err(|e| format!("TTS 模型未就绪: {e}"))?;
        // 合成音色参数统一解析：active 角色包带克隆音色时优先（Announcer 每次构建时
        // 探测，天然跟随伙伴切换），否则用配置默认音色/语速播报。
        let character = crate::companion::active_character_voice();
        let voice = tts::voice::resolve_voice_params(
            &cfg,
            None,
            None,
            character.as_ref().map(|v| v.wav.as_path()),
            character.as_ref().map(|v| v.text.as_str()),
        )?;
        let engine = tts::TtsEngine::new(cfg.clone())?;
        let speed = cfg.speed;
        let supports_streaming = engine.supports_streaming();
        let synth = move |text: &str, on_chunk: &mut dyn FnMut(&[f32], i32) -> bool| {
            if supports_streaming {
                engine.synthesize_streaming(text, speed, &voice, on_chunk)
            } else {
                // 不支持流式的后端（sherpa）：整段合成后单块回调，行为等价旧路径
                let samples = engine.synthesize(text, speed, &voice)?;
                on_chunk(&samples, engine.sample_rate());
                Ok(())
            }
        };
        Ok(Self::with(
            Box::new(synth),
            Box::new(|| {
                crate::voice::player::Speaker::try_new()
                    .map(|s| Box::new(s) as Box<dyn AudioPlayer>)
                    .map_err(|e| format!("打开音频输出设备失败: {e}"))
            }),
        ))
    }

    /// 请求播报一条文本；正在播/队列满则本条丢弃（返回 false）。
    pub fn announce(&self, text: &str) -> bool {
        self.tx.try_send(text.to_string()).is_ok()
    }
}

/// worker 主循环：串行消费队列文本——开设备 → 流式合成（逐块即播）→ 失败仅告警
/// → 播放排空有界等待。
fn run_worker(rx: Receiver<String>, synth: Box<SynthFn>, open_player: Box<OpenPlayerFn>) {
    while let Ok(text) = rx.recv() {
        let mut player = match open_player() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("dsh 播报无法打开音频输出设备，跳过语音: {e}");
                continue;
            }
        };
        let result = {
            let mut on_chunk = |chunk: &[f32], rate: i32| {
                player.play(chunk.to_vec(), rate as u32);
                true
            };
            synth(&text, &mut on_chunk)
        };
        if let Err(e) = result {
            tracing::warn!("dsh 播报合成失败（跳过语音，气泡不受影响）: {e}");
            continue;
        }
        // 阻塞到播完（worker 串行语义）；有界等待：设备中途失效（拔线/驱动错误）
        // 时 `drained()` 可能永不为 true，60s 后放弃本条并告警，避免 worker 永久
        // 卡死导致语音播报静默失效。
        let deadline = Instant::now() + Duration::from_secs(60);
        while !player.drained() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !player.drained() {
            tracing::warn!("dsh 播报等待播放结束超时，放弃本条");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;

    /// 记录 play 调用的假播放器（与真实 Speaker 的队列语义解耦，专注断言）。
    struct RecordingPlayer {
        plays: Arc<Mutex<Vec<(Vec<f32>, u32)>>>,
    }

    impl AudioPlayer for RecordingPlayer {
        fn play(&mut self, samples: Vec<f32>, sample_rate: u32) {
            self.plays.lock().unwrap().push((samples, sample_rate));
        }
        fn stop(&mut self) {}
        fn drained(&self) -> bool {
            true
        }
    }

    fn recorder() -> (Arc<Mutex<Vec<(Vec<f32>, u32)>>>, Box<OpenPlayerFn>) {
        let plays = Arc::new(Mutex::new(Vec::new()));
        let plays_for_player = plays.clone();
        (
            plays,
            Box::new(move || {
                Ok(Box::new(RecordingPlayer {
                    plays: plays_for_player.clone(),
                }) as Box<dyn AudioPlayer>)
            }),
        )
    }

    #[test]
    fn test_announce_streams_chunks_via_injected_closures() {
        let (synth_tx, synth_rx) = mpsc::channel::<String>();
        let synth_tx = std::sync::Mutex::new(synth_tx);
        let (plays, open_player) = recorder();
        let announcer = Announcer::with(
            Box::new(move |text, on_chunk| {
                synth_tx.lock().unwrap().send(text.to_string()).unwrap();
                // 两块流式产出：第二块到达时第一块已在播放（队列语义）
                on_chunk(&[0.1, 0.2], 24000);
                on_chunk(&[0.3], 24000);
                Ok(())
            }),
            open_player,
        );
        assert!(announcer.announce("你好"));
        assert_eq!(
            synth_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "你好"
        );
        let plays = plays.lock().unwrap().clone();
        assert_eq!(
            plays,
            vec![(vec![0.1, 0.2], 24000), (vec![0.3], 24000),],
            "流式块应逐块到达播放器"
        );
    }

    #[test]
    fn test_queue_cap_one_overwrites_drops_excess() {
        // 主线程先持闭锁；合成闭包阻塞在 gate2.lock() 上，制造确定性的「播报中」
        let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
        let gate2 = gate.clone();
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered2 = entered.clone();
        let (_plays, open_player) = recorder();
        let announcer = Announcer::with(
            Box::new(move |_, on_chunk| {
                entered2.store(true, std::sync::atomic::Ordering::SeqCst);
                on_chunk(&[0.0], 24000);
                let _g = gate2.lock().unwrap(); // 阻塞直到主线程放行 = 播报中
                Ok(())
            }),
            open_player,
        );
        // 主线程先持锁：worker 的合成闭包将阻塞在锁上，期间 worker 不会回到 recv
        let _gate_held = gate.lock().unwrap();
        assert!(announcer.announce("第一句"), "空闲时应受理");
        // 等合成闭包进入并阻塞在闭锁上
        while !entered.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // 队列容量 1：worker 正忙（播报中），第二条进队，第三条被丢弃
        assert!(announcer.announce("第二句"), "队列空位应受理");
        assert!(!announcer.announce("第三条"), "队列满应丢弃");
        // 放行合成，让 worker 正常消费
        drop(_gate_held);
    }

    #[test]
    fn test_synth_failure_skips_silently() {
        let (plays, open_player) = recorder();
        let announcer =
            Announcer::with(Box::new(|_, _| Err("模型未就绪".to_string())), open_player);
        assert!(announcer.announce("会失败"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(plays.lock().unwrap().is_empty(), "合成失败不应有任何播放");
        // worker 存活：下一条正常受理
        assert!(announcer.announce("再来"));
    }

    #[test]
    fn test_voice_slot_available() {
        use SessionState::*;
        // 会话未运行：随时可播
        assert!(voice_slot_available(false, None));
        assert!(voice_slot_available(false, Some(Speaking)));
        // 运行中：只有待唤醒空闲 / 停止迁出态可插
        assert!(voice_slot_available(true, Some(Armed)));
        assert!(voice_slot_available(true, Some(Idle)));
        // 运行中的忙态：一律等待（Listening 污染 ASR、Speaking 与 TTS 重叠）
        assert!(!voice_slot_available(true, Some(Listening)));
        assert!(!voice_slot_available(true, Some(Speaking)));
        assert!(!voice_slot_available(true, Some(Thinking)));
        assert!(!voice_slot_available(true, Some(Greeting)));
        assert!(!voice_slot_available(true, Some(WaitingSpeech)));
        // 宿主未上报状态（防御）：视为不可插
        assert!(!voice_slot_available(true, None));
    }
}
