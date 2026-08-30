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

/// 一次 Agent Loop 的产出：结束原因 + 本轮回填的工具条目。
///
/// `tool_items` 供调用方写入跨轮 history，下一轮随构建输入回传给模型——
/// 否则模型不记得上一轮调过什么工具（如 `set_character_sprite` 切了什么形象）。
/// 成对且按回填顺序：`[ToolCall, ToolResult, ToolCall, ToolResult, ...]`；
/// 纯文本轮为空。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcome {
    pub reason: FinishReason,
    pub tool_items: Vec<InputItem>,
}

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
    ) -> Result<AgentOutcome, LlmError> {
        let tools = self.tool_runtime.definitions();
        let mut current: Vec<InputItem> = input.to_vec();
        let mut tool_items: Vec<InputItem> = Vec::new();

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
                return Ok(AgentOutcome {
                    reason: result,
                    tool_items,
                });
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(AgentOutcome {
                    reason: FinishReason::Cancelled,
                    tool_items,
                });
            }

            // 回填工具调用 + 工具结果，进入下一轮（同时累积给 AgentOutcome）
            for call in tool_calls {
                let output = self.tool_runtime.execute(&call.name, &call.arguments)?;
                let tool_result = ToolResult {
                    id: call.id.clone().unwrap_or_default(),
                    name: call.name.clone(),
                    content: output,
                };
                tool_items.push(InputItem::ToolCall(call.clone()));
                tool_items.push(InputItem::ToolResult(tool_result.clone()));
                current.push(InputItem::ToolCall(call));
                current.push(InputItem::ToolResult(tool_result));
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

    /// 每轮脚本动作：产出若干 tool call 或一段纯文本。
    enum MockTurn {
        ToolCalls(Vec<ToolCall>),
        Text(&'static str),
    }

    /// 模拟 provider：按脚本逐轮产出，并记录每轮收到的 input 供断言回填。
    struct MockProvider {
        turns: Vec<MockTurn>,
        /// 每轮收到的 input 快照（与 turns 同一轮次对齐）
        seen_inputs: Vec<Vec<InputItem>>,
    }

    impl MockProvider {
        fn new(turns: Vec<MockTurn>) -> Self {
            Self {
                turns,
                seen_inputs: Vec::new(),
            }
        }

        /// 第 n 轮收到的 input（0 起）。
        fn seen_input(&self, round: usize) -> Vec<InputItem> {
            self.seen_inputs[round].clone()
        }
    }

    fn tool_call(name: &str, id: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            arguments: "{}".into(),
            id: Some(id.to_string()),
        }
    }

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
            let round = self.seen_inputs.len();
            self.seen_inputs.push(input.to_vec());
            match &self.turns[round] {
                MockTurn::Text(text) => {
                    emit(OutputItem::MessageDelta(TokenDelta::new(*text)));
                }
                MockTurn::ToolCalls(calls) => {
                    for call in calls {
                        emit(OutputItem::ToolCall(call.clone()));
                    }
                }
            }
            Ok(FinishReason::Eos)
        }
    }

    /// 驱动一次 Agent::run，返回 (outcome, 捕获的文本增量)。
    fn run_agent(
        agent: &Agent,
        provider: &mut MockProvider,
        input: &[InputItem],
        cancel: Arc<AtomicBool>,
    ) -> (AgentOutcome, String) {
        let mut text = String::new();
        let outcome = agent
            .run(
                provider,
                input,
                &GenParams::default(),
                &mut |item| {
                    if let OutputItem::MessageDelta(d) = item {
                        text.push_str(&d.text);
                    }
                },
                cancel,
            )
            .unwrap();
        (outcome, text)
    }

    fn user_msg(content: &str) -> InputItem {
        InputItem::Message(ChatMessage::new(ChatRole::User, content))
    }

    #[test]
    fn test_agent_loop_executes_tool_then_returns_text() {
        // HOME 隔离：definitions() 会探测角色包 sprites 工具（磁盘 IO），测试需确定性
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![
                MockTurn::ToolCalls(vec![tool_call("get_current_time", "call_1")]),
                MockTurn::Text("答案是 42"),
            ]);
            let input = vec![user_msg("现在几点？")];
            let (outcome, text) = run_agent(&agent, &mut provider, &input, cancel_flag(false));
            assert_eq!(outcome.reason, FinishReason::Eos);
            // 第一轮 tool call → 执行工具 → 第二轮纯文本回复
            assert_eq!(text, "答案是 42");
            // 产出的工具条目：成对且按回填顺序
            assert_eq!(outcome.tool_items.len(), 2);
            assert_eq!(
                outcome.tool_items[0],
                InputItem::ToolCall(tool_call("get_current_time", "call_1"))
            );
            match &outcome.tool_items[1] {
                InputItem::ToolResult(t) => {
                    assert_eq!(t.id, "call_1");
                    assert_eq!(t.name, "get_current_time");
                    assert!(!t.content.is_empty());
                }
                other => panic!("第二条应为 ToolResult，实际：{other:?}"),
            }
        });
    }

    #[test]
    fn test_agent_round_plain_text_returns_empty_tool_items() {
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![MockTurn::Text("你好呀")]);
            let input = vec![user_msg("打个招呼")];
            let (outcome, text) = run_agent(&agent, &mut provider, &input, cancel_flag(false));
            assert_eq!(outcome.reason, FinishReason::Eos);
            assert_eq!(text, "你好呀");
            assert!(outcome.tool_items.is_empty());
        });
    }

    #[test]
    fn test_agent_round_multi_tool_calls_all_returned() {
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![
                MockTurn::ToolCalls(vec![
                    tool_call("get_current_time", "call_1"),
                    tool_call("get_current_time", "call_2"),
                ]),
                MockTurn::Text("好了"),
            ]);
            let input = vec![user_msg("几点了？顺便再看一次")];
            let (outcome, _) = run_agent(&agent, &mut provider, &input, cancel_flag(false));
            // 两个 call 各产出一对，顺序 [call, result, call, result]
            assert_eq!(outcome.tool_items.len(), 4);
            assert!(matches!(outcome.tool_items[0], InputItem::ToolCall(_)));
            assert!(matches!(outcome.tool_items[1], InputItem::ToolResult(_)));
            assert!(matches!(outcome.tool_items[2], InputItem::ToolCall(_)));
            assert!(matches!(outcome.tool_items[3], InputItem::ToolResult(_)));
        });
    }

    #[test]
    fn test_agent_round_cancelled_returns_no_new_items() {
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![MockTurn::ToolCalls(vec![tool_call(
                "get_current_time",
                "call_1",
            )])]);
            let input = vec![user_msg("现在几点？")];
            // cancel 预置为 true：第一轮产出 tool call 后、执行工具前即取消
            let (outcome, _) = run_agent(&agent, &mut provider, &input, cancel_flag(true));
            assert_eq!(outcome.reason, FinishReason::Cancelled);
            assert!(outcome.tool_items.is_empty());
        });
    }

    #[test]
    fn test_agent_round_unknown_tool_is_err() {
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![MockTurn::ToolCalls(vec![tool_call(
                "no_such_tool",
                "call_x",
            )])]);
            let input = vec![user_msg("调用一个不存在的工具")];
            let mut text = String::new();
            let result = agent.run(
                &mut provider,
                &input,
                &GenParams::default(),
                &mut |item| {
                    if let OutputItem::MessageDelta(d) = item {
                        text.push_str(&d.text);
                    }
                },
                cancel_flag(false),
            );
            assert!(result.is_err(), "未知工具应 Err 中断");
        });
    }

    #[test]
    fn test_agent_feeds_tool_items_back_to_provider() {
        crate::test_util::run_with_temp_home(|_home| {
            let agent = Agent::new(ToolRuntime::new(false));
            let mut provider = MockProvider::new(vec![
                MockTurn::ToolCalls(vec![tool_call("get_current_time", "call_1")]),
                MockTurn::Text("答案是 42"),
            ]);
            let input = vec![user_msg("现在几点？")];
            let _ = run_agent(&agent, &mut provider, &input, cancel_flag(false));
            // 第二轮 input 末尾应含上一轮回填的 ToolCall + ToolResult
            let second = provider.seen_input(1);
            let n = second.len();
            assert!(n >= 2);
            assert_eq!(
                second[n - 2],
                InputItem::ToolCall(tool_call("get_current_time", "call_1"))
            );
            assert!(matches!(&second[n - 1], InputItem::ToolResult(t) if t.id == "call_1"));
        });
    }

    /// 便捷构造 cancel 标志。
    fn cancel_flag(canceled: bool) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(canceled))
    }
}
