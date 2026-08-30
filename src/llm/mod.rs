/// LLM 模块（远程 API）。
///
/// 分层：
/// - `LlmEngine`（门面）：生命周期 + worker 线程 + 命令/事件 channel，供 CLI/Tauri 使用。
/// - `LlmProvider`（trait）：后端抽象。
/// - `http`：`OpenAiChatProvider`，OpenAI 兼容 Chat Completions API（智谱 GLM /
///   DeepSeek / OpenRouter / llama-server 等）。
/// - `anthropic`：`AnthropicProvider`，Anthropic 原生 Messages API（基于 genai crate）。
pub mod agent;
pub mod anthropic;
pub mod config;
pub mod error;
pub mod http;
pub mod provider;
pub mod tools;
pub mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use agent::Agent;
use config::ResolvedLlmConfig;
use error::LlmError;
use provider::LlmProvider;
use tools::ToolRuntime;
use types::{FinishReason, GenParams, InputItem, OutputItem, TokenDelta};

/// LLM 引擎事件（worker 线程 → 调用方）。`Clone` 供广播给多个订阅者。
#[derive(Debug, Clone, PartialEq)]
pub enum LlmEvent {
    /// 一次文本增量
    Token(TokenDelta),
    /// 一次生成中回填的工具调用+结果（成组原子送达，供跨轮 history 回传模型）。
    /// **先于**同一次生成的 [`LlmEvent::Finished`]（worker 单线程串行 send，mpsc 保序）；
    /// 生成 `Err` 时不广播。
    ToolRound { items: Vec<InputItem> },
    /// 生成结束（含结束原因）
    Finished(FinishReason),
    /// 错误（含中文描述）
    Error(String),
    /// 加载/卸载后的状态变化
    Status { ready: bool },
}

enum LlmCommand {
    Load,
    Unload,
    Generate {
        input: Vec<InputItem>,
        params: GenParams,
        cancel: Arc<AtomicBool>,
    },
    Shutdown,
}

/// LLM 引擎门面。
///
/// 内部 spawn 一个专用 worker OS 线程持有 `Box<dyn LlmProvider>`，命令经 `cmd_tx`
/// 投递，结果经 `evt_rx` 流式返回。这与项目现有 `std::thread::spawn + mpsc +
/// Arc<AtomicBool>` 的模式（`src/kws/mod.rs`）一致。
pub struct LlmEngine {
    cmd_tx: Sender<LlmCommand>,
    /// 事件广播：每个订阅者一个 mpsc channel（`mpsc::Sender` 是 `Sync`，`LlmEngine` 保持
    /// `Send + Sync`，可被 `Arc` 跨线程共享；voice 与 GUI 各自 `subscribe()` 互不抢事件）。
    subscribers: Arc<Mutex<Vec<Sender<LlmEvent>>>>,
    /// 生成互斥标志（voice/chat 不能同时生成，统一引擎串行）
    generating: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
    ready: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl LlmEngine {
    pub fn new(config: ResolvedLlmConfig) -> Result<Self, LlmError> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let subscribers = Arc::new(Mutex::new(Vec::<Sender<LlmEvent>>::new()));
        let generating = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));

        let ready_clone = ready.clone();
        let subs_clone = subscribers.clone();
        let generating_clone = generating.clone();
        let handle = std::thread::Builder::new()
            .name("llm-worker".to_string())
            .spawn(move || worker_loop(config, cmd_rx, subs_clone, generating_clone, ready_clone))
            .map_err(|e| LlmError::BackendUnavailable(e.to_string()))?;

        Ok(Self {
            cmd_tx,
            subscribers,
            generating,
            handle: Mutex::new(Some(handle)),
            ready,
            cancel,
        })
    }

    /// 订阅事件流（每个调用方持独立 `Receiver`，互不抢事件；断开的订阅者自动清理）。
    pub fn subscribe(&self) -> Receiver<LlmEvent> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(tx);
        rx
    }

    /// 模型是否已加载。
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// 当前是否正在生成（生成互斥：voice 与 GUI 不能同时生成）。
    ///
    /// 供切换/卸载保护使用：仅当 voice 会话正在用 LLM 生成时才需要阻止切换，
    /// 空闲（待唤醒）时切换是安全的——voice 会从共享引擎槽感知新引擎。
    pub fn is_generating(&self) -> bool {
        self.generating.load(Ordering::Relaxed)
    }

    /// 阻塞等待加载完成（模型库切换事务用）。
    ///
    /// 入队 `Load` 后等待 worker 发出 `Status{ready:true}` 或 `Error`；`timeout` 只是
    /// 错误检测的安全网——即使超时，调用方 drop 本引擎时 `Drop` 会 `join` worker，
    /// 保证加载任务彻底结束、资源释放后才返回（见 `Drop` 注释）。
    ///
    /// **顺序敏感**：必须先订阅再发命令。远程 provider 的 `load()` 瞬时完成，worker
    /// 可能在调用方订阅建立前就广播 `Status`——后订阅会永久错过该事件，只能等
    /// `timeout` 兜底（曾致语音会话启动假死「LLM 模型加载超时」，启动事件风暴下
    /// 高发）。
    pub fn load_blocking(&self, timeout: std::time::Duration) -> Result<(), String> {
        // 订阅自己的事件流（广播，不影响其它订阅者），只消费本次 Load 的 Status/Error。
        // 必须先于 send：广播发生在 worker 处理 Load 之后，晚订阅 = 丢事件。
        let rx = self.subscribe();
        #[cfg(test)]
        load_blocking_interleave_delay();
        self.cmd_tx
            .send(LlmCommand::Load)
            .map_err(|_| "LLM worker 线程已退出".to_string())?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err("LLM 模型加载超时".to_string());
            }
            let remaining = deadline.saturating_duration_since(now);
            match rx.recv_timeout(remaining) {
                Ok(LlmEvent::Status { ready: true }) => return Ok(()),
                Ok(LlmEvent::Status { ready: false }) => {
                    return Err("LLM 模型加载失败".to_string());
                }
                Ok(LlmEvent::Error(e)) => return Err(e),
                Ok(_) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err("LLM 模型加载超时".to_string());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("LLM worker 线程已退出".to_string());
                }
            }
        }
    }

    /// 加载模型（异步：结果经 [`LlmEvent::Status`]/[`LlmEvent::Error`] 返回）。
    pub fn load(&self) -> Result<(), LlmError> {
        self.cmd_tx
            .send(LlmCommand::Load)
            .map_err(|_| LlmError::BackendUnavailable("LLM worker 线程已退出".to_string()))
    }

    /// 卸载模型并释放内存。
    pub fn unload(&self) -> Result<(), LlmError> {
        self.cmd_tx
            .send(LlmCommand::Unload)
            .map_err(|_| LlmError::BackendUnavailable("LLM worker 线程已退出".to_string()))
    }

    /// 发起一次流式生成（异步：结果经 [`LlmEvent::Token`] 返回）。
    ///
    /// **生成互斥**：统一引擎串行，voice 与 GUI 不能同时生成（第二个调用返回 [`LlmError::Busy`]）。
    pub fn generate(&self, input: Vec<InputItem>, params: GenParams) -> Result<(), LlmError> {
        if self.generating.swap(true, Ordering::SeqCst) {
            return Err(LlmError::Busy);
        }
        self.cancel.store(false, Ordering::Relaxed);
        let result = self
            .cmd_tx
            .send(LlmCommand::Generate {
                input,
                params,
                cancel: self.cancel.clone(),
            })
            .map_err(|_| LlmError::BackendUnavailable("LLM worker 线程已退出".to_string()));
        if result.is_err() {
            self.generating.store(false, Ordering::SeqCst);
        }
        result
    }

    /// 取消当前生成。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for LlmEngine {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(LlmCommand::Shutdown);
        if let Some(handle) = self.handle.lock().ok().and_then(|mut h| h.take()) {
            let _ = handle.join();
        }
    }
}

/// 根据配置创建 provider。
///
/// 支持 OpenAI 兼容 Chat Completions（"openai" / "llamacpp-server"）与 Anthropic
/// 原生 Messages API（"anthropic"）。
/// 本地 llama.cpp 推理已移除：需要本地模型请自行部署 Ollama / llama-server，
/// 经 OpenAI 兼容 API 接入。
pub fn create_provider(
    config: ResolvedLlmConfig,
) -> Result<Box<dyn provider::LlmProvider>, LlmError> {
    match config.provider.as_str() {
        "openai" | "llamacpp-server" => Ok(Box::new(http::OpenAiChatProvider::new(&config)?)),
        "anthropic" => Ok(Box::new(anthropic::AnthropicProvider::new(&config)?)),
        other => Err(LlmError::UnsupportedProvider(other.to_string())),
    }
}

/// 把事件广播给所有订阅者；发送失败（订阅者断开）的 Sender 移除。
fn broadcast_to(subs: &Arc<Mutex<Vec<Sender<LlmEvent>>>>, ev: &LlmEvent) {
    let mut subs = subs.lock().unwrap_or_else(|e| e.into_inner());
    subs.retain(|s| s.send(ev.clone()).is_ok());
}

/// 测试注入：在 `load_blocking` 的「发命令」与「订阅事件流」之间制造调度停顿，
/// 确定性复现 worker 先完成 Load 广播、订阅方错过事件的竞态。生产构建为空。
#[cfg(test)]
static LOAD_BLOCKING_INTERLEAVE_DELAY_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
fn load_blocking_interleave_delay() {
    let ms = LOAD_BLOCKING_INTERLEAVE_DELAY_MS.load(Ordering::Relaxed);
    if ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// worker 线程主循环：创建 provider 后处理命令，直到 `Shutdown` 或 channel 关闭。
fn worker_loop(
    config: ResolvedLlmConfig,
    cmd_rx: Receiver<LlmCommand>,
    subs: Arc<Mutex<Vec<Sender<LlmEvent>>>>,
    generating: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
) {
    let cli_tools = config.cli_tools;
    let sprite_tool = config.sprite_tool;
    let mut provider = match create_provider(config) {
        Ok(p) => p,
        Err(e) => {
            broadcast_to(&subs, &LlmEvent::Error(e.to_string()));
            return;
        }
    };
    let agent = Agent::new(ToolRuntime::new(cli_tools).with_sprite_tool(sprite_tool));

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            LlmCommand::Load => match provider.load() {
                Ok(()) => {
                    ready.store(true, Ordering::Relaxed);
                    broadcast_to(&subs, &LlmEvent::Status { ready: true });
                }
                Err(e) => {
                    broadcast_to(&subs, &LlmEvent::Error(e.to_string()));
                }
            },
            LlmCommand::Unload => {
                provider.unload();
                ready.store(false, Ordering::Relaxed);
                broadcast_to(&subs, &LlmEvent::Status { ready: false });
            }
            LlmCommand::Generate {
                input,
                params,
                cancel,
            } => {
                run_generate_and_broadcast(
                    &agent,
                    &mut *provider,
                    &subs,
                    &generating,
                    input,
                    params,
                    cancel,
                );
            }
            LlmCommand::Shutdown => break,
        }
    }

    provider.unload();
    ready.store(false, Ordering::Relaxed);
}

/// 执行一次生成并按序广播：`ToolRound`（如有）→ `Finished` / `Error`。
///
/// 独立成函数便于用 mock provider 单测（`worker_loop` 内部自建 provider，无法注入）。
/// 广播顺序依赖 worker 单线程串行 send + mpsc FIFO：订阅方（voice session）先收到
/// 工具轮、再收到结束事件，从而把两者一并写入跨轮 history（user → tools → assistant）。
fn run_generate_and_broadcast(
    agent: &Agent,
    provider: &mut dyn LlmProvider,
    subs: &Arc<Mutex<Vec<Sender<LlmEvent>>>>,
    generating: &AtomicBool,
    input: Vec<InputItem>,
    params: GenParams,
    cancel: Arc<AtomicBool>,
) {
    let mut emit = |item: OutputItem| match item {
        OutputItem::MessageDelta(delta) => {
            broadcast_to(subs, &LlmEvent::Token(delta));
        }
        OutputItem::ToolCall(_) => {
            // 工具调用由 Agent Loop 内部处理；完整工具轮经 LlmEvent::ToolRound 外传
        }
    };
    let result = agent.run(provider, &input, &params, &mut emit, cancel);
    // 生成结束（含错误/取消）→ 释放互斥标志
    generating.store(false, Ordering::SeqCst);
    match result {
        Ok(outcome) => {
            if !outcome.tool_items.is_empty() {
                broadcast_to(
                    subs,
                    &LlmEvent::ToolRound {
                        items: outcome.tool_items,
                    },
                );
            }
            broadcast_to(subs, &LlmEvent::Finished(outcome.reason));
        }
        Err(e) => broadcast_to(subs, &LlmEvent::Error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;
    use std::time::Duration;

    #[test]
    fn test_broadcast_to_all_subscribers() {
        let subs = Arc::new(Mutex::new(Vec::new()));
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        subs.lock().unwrap().push(tx1);
        subs.lock().unwrap().push(tx2);

        let ev = LlmEvent::Token(TokenDelta::new("你好"));
        broadcast_to(&subs, &ev);
        assert_eq!(rx1.try_recv().unwrap(), ev.clone());
        assert_eq!(rx2.try_recv().unwrap(), ev);

        // 断开订阅者被清理
        drop(rx1);
        broadcast_to(&subs, &LlmEvent::Status { ready: true });
        assert_eq!(subs.lock().unwrap().len(), 1);
    }

    /// 引擎测试用配置：OpenAI 兼容 provider 指向不可达地址（构造不发请求，仅建 client）。
    fn engine_test_config() -> crate::llm::config::ResolvedLlmConfig {
        crate::llm::config::ResolvedLlmConfig {
            enabled: true,
            cli_tools: false,
            sprite_tool: true,
            prompt_cache: true,
            thinking: false,
            reasoning_effort: None,
            provider: "openai".to_string(),
            system_prompt: String::new(),
            params: GenParams::default(),
            base_url: Some("http://127.0.0.1:1".to_string()),
            api_key: None,
            model: Some("test-model".to_string()),
        }
    }

    #[test]
    fn test_subscribe_each_receiver_gets_copy() {
        run_with_temp_home(|_| {
            let cfg = engine_test_config();
            let engine = LlmEngine::new(cfg).unwrap();
            let rx1 = engine.subscribe();
            let rx2 = engine.subscribe();
            // 触发一次事件（Unload 无条件广播 Status{ready:false}）
            engine.unload().unwrap();
            let e1 = rx1.recv_timeout(Duration::from_secs(2)).unwrap();
            let e2 = rx2.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(matches!(e1, LlmEvent::Status { ready: false }));
            assert_eq!(e1, e2);
        });
    }

    #[test]
    fn test_load_blocking_does_not_miss_instant_load_event() {
        run_with_temp_home(|_| {
            // 远程 provider 的 load() 瞬时完成（OpenAiChatProvider::load 直接 Ok）：
            // worker 在收到 Load 后立刻广播 Status。若调用方在订阅建立前先发命令
            // （启动风暴下的调度停顿，间隔以百毫秒计），广播会落在订阅之前，
            // load_blocking 只能干等超时——曾致语音会话「LLM 模型加载超时」假死。
            LOAD_BLOCKING_INTERLEAVE_DELAY_MS.store(300, Ordering::Relaxed);
            let engine = LlmEngine::new(engine_test_config()).unwrap();
            let result = engine.load_blocking(Duration::from_secs(2));
            LOAD_BLOCKING_INTERLEAVE_DELAY_MS.store(0, Ordering::Relaxed);
            assert!(result.is_ok(), "瞬时完成的 Load 事件不得被错过：{result:?}");
        });
    }

    #[test]
    fn test_generate_mutual_exclusion() {
        run_with_temp_home(|_| {
            let cfg = engine_test_config();
            let engine = LlmEngine::new(cfg).unwrap();
            // 模拟生成中：generating=true 时再 generate → Busy
            engine.generating.store(true, Ordering::SeqCst);
            let err = engine.generate(vec![], GenParams::default()).unwrap_err();
            assert!(matches!(err, LlmError::Busy));
            engine.generating.store(false, Ordering::SeqCst);
            // 空闲时可发起（入队成功）
            assert!(engine.generate(vec![], GenParams::default()).is_ok());
        });
    }

    // ---------- ToolRound 广播（run_generate_and_broadcast） ----------

    use crate::llm::types::{ChatMessage, ChatRole, ToolCall, ToolDefinition};

    /// mock provider：第一轮产出 tool call，回填后第二轮产出纯文本
    /// （按 input 是否含 ToolResult 区分轮次）。
    struct ToolThenText;

    /// mock provider：第一轮产出 tool call，第二轮返回 Err（模拟生成失败）。
    struct ToolThenErr;

    /// 共享的单轮脚本：非回填轮 emit tool call，回填轮按 `err` 决定纯文本或 Err。
    fn scripted_generate(
        input: &[InputItem],
        emit: &mut (dyn FnMut(OutputItem) + Send),
        err: bool,
    ) -> Result<FinishReason, LlmError> {
        let has_tool_result = input.iter().any(|i| matches!(i, InputItem::ToolResult(_)));
        if has_tool_result {
            if err {
                return Err(LlmError::InferenceFailed("mock 第二轮失败".into()));
            }
            emit(OutputItem::MessageDelta(TokenDelta::new("done")));
        } else {
            emit(OutputItem::ToolCall(ToolCall {
                name: "get_current_time".into(),
                arguments: "{}".into(),
                id: Some("call_1".into()),
            }));
        }
        Ok(FinishReason::Eos)
    }

    impl LlmProvider for ToolThenText {
        fn is_ready(&self) -> bool {
            true
        }
        fn load(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        fn unload(&mut self) {}
        fn generate(
            &mut self,
            input: &[InputItem],
            _tools: &[ToolDefinition],
            _params: &GenParams,
            emit: &mut (dyn FnMut(OutputItem) + Send),
            _cancel: Arc<AtomicBool>,
        ) -> Result<FinishReason, LlmError> {
            scripted_generate(input, emit, false)
        }
    }

    impl LlmProvider for ToolThenErr {
        fn is_ready(&self) -> bool {
            true
        }
        fn load(&mut self) -> Result<(), LlmError> {
            Ok(())
        }
        fn unload(&mut self) {}
        fn generate(
            &mut self,
            input: &[InputItem],
            _tools: &[ToolDefinition],
            _params: &GenParams,
            emit: &mut (dyn FnMut(OutputItem) + Send),
            _cancel: Arc<AtomicBool>,
        ) -> Result<FinishReason, LlmError> {
            scripted_generate(input, emit, true)
        }
    }

    /// 两个订阅者的 subs + generating 标志。
    fn test_subs() -> (
        Arc<Mutex<Vec<Sender<LlmEvent>>>>,
        Receiver<LlmEvent>,
        Receiver<LlmEvent>,
        AtomicBool,
    ) {
        let subs = Arc::new(Mutex::new(Vec::<Sender<LlmEvent>>::new()));
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        subs.lock().unwrap().push(tx1);
        subs.lock().unwrap().push(tx2);
        (subs, rx1, rx2, AtomicBool::new(false))
    }

    #[test]
    fn test_generate_broadcasts_tool_round_before_finished() {
        // HOME 隔离：definitions() 会探测角色包 sprites 工具（磁盘 IO），测试需确定性
        run_with_temp_home(|_| {
            let (subs, rx1, rx2, generating) = test_subs();
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = ToolThenText;
            run_generate_and_broadcast(
                &agent,
                &mut provider,
                &subs,
                &generating,
                vec![InputItem::Message(ChatMessage::new(
                    ChatRole::User,
                    "现在几点？",
                ))],
                GenParams::default(),
                Arc::new(AtomicBool::new(false)),
            );
            assert!(!generating.load(Ordering::SeqCst), "结束后应释放互斥标志");
            for rx in [&rx1, &rx2] {
                // 顺序契约：第二轮文本增量流式透传（Token）→ ToolRound → Finished；
                // ToolRound 必须先于 Finished（订阅方在 Finished 分支一并入史）
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                    LlmEvent::Token(TokenDelta::new("done")),
                    "回填轮的文本增量应先流出"
                );
                match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                    LlmEvent::ToolRound { items } => {
                        assert_eq!(items.len(), 2, "call + result 成对");
                        assert!(
                            matches!(&items[0], InputItem::ToolCall(c) if c.name == "get_current_time")
                        );
                        assert!(matches!(&items[1], InputItem::ToolResult(t) if t.id == "call_1"));
                    }
                    other => panic!("第二个事件应为 ToolRound，实际：{other:?}"),
                }
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                    LlmEvent::Finished(FinishReason::Eos)
                );
            }
        });
    }

    #[test]
    fn test_generate_error_does_not_broadcast_tool_round() {
        run_with_temp_home(|_| {
            let (subs, rx1, _rx2, generating) = test_subs();
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = ToolThenErr;
            run_generate_and_broadcast(
                &agent,
                &mut provider,
                &subs,
                &generating,
                vec![InputItem::Message(ChatMessage::new(
                    ChatRole::User,
                    "现在几点？",
                ))],
                GenParams::default(),
                Arc::new(AtomicBool::new(false)),
            );
            // Err 路径：只有 Error，无 ToolRound、无 Finished
            assert!(matches!(
                rx1.recv_timeout(Duration::from_secs(2)).unwrap(),
                LlmEvent::Error(_)
            ));
            assert!(
                rx1.recv_timeout(Duration::from_millis(100)).is_err(),
                "不应有额外事件"
            );
        });
    }

    #[test]
    fn test_llm_event_tool_round_eq_and_clone() {
        let item = || {
            InputItem::ToolCall(ToolCall {
                name: "t".into(),
                arguments: "{}".into(),
                id: Some("1".into()),
            })
        };
        let a = LlmEvent::ToolRound {
            items: vec![item()],
        };
        assert_eq!(a, a.clone(), "Clone 后应相等（LlmEvent 的 PartialEq 契约）");
        let b = LlmEvent::ToolRound { items: vec![] };
        assert_ne!(a, b);
    }
}
