/// OpenAI 兼容 Chat Completions API 的 HTTP provider（基于 async-openai）。
///
/// 通过 `POST {base_url}/chat/completions` 流式调用（SSE 由 SDK 解析）。
/// 可用于智谱 GLM（open.bigmodel.cn）、DeepSeek、OpenRouter，或任何兼容
/// `/v1/chat/completions` 的 server（如 llama.cpp `llama-server`）。
///
/// 注意：Responses API（`/v1/responses`）是 OpenAI 专有协议，第三方平台普遍不支持
/// （智谱实测 404），因此这里走生态通用的 Chat Completions。
///
/// 同步/异步桥接：provider 内部持有 current_thread tokio runtime，`generate` 里
/// `block_on` 驱动流式请求。调用方已身处 tokio 运行时（如 CLI 的 `#[tokio::main]`）时，
/// 就地 `block_on` / drop runtime 都会 panic，故 `generate` 切普通线程执行、
/// `Drop` 把 runtime 移交普通线程析构（相应地 `emit` 要求 `Send`）。
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequestArgs, FinishReason as ApiFinishReason, FunctionCall, FunctionObject,
};
use futures_util::StreamExt;

use crate::llm::config::ResolvedLlmConfig;
use crate::llm::error::LlmError;
use crate::llm::provider::LlmProvider;
use crate::llm::types::{
    ChatRole, FinishReason, GenParams, InputItem, OutputItem, TokenDelta, ToolCall, ToolDefinition,
};

pub struct OpenAiChatProvider {
    client: Client<OpenAIConfig>,
    model: String,
    system_prompt: String,
    /// 自建单线程 runtime；Drop 时 take 成 None，以便在 async 上下文中移交普通线程析构。
    runtime: Option<tokio::runtime::Runtime>,
}

impl OpenAiChatProvider {
    pub fn new(config: &ResolvedLlmConfig) -> Result<Self, LlmError> {
        let base_url = config
            .base_url
            .clone()
            .ok_or_else(|| LlmError::BackendUnavailable("未配置 base_url".to_string()))?;
        let model = config
            .model
            .clone()
            .ok_or_else(|| LlmError::BackendUnavailable("未配置模型名（model）".to_string()))?;
        let openai_config = OpenAIConfig::new()
            .with_api_base(base_url)
            // 无 key 的本地 server（llama-server）传空串即可，服务端忽略 Authorization
            .with_api_key(config.api_key.clone().unwrap_or_default());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| LlmError::BackendUnavailable(format!("创建 tokio runtime 失败：{e}")))?;
        Ok(Self {
            client: Client::with_config(openai_config),
            model,
            system_prompt: config.system_prompt.clone(),
            runtime: Some(runtime),
        })
    }

    /// 把 `InputItem` 列表转成 Chat Completions 的 `messages`。
    ///
    /// 若调用方未提供 system 消息，则在开头注入配置的 system prompt。
    pub fn build_messages(
        &self,
        input: &[InputItem],
    ) -> Result<Vec<ChatCompletionRequestMessage>, LlmError> {
        let has_system = input
            .iter()
            .any(|item| matches!(item, InputItem::Message(m) if m.role == ChatRole::System));
        let mut messages: Vec<ChatCompletionRequestMessage> = Vec::with_capacity(input.len() + 1);
        if !has_system {
            messages.push(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(self.system_prompt.as_str())
                    .build()
                    .map_err(|e| LlmError::InferenceFailed(e.to_string()))?
                    .into(),
            );
        }
        for item in input {
            messages.push(Self::to_message(item)?);
        }
        Ok(messages)
    }

    fn to_message(item: &InputItem) -> Result<ChatCompletionRequestMessage, LlmError> {
        let build_err =
            |e: async_openai::error::OpenAIError| LlmError::InferenceFailed(e.to_string());
        Ok(match item {
            InputItem::Message(m) => match m.role {
                ChatRole::System => ChatCompletionRequestSystemMessageArgs::default()
                    .content(m.content.as_str())
                    .build()
                    .map_err(build_err)?
                    .into(),
                ChatRole::User => ChatCompletionRequestUserMessageArgs::default()
                    .content(m.content.as_str())
                    .build()
                    .map_err(build_err)?
                    .into(),
                ChatRole::Assistant => ChatCompletionRequestAssistantMessageArgs::default()
                    .content(m.content.as_str())
                    .build()
                    .map_err(build_err)?
                    .into(),
                // 工具结果正常走 InputItem::ToolResult；裸 Tool 角色消息按 user 处理，
                // 避免构造缺少 tool_call_id 的非法 tool 消息。
                ChatRole::Tool => ChatCompletionRequestUserMessageArgs::default()
                    .content(m.content.as_str())
                    .build()
                    .map_err(build_err)?
                    .into(),
            },
            InputItem::ToolCall(c) => ChatCompletionRequestAssistantMessageArgs::default()
                .tool_calls(vec![ChatCompletionMessageToolCalls::Function(
                    ChatCompletionMessageToolCall {
                        id: c.id.clone().unwrap_or_default(),
                        function: FunctionCall {
                            name: c.name.clone(),
                            arguments: c.arguments.clone(),
                        },
                    },
                )])
                .build()
                .map_err(build_err)?
                .into(),
            InputItem::ToolResult(t) => ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(t.id.as_str())
                .content(t.content.as_str())
                .build()
                .map_err(build_err)?
                .into(),
        })
    }

    /// 把 `ToolDefinition` 转成 Chat Completions 的 `tools`。
    pub fn to_tools(tools: &[ToolDefinition]) -> Vec<ChatCompletionTools> {
        tools
            .iter()
            .map(|t| {
                ChatCompletionTools::Function(ChatCompletionTool {
                    function: FunctionObject {
                        name: t.name.clone(),
                        description: Some(t.description.clone()),
                        parameters: Some(t.parameters.clone()),
                        strict: None,
                    },
                })
            })
            .collect()
    }

    /// 流式生成主循环（async 部分）：逐 chunk  emit 文本增量、按 index 合并 tool call 碎片。
    async fn generate_stream(
        &self,
        messages: Vec<ChatCompletionRequestMessage>,
        tools: &[ToolDefinition],
        params: &GenParams,
        emit: &mut (dyn FnMut(OutputItem) + Send),
        cancel: &AtomicBool,
    ) -> Result<FinishReason, LlmError> {
        let mut args = CreateChatCompletionRequestArgs::default();
        args.model(&self.model)
            .messages(messages)
            // top_k/min_p/repeat_penalty/seed 仅部分服务端支持（如 llama-server），
            // 标准 OpenAI Chat Completions 无对应项，这里不发
            .max_tokens(params.max_tokens as u32)
            .temperature(params.temperature)
            .top_p(params.top_p);
        if !tools.is_empty() {
            args.tools(Self::to_tools(tools));
        }
        let request = args
            .build()
            .map_err(|e| LlmError::InferenceFailed(e.to_string()))?;

        let mut stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| LlmError::BackendUnavailable(format!("chat completions 请求失败：{e}")))?;

        // tool call 累加器：按 chunk.index 合并（id, name, arguments）
        let mut tool_acc: Vec<(Option<String>, String, String)> = Vec::new();
        let mut finish = FinishReason::Eos;
        while let Some(item) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                return Ok(FinishReason::Cancelled);
            }
            let chunk =
                item.map_err(|e| LlmError::InferenceFailed(format!("流式解析失败：{e}")))?;
            for choice in chunk.choices {
                let delta = choice.delta;
                // reasoning_content（智谱思考链）等非标准扩展字段已被 SDK 反序列化丢弃
                if let Some(content) = delta.content.filter(|s| !s.is_empty()) {
                    emit(OutputItem::MessageDelta(TokenDelta::new(content)));
                }
                if let Some(tool_calls) = delta.tool_calls {
                    for tc in tool_calls {
                        let index = tc.index as usize;
                        while tool_acc.len() <= index {
                            tool_acc.push((None, String::new(), String::new()));
                        }
                        let acc = &mut tool_acc[index];
                        if tc.id.is_some() {
                            acc.0 = tc.id;
                        }
                        if let Some(f) = tc.function {
                            if let Some(name) = f.name {
                                acc.1 = name;
                            }
                            if let Some(args) = f.arguments {
                                acc.2.push_str(&args);
                            }
                        }
                    }
                }
                if let Some(reason) = choice.finish_reason {
                    finish = match reason {
                        ApiFinishReason::Stop
                        | ApiFinishReason::ToolCalls
                        | ApiFinishReason::FunctionCall => FinishReason::Eos,
                        ApiFinishReason::Length => FinishReason::MaxTokens,
                        ApiFinishReason::ContentFilter => {
                            return Err(LlmError::InferenceFailed(
                                "内容被服务端安全过滤拦截".to_string(),
                            ));
                        }
                    };
                }
            }
        }

        // 流结束后统一 emit 合并完成的 tool call
        for (id, name, arguments) in tool_acc {
            emit(OutputItem::ToolCall(ToolCall {
                name,
                arguments,
                id,
            }));
        }
        Ok(finish)
    }
}

impl LlmProvider for OpenAiChatProvider {
    fn is_ready(&self) -> bool {
        // 远程服务无本地加载态，构造成功即可用
        true
    }

    fn load(&mut self) -> Result<(), LlmError> {
        Ok(())
    }

    fn unload(&mut self) {}

    fn generate(
        &mut self,
        input: &[InputItem],
        tools: &[ToolDefinition],
        params: &GenParams,
        emit: &mut (dyn FnMut(OutputItem) + Send),
        cancel: Arc<AtomicBool>,
    ) -> Result<FinishReason, LlmError> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(FinishReason::Cancelled);
        }
        let messages = self.build_messages(input)?;
        // 调用方可能已身处 tokio 运行时（如 CLI 的 `#[tokio::main]`），此时对自建 runtime
        // 就地 block_on 会 panic；换普通线程执行。GUI worker 本就是普通线程，走原路径。
        if tokio::runtime::Handle::try_current().is_ok() {
            let this = &*self;
            let fut = this.generate_stream(messages, tools, params, emit, &cancel);
            return std::thread::scope(|s| {
                s.spawn(move || {
                    this.runtime
                        .as_ref()
                        .expect("runtime 存在于正常生命周期内")
                        .block_on(fut)
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(LlmError::BackendUnavailable(
                        "generate 线程 panic".to_string(),
                    ))
                })
            });
        }
        self.runtime
            .as_ref()
            .expect("runtime 存在于正常生命周期内")
            .block_on(self.generate_stream(messages, tools, params, emit, &cancel))
    }
}

impl Drop for OpenAiChatProvider {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        // tokio 运行时不能在 async 上下文中 drop（Cannot drop a runtime ...），
        // 移交普通线程收尾，兼容 CLI 类调用方。
        if tokio::runtime::Handle::try_current().is_ok() {
            let _ = std::thread::spawn(move || drop(runtime)).join();
        } else {
            drop(runtime);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ChatMessage, ChatRole, ToolResult};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn test_config(base_url: Option<String>, model: Option<String>) -> ResolvedLlmConfig {
        ResolvedLlmConfig {
            enabled: true,
            cli_tools: false,
            sprite_tool: true,
            prompt_cache: true,
            thinking: false,
            reasoning_effort: None,
            provider: "openai".to_string(),
            system_prompt: "测试系统提示".to_string(),
            params: GenParams::default(),
            base_url,
            api_key: Some("test-key".to_string()),
            model,
        }
    }

    /// 启动一个只服务一次请求的 mock server，返回给定状态码 + SSE body。
    /// 返回 (端口, 请求接收端)：接收端会收到 (请求路径, 请求体)。
    fn spawn_mock(body: &'static str, status: u16) -> (u16, mpsc::Receiver<(String, String)>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            if let Ok(mut req) = server.recv() {
                let url = req.url().to_string();
                let mut req_body = String::new();
                req.as_reader().read_to_string(&mut req_body).ok();
                let _ = tx.send((url, req_body));
                let header =
                    tiny_http::Header::from_bytes("Content-Type", "text/event-stream").unwrap();
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(status)
                    .with_header(header);
                req.respond(resp).ok();
            }
        });
        (port, rx)
    }

    fn user_input(text: &str) -> Vec<InputItem> {
        vec![InputItem::Message(ChatMessage::new(ChatRole::User, text))]
    }

    // ---------- 构造校验 ----------

    #[test]
    fn test_new_requires_base_url() {
        let cfg = test_config(None, Some("glm-4.7-flash".to_string()));
        let err = OpenAiChatProvider::new(&cfg).err().unwrap();
        assert!(err.to_string().contains("base_url"), "实际错误：{err}");
    }

    #[test]
    fn test_new_requires_model() {
        let cfg = test_config(Some("http://127.0.0.1:1".to_string()), None);
        let err = OpenAiChatProvider::new(&cfg).err().unwrap();
        assert!(err.to_string().contains("model"), "实际错误：{err}");
    }

    #[test]
    fn test_load_is_noop_ok() {
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();
        assert!(p.is_ready());
        assert!(p.load().is_ok());
        p.unload();
    }

    // ---------- 消息/工具转换 ----------

    #[test]
    fn test_build_messages_injects_system_prompt_when_missing() {
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let p = OpenAiChatProvider::new(&cfg).unwrap();
        let messages = p.build_messages(&user_input("你好")).unwrap();
        let value = serde_json::to_value(&messages).unwrap();
        assert_eq!(value[0]["role"], "system");
        assert_eq!(value[0]["content"], "测试系统提示");
        assert_eq!(value[1]["role"], "user");
        assert_eq!(value[1]["content"], "你好");
    }

    #[test]
    fn test_build_messages_keeps_existing_system_prompt() {
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let p = OpenAiChatProvider::new(&cfg).unwrap();
        let input = vec![
            InputItem::Message(ChatMessage::new(ChatRole::System, "已有提示")),
            InputItem::Message(ChatMessage::new(ChatRole::User, "你好")),
        ];
        let messages = p.build_messages(&input).unwrap();
        let value = serde_json::to_value(&messages).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 2, "不应重复注入 system");
        assert_eq!(value[0]["role"], "system");
        assert_eq!(value[0]["content"], "已有提示");
    }

    #[test]
    fn test_build_messages_maps_tool_call_and_tool_result() {
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let p = OpenAiChatProvider::new(&cfg).unwrap();
        let input = vec![
            InputItem::Message(ChatMessage::new(ChatRole::User, "几点了")),
            InputItem::ToolCall(ToolCall {
                name: "get_time".to_string(),
                arguments: "{}".to_string(),
                id: Some("call_1".to_string()),
            }),
            InputItem::ToolResult(ToolResult {
                id: "call_1".to_string(),
                name: "get_time".to_string(),
                content: "12:00".to_string(),
            }),
        ];
        let messages = p.build_messages(&input).unwrap();
        let value = serde_json::to_value(&messages).unwrap();
        // system 注入 + user + assistant(tool_calls) + tool
        assert_eq!(value[1]["role"], "user");
        assert_eq!(value[2]["role"], "assistant");
        assert_eq!(value[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(value[2]["tool_calls"][0]["type"], "function");
        assert_eq!(value[2]["tool_calls"][0]["function"]["name"], "get_time");
        assert_eq!(value[2]["tool_calls"][0]["function"]["arguments"], "{}");
        assert_eq!(value[3]["role"], "tool");
        assert_eq!(value[3]["tool_call_id"], "call_1");
        assert_eq!(value[3]["content"], "12:00");
    }

    #[test]
    fn test_to_tools_maps_definitions() {
        let tools = vec![ToolDefinition {
            name: "get_time".to_string(),
            description: "获取当前时间".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let result = OpenAiChatProvider::to_tools(&tools);
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value[0]["type"], "function");
        assert_eq!(value[0]["function"]["name"], "get_time");
        assert_eq!(value[0]["function"]["description"], "获取当前时间");
        assert_eq!(value[0]["function"]["parameters"]["type"], "object");
    }

    // ---------- 流式生成（mock SSE server） ----------

    const SSE_TEXT: &str = concat!(
        "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你好\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"世界\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[test]
    fn test_generate_streams_text_deltas() {
        let (port, rx) = spawn_mock(SSE_TEXT, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let reason = p
            .generate(
                &user_input("打个招呼"),
                &[],
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap();

        assert_eq!(reason, FinishReason::Eos);
        let texts: Vec<&str> = out
            .iter()
            .map(|item| match item {
                OutputItem::MessageDelta(d) => d.text.as_str(),
                other => panic!("期望 MessageDelta，实际 {other:?}"),
            })
            .collect();
        assert_eq!(texts, ["你好", "世界"]);

        // 请求契约：chat/completions 路径 + 关键字段
        let (url, req_body) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(url, "/chat/completions");
        let req: serde_json::Value = serde_json::from_str(&req_body).unwrap();
        assert_eq!(req["model"], "glm-4.7-flash");
        assert_eq!(req["stream"], true);
        assert_eq!(req["messages"][0]["role"], "system");
        assert_eq!(req["messages"][1]["role"], "user");
        assert_eq!(req["messages"][1]["content"], "打个招呼");
        assert_eq!(req["max_tokens"], 512);
        assert!((req["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-6);
    }

    /// 回归：CLI（`#[tokio::main]`）在 tokio 运行时上下文里调用 generate 时，
    /// provider 自建的 runtime 不得 panic（Cannot start a runtime from within a runtime）。
    #[tokio::test]
    async fn test_generate_inside_tokio_runtime() {
        let (port, _rx) = spawn_mock(SSE_TEXT, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let reason = p
            .generate(
                &user_input("打个招呼"),
                &[],
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap();

        assert_eq!(reason, FinishReason::Eos);
        let texts: Vec<&str> = out
            .iter()
            .map(|item| match item {
                OutputItem::MessageDelta(d) => d.text.as_str(),
                other => panic!("期望 MessageDelta，实际 {other:?}"),
            })
            .collect();
        assert_eq!(texts, ["你好", "世界"]);
    }

    /// 回归：tokio 运行时上下文里 drop provider 时，自建 runtime 不得 panic
    /// （Cannot drop a runtime in a context where blocking is not allowed）。
    #[tokio::test]
    async fn test_drop_inside_tokio_runtime() {
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let _p = OpenAiChatProvider::new(&cfg).unwrap();
        // drop 发生在 tokio 上下文：修复前此处 panic
    }

    #[test]
    fn test_generate_ignores_reasoning_content() {
        // 智谱 GLM 的思考链走非标准字段 reasoning_content，应被静默丢弃（不进 TTS）
        let sse = concat!(
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"让我想想\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"答案\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (port, _rx) = spawn_mock(sse, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        p.generate(
            &user_input("hi"),
            &[],
            &GenParams::default(),
            &mut |item| out.push(item),
            cancel,
        )
        .unwrap();

        let texts: Vec<&str> = out
            .iter()
            .map(|item| match item {
                OutputItem::MessageDelta(d) => d.text.as_str(),
                other => panic!("期望 MessageDelta，实际 {other:?}"),
            })
            .collect();
        assert_eq!(texts, ["答案"], "reasoning_content 不应产生输出");
    }

    #[test]
    fn test_generate_merges_tool_call_chunks() {
        let sse = concat!(
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_time\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"上海\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (port, _rx) = spawn_mock(sse, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let tools = vec![ToolDefinition {
            name: "get_time".to_string(),
            description: "获取当前时间".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let reason = p
            .generate(
                &user_input("几点了"),
                &tools,
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap();

        assert_eq!(reason, FinishReason::Eos);
        assert_eq!(out.len(), 1, "碎片应合并为一次 ToolCall，实际 {out:?}");
        match &out[0] {
            OutputItem::ToolCall(call) => {
                assert_eq!(call.id.as_deref(), Some("call_1"));
                assert_eq!(call.name, "get_time");
                assert_eq!(call.arguments, "{\"city\":\"上海\"}");
            }
            other => panic!("期望 ToolCall，实际 {other:?}"),
        }
    }

    #[test]
    fn test_generate_maps_length_to_max_tokens() {
        let sse = concat!(
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"截断\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"glm-4.7-flash\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let (port, _rx) = spawn_mock(sse, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let reason = p
            .generate(
                &user_input("hi"),
                &[],
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap();
        assert_eq!(reason, FinishReason::MaxTokens);
    }

    #[test]
    fn test_generate_cancelled_before_request() {
        // 不可达地址：若实现未在发请求前检查 cancel，将返回连接错误而非 Cancelled
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(true));
        let reason = p
            .generate(
                &user_input("hi"),
                &[],
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap();
        assert_eq!(reason, FinishReason::Cancelled);
        assert!(out.is_empty());
    }

    #[test]
    fn test_generate_http_error_maps_to_backend_unavailable() {
        // 用 401（不可重试）验证错误映射；429/5xx 由 SDK 内置 OpenAIRetryLayer 自动指数退避重试
        // （覆盖智谱 1305 限流场景），这里不重复测试以免拖慢套件。
        let (port, _rx) = spawn_mock(
            "{\"error\":{\"code\":\"1001\",\"message\":\"invalid api key\"}}",
            401,
        );
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("glm-4.7-flash".to_string()),
        );
        let mut p = OpenAiChatProvider::new(&cfg).unwrap();

        let mut out: Vec<OutputItem> = Vec::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let err = p
            .generate(
                &user_input("hi"),
                &[],
                &GenParams::default(),
                &mut |item| out.push(item),
                cancel,
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid api key"),
            "实际错误：{err}"
        );
    }
}
