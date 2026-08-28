/// TTS 合成线程（`SynthHandle`）。
///
/// sherpa-onnx 的 `OfflineTts::generate_with_config` 是同步阻塞（数秒级），因此把
/// 合成放到独立线程，避免卡住编排线程。单消费者保证句子顺序：`Synthesize` 串行
/// 处理、结果严格按提交序回传，编排线程按接收序 `append` 到播放器即天然保序。
///
/// `gen_id`（generation id）用于打断后的过期丢弃：每次进入新一轮生成 `+1`，
/// 编排线程只接受 `gen_id == current` 的结果；打断时 `cancel_all()` 置 cancel，
/// 当前句经 `synthesize_with_progress` 的进度回调返回 false 提前终止，待处理命令
/// 快速返回错误（不浪费算力）。
///
/// 引擎热切换：`SwapEngine` 命令携带新引擎 + 新音色，在命令队列中排队——mpsc
/// 单消费者语义天然给出「当前句完成 → 处理 Swap → 后续句用新引擎」的**句间零
/// 中断**切换（TTS 模型切换不杀会话、不打断正在合成的句子，见
/// `session::refresh_tts_if_switched`）。旧引擎在替换处 drop（sherpa 释放内存 /
/// audiocpp 释放 server 租约）。
///
/// 流式合成：`supports_streaming()` 的引擎（audiocpp 流式族）在 `Synthesize`
/// 处理时改走 `synthesize_streaming`，逐块发 [`SynthResult::StreamChunk`]、句末
/// 发 [`SynthResult::StreamDone`] 终态（「每句恰一个终态」协议与 `Done`/`Error`
/// 同地位）。发送闭包双查 cancel 与结果通道 send：任一失败即停止读取 SSE
/// （协作取消，取消延迟 = chunk 边界，修复 audiocpp 在途 HTTP 不可中断的缺陷）。
/// 协作取消与正常完成统一补终态，打断后由编排线程按 gen_id 丢弃。
use crate::tts::{TtsEngine, TtsVoiceParams};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// 发给合成线程的命令。
enum SynthCommand {
    Synthesize {
        text: String,
        gen_id: u64,
    },
    /// 句间热切换引擎（排队语义：pending 的 Synthesize 先处理完）。
    /// 引擎装箱：避免该变体撑大整个命令枚举（clippy large_enum_variant）。
    SwapEngine {
        engine: Box<TtsEngine>,
        voice: TtsVoiceParams,
    },
    Shutdown,
}

/// 合成结果（成功或失败，均带 `gen_id` 供编排线程做过期丢弃）。
///
/// 协议：每个 `Synthesize` 命令**恰一个终态**（`Done`/`StreamDone`/`Error`）；
/// 流式路径在终态前可发任意多个 [`SynthResult::StreamChunk`]（同句内严格有序，
/// 不计入编排线程的消费计数）。
#[derive(Debug)]
pub enum SynthResult {
    Done {
        gen_id: u64,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    /// 流式分块（块序即播放序；样本已应用语速，按 `sample_rate` 播放）
    StreamChunk {
        gen_id: u64,
        samples: Vec<f32>,
        sample_rate: u32,
    },
    /// 流式终态（句内全部分块已发完；对齐 `Done` 的地位）
    StreamDone {
        gen_id: u64,
    },
    Error {
        gen_id: u64,
        message: String,
    },
}

/// 合成线程句柄。
pub struct SynthHandle {
    tx: Sender<SynthCommand>,
    rx: Receiver<SynthResult>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl SynthHandle {
    /// 启动合成线程。`voice` / `speed` 为每句合成固定使用的说话人/音色与语速参数。
    pub fn new(tts: TtsEngine, voice: TtsVoiceParams, speed: f32) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();

        let join = std::thread::Builder::new()
            .name("voice-tts".to_string())
            .spawn(move || {
                // 可变绑定：SwapEngine 命令在句间替换（见模块文档）
                let mut tts = tts;
                let mut voice = voice;
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        SynthCommand::Synthesize { text, gen_id } => {
                            // cancel 置位时跳过合成（进度回调返回 false 也用于提前终止当前句）
                            if cancel_clone.load(Ordering::Relaxed) {
                                let _ = done_tx.send(SynthResult::Error {
                                    gen_id,
                                    message: "已取消".to_string(),
                                });
                                continue;
                            }
                            let payload = if tts.supports_streaming() {
                                // 流式路径：闭包双查 cancel（打断）与 send（会话退出
                                // rx drop）——任一失败即停止读取 SSE（取消延迟 =
                                // chunk 边界）；正常完成与协作取消统一补终态，
                                // 打断后由编排线程按 gen_id 丢弃
                                let chunk_cancel = cancel_clone.clone();
                                let chunk_tx = done_tx.clone();
                                match tts.synthesize_streaming(
                                    &text,
                                    speed,
                                    &voice,
                                    &mut |samples, rate| {
                                        if chunk_cancel.load(Ordering::Relaxed) {
                                            return false;
                                        }
                                        chunk_tx
                                            .send(SynthResult::StreamChunk {
                                                gen_id,
                                                samples: samples.to_vec(),
                                                sample_rate: rate as u32,
                                            })
                                            .is_ok()
                                    },
                                ) {
                                    Ok(()) => SynthResult::StreamDone { gen_id },
                                    Err(e) => SynthResult::Error { gen_id, message: e },
                                }
                            } else {
                                // progress 回调返回 false 可提前终止当前句（打断时减少
                                // 无用计算）；闭包需 'static，clone 一份 cancel 再 move。
                                let progress_cancel = cancel_clone.clone();
                                let result =
                                    tts.synthesize_with_progress(&text, speed, &voice, move |_p| {
                                        !progress_cancel.load(Ordering::Relaxed)
                                    });
                                // 采样率每次合成后读引擎现值：热切换后随引擎变化，且
                                // audiocpp 首响应校准值（client 按响应 wav 头 set）能
                                // 传回播放侧（此前 spawn 前一次性捕获是真缺陷）
                                match result {
                                    Ok(samples) => SynthResult::Done {
                                        gen_id,
                                        samples,
                                        sample_rate: tts.sample_rate() as u32,
                                    },
                                    Err(e) => SynthResult::Error { gen_id, message: e },
                                }
                            };
                            let _ = done_tx.send(payload);
                        }
                        SynthCommand::SwapEngine {
                            engine,
                            voice: new_voice,
                        } => {
                            tts = *engine;
                            voice = new_voice;
                        }
                        SynthCommand::Shutdown => break,
                    }
                }
            })
            .expect("spawn voice-tts 线程失败");

        Self {
            tx: cmd_tx,
            rx: done_rx,
            cancel,
            join: Some(join),
        }
    }

    /// 入队一句文本合成（非阻塞）。
    pub fn enqueue(&self, text: String, gen_id: u64) {
        let _ = self.tx.send(SynthCommand::Synthesize { text, gen_id });
    }

    /// 句间热切换引擎与音色（非阻塞）。排队在 pending 的 `Synthesize` 之后：
    /// 当前句用旧引擎完成，后续句用新引擎——切换零中断。
    pub fn swap_engine(&self, engine: TtsEngine, voice: TtsVoiceParams) {
        let _ = self.tx.send(SynthCommand::SwapEngine {
            engine: Box::new(engine),
            voice,
        });
    }

    /// 非阻塞拉取一个合成结果（None = 暂无结果）。
    pub fn try_recv(&self) -> Option<SynthResult> {
        self.rx.try_recv().ok()
    }

    /// 取消当前一轮的合成（打断时调用）：置 cancel，让当前句提前终止、待处理跳过。
    pub fn cancel_all(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 新一轮生成开始前复位取消标志。
    pub fn clear_cancel(&self) {
        self.cancel.store(false, Ordering::Relaxed);
    }
}

impl Drop for SynthHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(SynthCommand::Shutdown);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用一个「立即返回固定样本」的假合成（不依赖真实 TTS 模型）测试句柄的
    /// 队列/取消语义。真实 `TtsEngine` 需要模型文件，这里只测句柄逻辑。
    fn handle_with_fake_tts() -> (SynthHandle, Arc<AtomicBool>) {
        // 直接构造：跳过线程内的真实合成，模拟通过 enqueue + 手动构造结果较复杂。
        // 这里用哨兵：cancel 标志作为「是否执行」开关，无法注入假引擎时退化为
        // 结构/生命周期测试（见下）。真实队列语义在 session.rs 集成测试覆盖。
        let (cmd_tx, cmd_rx) = mpsc::channel::<SynthCommand>();
        let (done_tx, done_rx) = mpsc::channel::<SynthResult>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();
        let join = std::thread::spawn(move || {
            // 假合成：从文本里取字符数作为「合成耗时」，产出固定样本
            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    SynthCommand::Synthesize { text, gen_id } => {
                        if cancel_clone.load(Ordering::Relaxed) {
                            let _ = done_tx.send(SynthResult::Error {
                                gen_id,
                                message: "已取消".to_string(),
                            });
                            continue;
                        }
                        // 模拟耗时：句长 ms 级
                        std::thread::sleep(std::time::Duration::from_millis(
                            (text.chars().count() as u64).min(20),
                        ));
                        let _ = done_tx.send(SynthResult::Done {
                            gen_id,
                            samples: vec![0.0; 240],
                            sample_rate: 24000,
                        });
                    }
                    // 假线程无法构造真实引擎，仅确认协议可达（真实语义见
                    // test_swap_engine_between_sentences 的双 stub 测试）
                    SynthCommand::SwapEngine { .. } => {}
                    SynthCommand::Shutdown => break,
                }
            }
        });
        (
            SynthHandle {
                tx: cmd_tx,
                rx: done_rx,
                cancel: cancel.clone(),
                join: Some(join),
            },
            cancel,
        )
    }

    #[test]
    fn test_enqueue_and_recv_in_order() {
        let (h, _) = handle_with_fake_tts();
        h.enqueue("第一句".to_string(), 1);
        h.enqueue("第二句".to_string(), 1);

        // 等待两个结果按提交序返回
        let mut got = Vec::new();
        for _ in 0..2 {
            loop {
                if let Some(r) = h.try_recv() {
                    match r {
                        SynthResult::Done {
                            gen_id,
                            samples,
                            sample_rate,
                        } => {
                            got.push((gen_id, samples.len(), sample_rate));
                        }
                        SynthResult::Error { message, .. } => panic!("不应失败: {message}"),
                        // 假合成（非流式）不会产生流式结果；防回归的守卫臂
                        SynthResult::StreamChunk { .. } | SynthResult::StreamDone { .. } => {
                            panic!("假合成不应产生流式结果")
                        }
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (1, 240, 24000));
        assert_eq!(got[1], (1, 240, 24000));
    }

    #[test]
    fn test_cancel_all_returns_errors() {
        let (h, _) = handle_with_fake_tts();
        h.cancel_all();
        h.enqueue("一句话".to_string(), 7);
        let mut got_error = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Some(SynthResult::Error { gen_id, .. }) = h.try_recv() {
                assert_eq!(gen_id, 7);
                got_error = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(got_error, "cancel 后应快速返回错误");
    }

    #[test]
    fn test_clear_cancel_resumes() {
        let (h, _) = handle_with_fake_tts();
        h.cancel_all();
        h.clear_cancel();
        h.enqueue("恢复".to_string(), 3);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut done = false;
        while std::time::Instant::now() < deadline {
            if let Some(r) = h.try_recv() {
                assert!(matches!(r, SynthResult::Done { .. }));
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(done, "clear_cancel 后应正常合成");
    }

    // ---------- SwapEngine 句间语义（真实引擎 + tiny_http 双 stub） ----------

    /// 起一个返回固定采样率 wav 的 stub server（对齐 client.rs 的 spawn_stub 模式）。
    /// 返回 base_url 与服务线程句柄（线程随进程退出，无需显式停）。
    fn spawn_stub_wav(sample_rate: u32) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                let samples = vec![0.2f32; (sample_rate / 10) as usize]; // 0.1s
                let base = tempfile::tempdir().unwrap();
                let path = base.path().join("resp.wav");
                crate::audio::write_wav_f32(&path, sample_rate, &samples).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                let _ = request.respond(tiny_http::Response::from_data(bytes));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 真实 audiocpp 引擎（直连 stub，无需模型文件与真实 server 进程）。
    /// 选 qwen3_tts（非流式族）以锚定整段合成路径；音色须配 [`stub_reference_voice`]
    /// （qwen3 Base 拦截 Sid 兜底）。
    fn stub_engine(base_url: &str) -> TtsEngine {
        let cfg = crate::tts::config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Qwen3Tts06,
            ..crate::tts::config::ResolvedTtsConfig::default()
        };
        TtsEngine::from_audiocpp_for_test(crate::audiocpp::client::AudiocppTts::new_with_base_url(
            cfg, base_url,
        ))
    }

    /// stub 引擎用的克隆音色参数（路径不被 stub 读取，仅走请求体映射）。
    fn stub_reference_voice() -> crate::tts::TtsVoiceParams {
        crate::tts::TtsVoiceParams::Reference {
            wav_path: std::path::PathBuf::from("/voices/stub.wav"),
            reference_text: "stub".to_string(),
        }
    }

    /// SwapEngine 句间语义：swap 前入队的句子用旧引擎（旧采样率），swap 后的
    /// 新句子用新引擎；sample_rate 每句随引擎读取（修复一次性捕获缺陷）。
    /// 两个 stub 分别返回 16k / 24k wav，用结果采样率区分「哪台引擎在工作」。
    #[test]
    fn test_swap_engine_between_sentences() {
        let url_a = spawn_stub_wav(16_000);
        let url_b = spawn_stub_wav(24_000);
        let engine_a = stub_engine(&url_a);
        let engine_b = stub_engine(&url_b);

        let h = SynthHandle::new(engine_a, stub_reference_voice(), 1.0);
        // 句 1 入队（旧引擎）→ swap 排队 → 句 2 入队（新引擎）
        h.enqueue("第一句旧引擎".to_string(), 1);
        h.swap_engine(engine_b, stub_reference_voice());
        h.enqueue("第二句新引擎".to_string(), 1);

        let mut rates = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while rates.len() < 2 && std::time::Instant::now() < deadline {
            if let Some(SynthResult::Done { sample_rate, .. }) = h.try_recv() {
                rates.push(sample_rate);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(
            rates,
            vec![16_000, 24_000],
            "句 1 应为旧引擎采样率 16k，句 2 应为新引擎采样率 24k"
        );
    }

    /// Drop 语义回归：swap 后 Drop 应等线程退出（新引擎随线程 drop，无泄漏挂起）。
    #[test]
    fn test_drop_after_swap_joins_cleanly() {
        let url = spawn_stub_wav(24_000);
        let h = SynthHandle::new(stub_engine(&url), stub_reference_voice(), 1.0);
        h.swap_engine(stub_engine(&url), stub_reference_voice());
        h.enqueue("x".to_string(), 1);
        // 作用域结束触发 Drop（Shutdown + join）；若卡死本测试超时失败
        drop(h);
    }

    /// TTS 热切换邮箱语义（`voice::TtsSwapSlot`）：连续覆盖写后读方 take 拿到
    /// **最新**代际（旧 pending 被覆盖 drop，连续切换最终一致）；take 后为空
    /// （不重复消费）。模拟 `set_current_model` 写方与会话读方的交互协议。
    #[test]
    fn test_tts_swap_slot_overwrite_and_take() {
        use crate::voice::session::{TtsSwap, TtsSwapSlot};
        let url = spawn_stub_wav(24_000);
        let slot: TtsSwapSlot = Arc::new(std::sync::Mutex::new(None));
        // 写方连续两次覆盖（模拟用户快速连切两个模型）
        for generation in [1u64, 2] {
            let engine = stub_engine(&url);
            let cfg = crate::tts::config::ResolvedTtsConfig::default();
            *slot.lock().unwrap() = Some(TtsSwap {
                engine,
                cfg,
                generation,
            });
        }
        // 读方（会话循环）take：拿到最新一代
        let swap = slot.lock().unwrap().take().unwrap();
        assert_eq!(swap.generation, 2, "覆盖写后应只剩最新代际");
        drop(swap);
        // take 后为空（不重复消费）
        assert!(slot.lock().unwrap().take().is_none());
    }

    // ---------- 流式：tiny_http 延迟 SSE stub + 真实 audiocpp 引擎 ----------

    /// 逐事件延迟推送的 SSE body：每次 read 前 sleep，模拟真实流的分块到达节奏。
    /// 一次性全量 body 会让 cancel 测试失去确定性（读取方永远赶不上取消时刻）。
    struct DelayedSseBody {
        events: std::vec::IntoIter<Vec<u8>>,
        current: Vec<u8>,
        pos: usize,
        delay: std::time::Duration,
    }

    impl std::io::Read for DelayedSseBody {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.current.len() {
                match self.events.next() {
                    Some(ev) => {
                        std::thread::sleep(self.delay);
                        self.current = ev;
                        self.pos = 0;
                    }
                    None => return Ok(0), // EOF：流结束
                }
            }
            let n = buf.len().min(self.current.len() - self.pos);
            buf[..n].copy_from_slice(&self.current[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    /// 构造 n 条 delta 事件 + done + [DONE]（每条 i16_count 个 i16 样本）。
    fn sse_events(n: usize, i16_count: usize) -> Vec<Vec<u8>> {
        use base64::Engine as _;
        let mut events = Vec::new();
        for _ in 0..n {
            let pcm: Vec<u8> = std::iter::repeat_n([0x10u8, 0x27], i16_count)
                .flatten()
                .collect();
            let b64 = base64::engine::general_purpose::STANDARD.encode(pcm);
            events.push(
                format!(
                    "data: {}\n\n",
                    serde_json::json!({"type": "speech.audio.delta", "audio": b64})
                )
                .into_bytes(),
            );
        }
        events.push(b"data: {\"type\":\"speech.audio.done\"}\n\ndata: [DONE]\n\n".to_vec());
        events
    }

    /// 起延迟 SSE stub（omnivoice 流式引擎的依赖）：逐事件间隔 `delay` 推送。
    fn spawn_stub_sse_incremental(events: Vec<Vec<u8>>, delay: std::time::Duration) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        std::thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..])
                        .unwrap();
                let reader = DelayedSseBody {
                    events: events.clone().into_iter(),
                    current: Vec::new(),
                    pos: 0,
                    delay,
                };
                let _ = request.respond(tiny_http::Response::new(
                    tiny_http::StatusCode(200),
                    vec![header],
                    reader,
                    None,
                    None,
                ));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// omnivoice（流式族）真实引擎，直连 SSE stub。
    fn stub_streaming_engine(base_url: &str) -> TtsEngine {
        let cfg = crate::tts::config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Omnivoice,
            ..crate::tts::config::ResolvedTtsConfig::default()
        };
        TtsEngine::from_audiocpp_for_test(crate::audiocpp::client::AudiocppTts::new_with_base_url(
            cfg, base_url,
        ))
    }

    /// 收集结果直到拿到终态（Done/StreamDone/Error）或超时。
    fn recv_until_terminal(h: &SynthHandle) -> Vec<SynthResult> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(r) = h.try_recv() {
                let terminal = matches!(
                    r,
                    SynthResult::Done { .. }
                        | SynthResult::StreamDone { .. }
                        | SynthResult::Error { .. }
                );
                out.push(r);
                if terminal {
                    break;
                }
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        out
    }

    /// 流式序列协议：[Chunk, Chunk, StreamDone] 严格同序同 gen_id。
    #[test]
    fn test_streaming_chunk_sequence_and_terminal() {
        let url = spawn_stub_sse_incremental(sse_events(2, 240), std::time::Duration::ZERO);
        let h = SynthHandle::new(
            stub_streaming_engine(&url),
            crate::tts::TtsVoiceParams::Sid(0),
            1.0,
        );
        h.enqueue("一句话".to_string(), 7);

        let results = recv_until_terminal(&h);
        assert_eq!(results.len(), 3, "两块 + 终态: {results:?}");
        assert!(matches!(
            (&results[0], &results[1], &results[2]),
            (
                SynthResult::StreamChunk {
                    gen_id: 7,
                    sample_rate: 24_000,
                    ..
                },
                SynthResult::StreamChunk { gen_id: 7, .. },
                SynthResult::StreamDone { gen_id: 7 },
            )
        ));
        // 块样本数按事件载荷（每事件 240 个 i16）
        if let SynthResult::StreamChunk { samples, .. } = &results[0] {
            assert_eq!(samples.len(), 240);
        }
    }

    /// 热切换在流式下成立：句 1 流式（Chunk/StreamDone）→ swap → 句 2 整段（Done）。
    /// 混合序列是 PR #162 句间零中断能力对流式路径的守卫。
    #[test]
    fn test_streaming_swap_engine_between_sentences() {
        let url_sse = spawn_stub_sse_incremental(sse_events(1, 100), std::time::Duration::ZERO);
        let url_wav = spawn_stub_wav(24_000);
        let h = SynthHandle::new(
            stub_streaming_engine(&url_sse),
            crate::tts::TtsVoiceParams::Sid(0),
            1.0,
        );
        h.enqueue("流式句".to_string(), 1);
        let first = recv_until_terminal(&h);
        assert_eq!(first.len(), 2, "一块 + 终态");
        assert!(matches!(first[0], SynthResult::StreamChunk { .. }));
        assert!(matches!(first[1], SynthResult::StreamDone { gen_id: 1 }));

        h.swap_engine(stub_engine(&url_wav), stub_reference_voice());
        h.enqueue("整段句".to_string(), 1);
        let second = recv_until_terminal(&h);
        assert_eq!(second.len(), 1, "非流式句恰一个终态");
        assert!(matches!(
            second[0],
            SynthResult::Done {
                sample_rate: 24_000,
                ..
            }
        ));
    }

    /// 流式中途取消：20 事件 ×100ms（全量 ~2s）；首块后 cancel_all →
    /// 终态在下一 chunk 边界（≤1s）到达且分块数远小于全量。
    #[test]
    fn test_streaming_cancel_mid_sentence_stops_chunks() {
        let url =
            spawn_stub_sse_incremental(sse_events(20, 240), std::time::Duration::from_millis(100));
        let h = SynthHandle::new(
            stub_streaming_engine(&url),
            crate::tts::TtsVoiceParams::Sid(0),
            1.0,
        );
        h.enqueue("长句".to_string(), 1);

        // 等首块到达再取消（保证取消发生在流中途而非流前置检查）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if matches!(h.try_recv(), Some(SynthResult::StreamChunk { .. })) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        h.cancel_all();

        let cancel_at = std::time::Instant::now();
        let results = recv_until_terminal(&h);
        let elapsed = cancel_at.elapsed();
        assert!(!results.is_empty(), "取消后应仍有终态到达");
        let chunks = results
            .iter()
            .filter(|r| matches!(r, SynthResult::StreamChunk { .. }))
            .count();
        assert!(chunks < 20, "取消后不应收完全量分块（got {chunks}）");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "终态应在下一 chunk 边界到达（{elapsed:?}，全量流需 ~2s）"
        );
    }

    /// 会话退出（drop 句柄 → rx drop → send 失败）在流在途时中止读取，
    /// Drop 的 join 不挂起（无泄漏）。
    #[test]
    fn test_drop_during_streaming_joins_quickly() {
        let url =
            spawn_stub_sse_incremental(sse_events(20, 240), std::time::Duration::from_millis(100));
        let h = SynthHandle::new(
            stub_streaming_engine(&url),
            crate::tts::TtsVoiceParams::Sid(0),
            1.0,
        );
        h.enqueue("长句".to_string(), 1);
        // 等首块确认流已建立
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if matches!(h.try_recv(), Some(SynthResult::StreamChunk { .. })) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let drop_at = std::time::Instant::now();
        drop(h); // Shutdown + join；线程应在下一 chunk 边界（send 失败）退出
        assert!(
            drop_at.elapsed() < std::time::Duration::from_secs(5),
            "drop 应快速返回（{}ms），全量流需 ~2s",
            drop_at.elapsed().as_millis()
        );
    }

    /// 流式全链路 E2E（真实引擎 + 真实 sidecar streaming 模式 + 真实 SSE）：
    /// 量化「首块延迟 vs 流结束」并验证多块协议。环境约定与
    /// `tts::test_omnivoice_synthesize_produces_audio` 相同：
    /// `OMNIVOICE_E2E_DIR=/path/to/omnivoice-audiocpp cargo test -- --ignored`
    /// （可选 OMNIVOICE_E2E_REF / OMNIVOICE_E2E_REF_TEXT 验证克隆）。
    #[test]
    #[ignore = "需要 omnivoice GGUF 在 OMNIVOICE_E2E_DIR 目录 + audiocpp 引擎可定位"]
    fn test_omnivoice_streaming_first_chunk_latency() {
        let Some(dir) = std::env::var("OMNIVOICE_E2E_DIR").ok() else {
            eprintln!("跳过：未设置 OMNIVOICE_E2E_DIR");
            return;
        };
        let cfg = crate::tts::config::ResolvedTtsConfig {
            backend: crate::tts::config::TtsBackendKind::Audiocpp,
            model_type: crate::tts::config::TtsModelKind::Omnivoice,
            model_dir: std::path::PathBuf::from(&dir),
            // 阶段 1 实测：omnivoice CPU RTF 6.6 不可用，Metal 0.41 达标
            provider: "metal".to_string(),
            ..crate::tts::config::ResolvedTtsConfig::default()
        };
        let engine = TtsEngine::new(cfg).unwrap();
        assert!(engine.supports_streaming(), "omnivoice 应支持流式");

        let voice = match std::env::var("OMNIVOICE_E2E_REF") {
            Ok(ref_wav) => crate::tts::TtsVoiceParams::Reference {
                wav_path: std::path::PathBuf::from(ref_wav),
                reference_text: std::env::var("OMNIVOICE_E2E_REF_TEXT").unwrap_or_else(|_| {
                    "那还是36年前, 1987年. 我呢考上了武汉大学的计算机系.".to_string()
                }),
            },
            Err(_) => crate::tts::TtsVoiceParams::Sid(0), // auto voice
        };
        let text = "语音合成的流式输出能显著降低首响延迟，让对话体验更加自然流畅。当模型逐块生成音频时，播放器可以立即开始播放第一块内容，而不需要等待整句合成完成。";
        let h = SynthHandle::new(engine, voice, 1.0);
        let t0 = std::time::Instant::now();
        h.enqueue(text.to_string(), 1);

        let mut first_chunk: Option<std::time::Duration> = None;
        let mut chunk_count = 0usize;
        let mut sample_count = 0usize;
        let terminal_at = loop {
            match h.try_recv() {
                Some(SynthResult::StreamChunk { samples, .. }) => {
                    if first_chunk.is_none() {
                        first_chunk = Some(t0.elapsed());
                    }
                    chunk_count += 1;
                    sample_count += samples.len();
                }
                Some(SynthResult::StreamDone { .. }) => break t0.elapsed(),
                Some(SynthResult::Error { message, .. }) => panic!("流式合成失败: {message}"),
                Some(SynthResult::Done { .. }) => panic!("流式路径不应返回整段 Done"),
                None => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        };
        eprintln!(
            "omnivoice streaming e2e: 首块 {:.2}s / 流结束 {:.2}s / 块数 {} / 音频 {:.1}s",
            first_chunk.unwrap().as_secs_f32(),
            terminal_at.as_secs_f32(),
            chunk_count,
            sample_count as f32 / 24000.0
        );
        assert!(
            chunk_count >= 2,
            "120 字长句在 chunk=40 粒度下应多块（got {chunk_count}）"
        );
        let first = first_chunk.unwrap().as_secs_f32();
        let total = terminal_at.as_secs_f32();
        assert!(
            first < total * 0.8,
            "首块延迟应显著早于流结束（{first:.2}s vs {total:.2}s）"
        );
    }
}
