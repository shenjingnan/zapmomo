/// dsh 事件语音播报：独立 worker 线程合成 + rodio 播放。
///
/// - 与 voice 会话的互斥由调用方（管线）判断「voice 会话是否运行」决定是否调用
/// - 自身防重叠：worker 串行播放；排队容量 1（`SyncSender::try_send`），溢出丢弃——
///   气泡已传达信息，语音只是增强
/// - 合成/播放函数可注入（测试无音频设备依赖）
use crate::tts;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

pub struct Announcer {
    tx: SyncSender<String>,
    // JoinHandle 非 Sync，用 Mutex 包一层使 Announcer 可跨线程共享（Tauri state 用）；
    // 不 join：drop Announcer -> tx 释放 -> worker 的 recv 出错自然退出（非阻塞）
    _handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Announcer {
    /// 注入合成/播放函数的构造（测试用）。
    ///
    /// `synth(text) -> (PCM 样本)`，`play(samples, sample_rate)` 阻塞到播完。
    pub fn with(
        synth: impl Fn(&str) -> Result<Vec<f32>, String> + Send + 'static,
        play: impl FnMut(Vec<f32>, u32) + Send + 'static,
        sample_rate: u32,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        // 命名线程便于日志定位（同 voice 合成线程的 voice-tts 命名惯例）
        let handle = std::thread::Builder::new()
            .name("dsh-announce".to_string())
            .spawn(move || run_worker(rx, synth, play, sample_rate))
            .expect("spawn dsh-announce 线程失败");
        Self {
            tx,
            _handle: Mutex::new(Some(handle)),
        }
    }

    /// 生产构造：sherpa TTS 合成 + rodio Speaker 播放（默认音色/语速）。
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
        let sample_rate = engine.sample_rate() as u32;
        let speed = cfg.speed;
        Ok(Self::with(
            move |text| engine.synthesize(text, speed, &voice),
            move |samples, rate| {
                if let Ok(mut speaker) = crate::voice::player::Speaker::try_new() {
                    use crate::voice::player::AudioPlayer;
                    speaker.play(samples, rate);
                    // 阻塞到播完（worker 串行语义）；有界等待：设备中途失效
                    // （拔线/驱动错误）时 `drained()` 可能永不为 true，60s 后放弃
                    // 本条并告警，避免 worker 永久卡死导致语音播报静默失效。
                    let deadline = Instant::now() + Duration::from_secs(60);
                    while !speaker.drained() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    if !speaker.drained() {
                        tracing::warn!("dsh 播报等待播放结束超时，放弃本条");
                    }
                } else {
                    tracing::warn!("dsh 播报无法打开音频输出设备，跳过语音");
                }
            },
            sample_rate,
        ))
    }

    /// 请求播报一条文本；正在播/队列满则本条丢弃（返回 false）。
    pub fn announce(&self, text: &str) -> bool {
        self.tx.try_send(text.to_string()).is_ok()
    }
}

/// worker 主循环：串行消费队列文本，合成成功后播放（阻塞到播完），失败仅告警。
fn run_worker(
    rx: Receiver<String>,
    synth: impl Fn(&str) -> Result<Vec<f32>, String>,
    mut play: impl FnMut(Vec<f32>, u32),
    sample_rate: u32,
) {
    while let Ok(text) = rx.recv() {
        match synth(&text) {
            Ok(samples) => play(samples, sample_rate),
            Err(e) => {
                tracing::warn!("dsh 播报合成失败（跳过语音，气泡不受影响）: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_announce_plays_via_injected_closures() {
        let (played_tx, played_rx) = mpsc::channel::<(Vec<f32>, u32)>();
        let (synth_tx, synth_rx) = mpsc::channel::<String>();
        let synth_tx = std::sync::Mutex::new(synth_tx);
        let announcer = Announcer::with(
            move |text| {
                synth_tx.lock().unwrap().send(text.to_string()).unwrap();
                Ok(vec![0.1, 0.2])
            },
            move |samples, rate| {
                played_tx.send((samples, rate)).unwrap();
            },
            24000,
        );
        assert!(announcer.announce("你好"));
        assert_eq!(
            synth_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "你好"
        );
        let (samples, rate) = played_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(samples, vec![0.1, 0.2]);
        assert_eq!(rate, 24000);
    }

    #[test]
    fn test_queue_cap_one_overwrites_drops_excess() {
        // 主线程先持闭锁；播放闭包阻塞在 gate2.lock() 上，制造确定性的「正在播报」
        let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
        let gate2 = gate.clone();
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entered2 = entered.clone();
        let announcer = Announcer::with(
            |_| Ok(vec![0.0]),
            move |_, _| {
                entered2.store(true, std::sync::atomic::Ordering::SeqCst);
                let _g = gate2.lock().unwrap(); // 阻塞直到主线程放行 = 播放中
            },
            24000,
        );
        // 主线程先持锁：worker 的播放闭包将阻塞在锁上，期间 worker 不会回到 recv
        let _gate_held = gate.lock().unwrap();
        assert!(announcer.announce("第一句"), "空闲时应受理");
        // 等播放闭包进入并阻塞在闭锁上
        while !entered.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        // 队列容量 1：worker 正忙（播放中），第二条进队，第三条被丢弃
        assert!(announcer.announce("第二句"), "队列空位应受理");
        assert!(!announcer.announce("第三句"), "队列满应丢弃");
        // 放行播放，让 worker 正常消费
        drop(_gate_held);
    }

    #[test]
    fn test_synth_failure_skips_silently() {
        let (played_tx, played_rx) = mpsc::channel::<(Vec<f32>, u32)>();
        let announcer = Announcer::with(
            |_| Err("模型未就绪".to_string()),
            move |samples, rate| {
                played_tx.send((samples, rate)).unwrap();
            },
            24000,
        );
        assert!(announcer.announce("会失败"));
        std::thread::sleep(Duration::from_millis(200));
        assert!(played_rx.try_recv().is_err(), "合成失败不应播放");
        // worker 存活：下一条正常受理
        assert!(announcer.announce("再来"));
    }
}
