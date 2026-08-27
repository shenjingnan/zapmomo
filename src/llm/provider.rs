use std::sync::Arc;
/// LLM 后端统一抽象。
///
/// 本地 llama.cpp 只是其中一种实现，未来可扩展 Ollama / OpenAI 兼容 / 云端等 provider。
/// 第一版保持最小，不过度抽象。
///
/// 注意：trait **不加 `Send` 约束**——llama.cpp 的 `LlamaContext` 非线程安全，
/// provider 实例在专用 worker 线程内创建并使用，不跨线程移动。
use std::sync::atomic::AtomicBool;

use crate::llm::error::LlmError;
use crate::llm::types::{FinishReason, GenParams, InputItem, OutputItem, ToolDefinition};

pub trait LlmProvider {
    /// 模型是否已加载可用。
    fn is_ready(&self) -> bool;

    /// 加载模型（local 实现真正加载；云端实现可空操作返回 `Ok(())`）。
    fn load(&mut self) -> Result<(), LlmError>;

    /// 卸载模型并释放内存。
    fn unload(&mut self);

    /// 流式生成：逐 item 调用 `emit` 推送增量（文本 / 工具调用），最后返回结束原因。
    ///
    /// `input` 是有序的上下文项（消息 + 工具结果），`tools` 是可用的工具定义。
    /// `cancel` 置位后应尽快停止并返回 [`FinishReason::Cancelled`]。
    /// `emit` 要求 `Send`：调用方身处 tokio 运行时时，生成会切换到普通线程执行。
    fn generate(
        &mut self,
        input: &[InputItem],
        tools: &[ToolDefinition],
        params: &GenParams,
        emit: &mut (dyn FnMut(OutputItem) + Send),
        cancel: Arc<AtomicBool>,
    ) -> Result<FinishReason, LlmError>;
}
