/// LLM 模块（OpenAI 兼容远程 API）。
///
/// 分层：
/// - `LlmEngine`（门面）：生命周期 + worker 线程 + 命令/事件 channel，供 CLI/Tauri 使用。
/// - `LlmProvider`（trait）：后端抽象。
/// - `http`：`OpenAiChatProvider`，OpenAI 兼容 Chat Completions API（智谱 GLM /
///   DeepSeek / OpenRouter / llama-server 等）。
pub mod agent;
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
use tools::ToolRuntime;
use types::{FinishReason, GenParams, InputItem, OutputItem, TokenDelta};

/// LLM 引擎事件（worker 线程 → 调用方）。`Clone` 供广播给多个订阅者。
#[derive(Debug, Clone, PartialEq)]
pub enum LlmEvent {
    /// 一次文本增量
    Token(TokenDelta),
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
    pub fn load_blocking(&self, timeout: std::time::Duration) -> Result<(), String> {
        self.cmd_tx
            .send(LlmCommand::Load)
            .map_err(|_| "LLM worker 线程已退出".to_string())?;
        // 订阅自己的事件流（广播，不影响其它订阅者），只消费本次 Load 的 Status/Error
        let rx = self.subscribe();
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
/// 只支持 OpenAI 兼容 Chat Completions（"openai" / "llamacpp-server"）。
/// 本地 llama.cpp 推理已移除：需要本地模型请自行部署 Ollama / llama-server，
/// 经 OpenAI 兼容 API 接入。
pub fn create_provider(
    config: ResolvedLlmConfig,
) -> Result<Box<dyn provider::LlmProvider>, LlmError> {
    match config.provider.as_str() {
        "openai" | "llamacpp-server" => Ok(Box::new(http::OpenAiChatProvider::new(&config)?)),
        other => Err(LlmError::UnsupportedProvider(other.to_string())),
    }
}

/// 把事件广播给所有订阅者；发送失败（订阅者断开）的 Sender 移除。
fn broadcast_to(subs: &Arc<Mutex<Vec<Sender<LlmEvent>>>>, ev: &LlmEvent) {
    let mut subs = subs.lock().unwrap_or_else(|e| e.into_inner());
    subs.retain(|s| s.send(ev.clone()).is_ok());
}

/// worker 线程主循环：创建 provider 后处理命令，直到 `Shutdown` 或 channel 关闭。
fn worker_loop(
    config: ResolvedLlmConfig,
    cmd_rx: Receiver<LlmCommand>,
    subs: Arc<Mutex<Vec<Sender<LlmEvent>>>>,
    generating: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
) {
    let mut provider = match create_provider(config) {
        Ok(p) => p,
        Err(e) => {
            broadcast_to(&subs, &LlmEvent::Error(e.to_string()));
            return;
        }
    };
    let agent = Agent::new(ToolRuntime::new());

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
                let mut emit = |item: OutputItem| match item {
                    OutputItem::MessageDelta(delta) => {
                        broadcast_to(&subs, &LlmEvent::Token(delta));
                    }
                    OutputItem::ToolCall(_) => {
                        // 工具调用由 Agent Loop 内部处理，不外传（未来可发 llm-tool-call 事件）
                    }
                };
                let result = agent.run(&mut *provider, &input, &params, &mut emit, cancel);
                // 生成结束（含错误/取消）→ 释放互斥标志
                generating.store(false, Ordering::SeqCst);
                match result {
                    Ok(reason) => broadcast_to(&subs, &LlmEvent::Finished(reason)),
                    Err(e) => broadcast_to(&subs, &LlmEvent::Error(e.to_string())),
                }
            }
            LlmCommand::Shutdown => break,
        }
    }

    provider.unload();
    ready.store(false, Ordering::Relaxed);
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
}
