//! Agent Runtime：循环调用 provider，处理工具调用，直到产出纯文本回复。
//!
//! 数据流：`input` → provider.generate → 收集 ToolCall → 执行工具 → 回填
//! `ToolCall` + `ToolResult` → 再次 generate，直到无 ToolCall（纯文本回复）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::llm::error::LlmError;
use crate::llm::provider::LlmProvider;
use crate::llm::tools::ToolRuntime;
use crate::llm::types::{FinishReason, GenParams, InputItem, OutputItem, ToolResult};

/// 单次 Agent Loop 的最大工具调用轮数（防止模型反复请求工具导致死循环）。
const MAX_ROUNDS: usize = 10;

pub struct Agent {
    tool_runtime: ToolRuntime,
}

impl Agent {
    pub fn new(tool_runtime: ToolRuntime) -> Self {
        Self { tool_runtime }
    }

    /// 运行 Agent Loop：循环调用 provider，处理工具调用，直到纯文本回复。
    pub fn run(
        &self,
        provider: &mut dyn LlmProvider,
        input: &[InputItem],
        params: &GenParams,
        emit: &mut (dyn FnMut(OutputItem) + Send),
        cancel: Arc<AtomicBool>,
    ) -> Result<FinishReason, LlmError> {
        let tools = self.tool_runtime.definitions();
        let mut current: Vec<InputItem> = input.to_vec();

        for _ in 0..MAX_ROUNDS {
            // 一轮生成：文本增量转发给上层，工具调用收集
            let mut tool_calls = Vec::new();
            let result = provider.generate(
                &current,
                &tools,
                params,
                &mut |item| match item {
                    OutputItem::MessageDelta(delta) => {
                        emit(OutputItem::MessageDelta(delta));
                    }
                    OutputItem::ToolCall(call) => tool_calls.push(call),
                },
                cancel.clone(),
            )?;

            // 无工具调用 → 纯文本回复，结束
            if tool_calls.is_empty() {
                return Ok(result);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(FinishReason::Cancelled);
            }

            // 回填工具调用 + 工具结果，进入下一轮
            for call in tool_calls {
                let output = self.tool_runtime.execute(&call.name, &call.arguments)?;
                current.push(InputItem::ToolCall(call.clone()));
                current.push(InputItem::ToolResult(ToolResult {
                    id: call.id.clone().unwrap_or_default(),
                    name: call.name.clone(),
                    content: output,
                }));
            }
        }

        Err(LlmError::InferenceFailed(format!(
            "工具调用超过 {MAX_ROUNDS} 轮，已终止"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ChatMessage, ChatRole, TokenDelta, ToolCall};

    /// 模拟 provider：第一轮产出 tool call，回填后第二轮产出文本。
    struct MockProvider;

    impl LlmProvider for MockProvider {
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
            _tools: &[crate::llm::types::ToolDefinition],
            _params: &GenParams,
            emit: &mut (dyn FnMut(OutputItem) + Send),
            _cancel: Arc<AtomicBool>,
        ) -> Result<FinishReason, LlmError> {
            let has_tool_result = input.iter().any(|i| matches!(i, InputItem::ToolResult(_)));
            if has_tool_result {
                emit(OutputItem::MessageDelta(TokenDelta::new("答案是 42")));
            } else {
                emit(OutputItem::ToolCall(ToolCall {
                    name: "get_current_time".into(),
                    arguments: "{}".into(),
                    id: Some("call_1".into()),
                }));
            }
            Ok(FinishReason::Eos)
        }
    }

    #[test]
    fn test_agent_loop_executes_tool_then_returns_text() {
        let agent = Agent::new(ToolRuntime::new(false));
        let mut provider = MockProvider;
        let input = vec![InputItem::Message(ChatMessage::new(
            ChatRole::User,
            "现在几点？",
        ))];
        let mut text = String::new();
        let result = agent
            .run(
                &mut provider,
                &input,
                &GenParams::default(),
                &mut |item| {
                    if let OutputItem::MessageDelta(d) = item {
                        text.push_str(&d.text);
                    }
                },
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
        assert_eq!(result, FinishReason::Eos);
        // 第一轮 tool call → 执行工具 → 第二轮纯文本回复
        assert_eq!(text, "答案是 42");
    }
}
