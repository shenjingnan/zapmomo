/// Anthropic 原生 Messages API provider（基于 genai crate 的 Anthropic adapter）。
///
/// 通过 `POST {base_url}/messages` 流式调用（SSE 由 genai 解析，默认端点
/// `https://api.anthropic.com/v1/`）。只用 genai 的消息/流式/tool use 传输层原语，
/// agent 编排由自研 `Agent`（`crate::llm::agent`）负责，不用 genai 的任何 agent 抽象。
///
/// 与 OpenAI 兼容 Chat Completions 的关键差异（由本 provider 屏蔽）：
/// - system prompt 是请求顶层字段而非消息（genai `ChatRequest::with_system` 一等字段）。
/// - `max_tokens` 必填（genai adapter 总是发送）。
/// - 新一代 Claude 模型（Opus 4.7+/5、Sonnet 5、Fable/Mythos 5）移除了
///   temperature/top_p/top_k 采样参数，发送即 400，按模型白名单门控（见
///   [`supports_sampling_params`]）。
/// - 流式 tool_use 的参数是 `input_json_delta` 分片：genai 的 `ToolCallChunk` 是
///   增量（Value::String 累积分片），完整解析后的 ToolCall 只在 End 事件的
///   `captured_tool_calls`（需 `capture_tool_calls` 开启），本 provider 以此为
///   准、ToolCallChunk 仅作兜底，流结束后统一 emit（与 http.rs 契约一致）。
///
/// 同步/异步桥接：provider 内部持有 current_thread tokio runtime，`generate` 里
/// `block_on` 驱动流式请求；runtime 与 provider 同生命周期、不跨线程移动，
/// 不违反 `LlmProvider` 不加 `Send` 的约定。
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, MessageOptions,
    ReasoningEffort, StopReason, Tool as GenaiTool, ToolCall as GenaiToolCall, ToolResponse,
};
use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ClientConfig, ServiceTarget};

use crate::llm::config::ResolvedLlmConfig;
use crate::llm::error::LlmError;
use crate::llm::provider::LlmProvider;
use crate::llm::types::{
    ChatRole, FinishReason, GenParams, InputItem, OutputItem, TokenDelta, ToolCall, ToolDefinition,
};

pub struct AnthropicProvider {
    client: Client,
    /// 带 `anthropic::` 命名空间前缀的模型名（强制走 Anthropic adapter）
    model: String,
    system_prompt: String,
    /// 是否在末位消息打 cache_control 断点（prompt caching）
    prompt_cache: bool,
    /// extended thinking 力度（None = API 默认）；开启后与采样参数互斥
    reasoning_effort: Option<ReasoningEffort>,
    runtime: tokio::runtime::Runtime,
}

impl AnthropicProvider {
    pub fn new(config: &ResolvedLlmConfig) -> Result<Self, LlmError> {
        let model = config
            .model
            .clone()
            .ok_or_else(|| LlmError::BackendUnavailable("未配置模型名（model）".to_string()))?;

        // ServiceTargetResolver 在 genai 默认解析（端点 + ANTHROPIC_API_KEY 环境变量）
        // 之后运行，仅在配置显式给出时覆盖对应字段；api_key 未配置时不提前报错，
        // 留给 genai 默认 resolver 读环境变量，两者都缺会在首次请求时报错（见
        // map_genai_error 的认证分支）
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let resolver = ServiceTargetResolver::from_resolver_fn(move |mut target: ServiceTarget| {
            if let Some(url) = &base_url {
                // genai adapter 以 `{base_url}messages` 拼 URL，端点必须以 "/" 结尾
                let url = if url.ends_with('/') {
                    url.clone()
                } else {
                    format!("{url}/")
                };
                target.endpoint = Endpoint::from_owned(url);
            }
            if let Some(key) = &api_key {
                target.auth = AuthData::from_single(key);
            }
            Ok(target)
        });
        let client = Client::builder()
            .with_config(ClientConfig::default().with_service_target_resolver(resolver))
            .build();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| LlmError::BackendUnavailable(format!("创建 tokio runtime 失败：{e}")))?;

        // 模型名统一加 anthropic:: 命名空间前缀：裸模型名靠 genai 的名字启发式推断
        // adapter，代理网关上的自定义模型名可能失配（genai 发请求时会剥掉前缀）
        let model = if model.contains("::") {
            model
        } else {
            format!("anthropic::{model}")
        };

        // extended thinking 力度：非法值快速失败（构造期中文报错，而非首请求 400）
        let reasoning_effort = config
            .reasoning_effort
            .as_deref()
            .map(parse_reasoning_effort)
            .transpose()?;

        Ok(Self {
            client,
            model,
            system_prompt: config.system_prompt.clone(),
            prompt_cache: config.prompt_cache,
            reasoning_effort,
            runtime,
        })
    }

    /// 把 `InputItem` 列表转成 genai 的 `ChatRequest`。
    ///
    /// system 消息提取为 `ChatRequest.system` 顶层字段（Anthropic 协议要求）；
    /// 若调用方未提供 system 消息，则注入配置的 system prompt。
    pub fn build_request(&self, input: &[InputItem], tools: &[ToolDefinition]) -> ChatRequest {
        let mut system_parts: Vec<&str> = Vec::new();
        let mut messages: Vec<ChatMessage> = Vec::with_capacity(input.len());
        for item in input {
            match item {
                InputItem::Message(m) => match m.role {
                    ChatRole::System => system_parts.push(m.content.as_str()),
                    ChatRole::User => messages.push(ChatMessage::user(m.content.clone())),
                    ChatRole::Assistant => {
                        messages.push(ChatMessage::assistant(m.content.clone()));
                    }
                    // 工具结果正常走 InputItem::ToolResult；裸 Tool 角色消息按 user 处理，
                    // 避免构造缺少 tool_use_id 的非法 tool_result 消息
                    ChatRole::Tool => messages.push(ChatMessage::user(m.content.clone())),
                },
                InputItem::ToolCall(c) => {
                    // assistant 的 tool_use 块；Anthropic 要求 input 是合法 JSON object，
                    // agent 回灌的 arguments 是本 provider 产出的 JSON 字符串，正常必可解析
                    let fn_arguments = serde_json::from_str(&c.arguments)
                        .unwrap_or_else(|_| serde_json::json!({}));
                    messages.push(ChatMessage::from(vec![GenaiToolCall {
                        call_id: c.id.clone().unwrap_or_default(),
                        fn_name: c.name.clone(),
                        fn_arguments,
                        thought_signatures: None,
                    }]));
                }
                // genai adapter 会把 tool 角色的 ToolResponse 转成 user 消息中的
                // tool_result 块（tool_use_id 配对），符合 Anthropic 协议要求
                InputItem::ToolResult(t) => messages.push(ChatMessage::from(ToolResponse::new(
                    t.id.clone(),
                    t.content.clone(),
                ))),
            }
        }

        let mut request = ChatRequest::new(messages);
        // prompt caching：在末位消息打 ephemeral 断点，缓存「system + 全部历史」前缀。
        // Agent 多轮（tool 回灌）与下一用户回合都以此前缀开头，命中缓存降延迟/成本；
        // 每次只在末位打一个断点，远低于 4 个断点的上限
        if self.prompt_cache
            && let Some(last) = request.messages.last_mut()
        {
            last.options =
                Some(MessageOptions::default().with_cache_control(CacheControl::Ephemeral));
        }
        // system 顶层字段：input 自带的优先（多条合并），否则注入配置值
        let system = if system_parts.is_empty() {
            (!self.system_prompt.is_empty()).then(|| self.system_prompt.clone())
        } else {
            Some(system_parts.join("\n\n"))
        };
        if let Some(system) = system {
            request = request.with_system(system);
        }
        if !tools.is_empty() {
            request = request.with_tools(tools.iter().map(|t| {
                GenaiTool::new(t.name.clone())
                    .with_description(t.description.clone())
                    .with_schema(t.parameters.clone())
            }));
        }
        request
    }

    /// 构建 `ChatOptions`：max_tokens 总是发送（Anthropic 必填）；
    /// temperature/top_p 按模型能力白名单门控（开启 thinking 时互斥不发）；
    /// 同时开启 `capture_tool_calls`（End 事件取完整 tool call）与
    /// `capture_usage`（缓存命中观测）。
    pub fn build_options(&self, params: &GenParams) -> ChatOptions {
        let mut options = ChatOptions::default()
            .with_max_tokens(params.max_tokens as u32)
            .with_capture_tool_calls(true)
            .with_capture_usage(true);
        if let Some(effort) = &self.reasoning_effort {
            // 开启 thinking 时不可自定义采样参数（Anthropic 要求 temperature=1），互斥
            options = options.with_reasoning_effort(effort.clone());
        } else if supports_sampling_params(&self.model) {
            options = options
                .with_temperature(params.temperature as f64)
                .with_top_p(params.top_p as f64);
        }
        // top_k/min_p/repeat_penalty/seed 对 Anthropic 无对应概念（或新一代模型拒绝），不发
        options
    }

    /// 流式生成主循环（async 部分）：逐事件 emit 文本增量，tool call 流末统一 emit。
    async fn generate_stream(
        &self,
        request: ChatRequest,
        options: &ChatOptions,
        emit: &mut dyn FnMut(OutputItem),
        cancel: &AtomicBool,
    ) -> Result<FinishReason, LlmError> {
        let mut stream = self
            .client
            .exec_chat_stream(&self.model, request, Some(options))
            .await
            .map_err(map_genai_error)?
            .stream;

        // 完整 tool call 以 End 事件的 captured_tool_calls 为准（capture_tool_calls
        // 已开启）；ToolCallChunk 是增量分片，仅在 End 缺失时兜底（按 call_id 保留
        // 最后一次累积值）
        let mut fallback_calls: Vec<GenaiToolCall> = Vec::new();
        let mut captured_calls: Vec<GenaiToolCall> = Vec::new();
        let mut finish = FinishReason::Eos;
        while let Some(item) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                return Ok(FinishReason::Cancelled);
            }
            match item.map_err(map_genai_error)? {
                ChatStreamEvent::Start => {}
                ChatStreamEvent::Chunk(chunk) => {
                    if !chunk.content.is_empty() {
                        emit(OutputItem::MessageDelta(TokenDelta::new(chunk.content)));
                    }
                }
                // 第一版不启用 extended thinking；思考/签名块直接忽略（不进 TTS/历史）
                ChatStreamEvent::ReasoningChunk(_) | ChatStreamEvent::ThoughtSignatureChunk(_) => {}
                ChatStreamEvent::ToolCallChunk(chunk) => {
                    let call = chunk.tool_call;
                    match fallback_calls
                        .iter_mut()
                        .find(|c| c.call_id == call.call_id)
                    {
                        Some(acc) => *acc = call,
                        None => fallback_calls.push(call),
                    }
                }
                ChatStreamEvent::End(end) => {
                    if let Some(calls) = end.captured_tool_calls() {
                        captured_calls = calls.into_iter().cloned().collect();
                    }
                    // usage 观测：prompt caching 命中/写入量（仅 debug 日志，不外传）
                    if let Some(usage) = &end.captured_usage {
                        let (mut cache_write, mut cache_read) = (0, 0);
                        if let Some(details) = &usage.prompt_tokens_details {
                            cache_write = details.cache_creation_tokens.unwrap_or(0);
                            cache_read = details.cached_tokens.unwrap_or(0);
                        }
                        tracing::debug!(
                            "anthropic usage: input={:?} output={:?} cache_write={cache_write} cache_read={cache_read}",
                            usage.prompt_tokens,
                            usage.completion_tokens,
                        );
                    }
                    finish = match end.captured_stop_reason {
                        Some(StopReason::MaxTokens(_)) => FinishReason::MaxTokens,
                        Some(StopReason::ContentFilter(reason)) => {
                            return Err(LlmError::InferenceFailed(format!(
                                "内容被服务端安全策略拦截（{reason}）"
                            )));
                        }
                        // Completed/ToolCall/StopSequence/Other 均视为正常结束
                        _ => FinishReason::Eos,
                    };
                }
            }
        }

        let calls = if captured_calls.is_empty() {
            fallback_calls
        } else {
            captured_calls
        };
        // 流结束后统一 emit 完整 tool call（契约：Agent 依赖一轮内收齐）；
        // call_id（toolu_xxx）原样保留，Agent 回灌 tool_result 靠它配对
        for call in calls {
            emit(OutputItem::ToolCall(ToolCall {
                name: call.fn_name,
                arguments: fn_arguments_to_string(call.fn_arguments),
                id: Some(call.call_id),
            }));
        }
        Ok(finish)
    }
}

/// 解析 reasoning_effort 配置值：非法值构造期快速失败（中文报错），而非首请求 400。
fn parse_reasoning_effort(value: &str) -> Result<ReasoningEffort, LlmError> {
    match value.to_lowercase().as_str() {
        "low" => Ok(ReasoningEffort::Low),
        "medium" => Ok(ReasoningEffort::Medium),
        "high" => Ok(ReasoningEffort::High),
        "max" => Ok(ReasoningEffort::Max),
        other => Err(LlmError::BackendUnavailable(format!(
            "不支持的 reasoning_effort：{other}（可选 low / medium / high / max）"
        ))),
    }
}

/// 新一代 Claude 模型（Opus 4.7+/5、Sonnet 5、Fable/Mythos 5）移除了
/// temperature/top_p/top_k 采样参数，发送即 400。保守白名单：只对确认支持的
/// 模型发送，未知模型默认不发——漏发只是回落模型默认行为，误发则直接 400，
/// 默认方向是安全的。
fn supports_sampling_params(model: &str) -> bool {
    // 剥掉命名空间前缀（如 anthropic::）
    let name = model.rsplit("::").next().unwrap_or(model);
    name.starts_with("claude-3")
        || name.starts_with("claude-opus-4-0")
        || name.starts_with("claude-opus-4-1")
        || name.starts_with("claude-opus-4-5")
        || name.starts_with("claude-opus-4-6")
        || name.starts_with("claude-sonnet-4-0")
        || name.starts_with("claude-sonnet-4-5")
        || name.starts_with("claude-sonnet-4-6")
        || name.starts_with("claude-haiku-4-5")
}

/// genai 的 fn_arguments 在流式增量里是 Value::String（部分 JSON 累积），
/// 完整解析后是 Value::Object；统一转成 JSON 字符串。
fn fn_arguments_to_string(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

/// genai 错误到 LlmError 的映射：HTTP 状态错误按状态码细分（消息附截断的错误体），
/// 认证缺失与网络错误归为 BackendUnavailable，其余兜底 InferenceFailed。
fn map_genai_error(err: genai::Error) -> LlmError {
    // 非流式请求的状态错误：webc::Error::ResponseFailedStatus
    if let genai::Error::WebAdapterCall { webc_error, .. }
    | genai::Error::WebModelCall { webc_error, .. } = &err
    {
        if let genai::webc::Error::ResponseFailedStatus { status, body, .. } = webc_error {
            return map_http_status(status.as_u16(), body);
        }
        return LlmError::BackendUnavailable(format!("Anthropic 网络请求失败：{webc_error}"));
    }
    // 流式请求的状态错误：genai::Error::HttpError 被装箱进 WebStream.error
    if let genai::Error::WebStream { error, .. } = &err
        && let Some(genai::Error::HttpError { status, body, .. }) =
            error.downcast_ref::<genai::Error>()
    {
        return map_http_status(status.as_u16(), body);
    }
    match &err {
        // api_key 缺失：配置未给且 ANTHROPIC_API_KEY 环境变量也没设
        genai::Error::Resolver { .. }
        | genai::Error::RequiresApiKey { .. }
        | genai::Error::NoAuthData { .. } => LlmError::BackendUnavailable(
            "未配置 Anthropic api_key（或设置 ANTHROPIC_API_KEY 环境变量）".to_string(),
        ),
        genai::Error::WebStream { .. } => {
            LlmError::BackendUnavailable(format!("Anthropic 流式连接失败：{err}"))
        }
        other => LlmError::InferenceFailed(other.to_string()),
    }
}

/// HTTP 状态码到 LlmError 的细分：401/403 认证、429 限流、其余 InferenceFailed，
/// 错误体截断 200 字符避免长错误刷屏。
fn map_http_status(status: u16, body: &str) -> LlmError {
    let body: String = body.chars().take(200).collect();
    match status {
        401 | 403 => {
            LlmError::BackendUnavailable(format!("Anthropic 认证失败，请检查 api_key（{body}）"))
        }
        429 => LlmError::InferenceFailed(format!("Anthropic 请求限流，请稍后重试（{body}）")),
        code => LlmError::InferenceFailed(format!("Anthropic 请求失败（HTTP {code}）：{body}")),
    }
}

impl LlmProvider for AnthropicProvider {
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
        emit: &mut dyn FnMut(OutputItem),
        cancel: Arc<AtomicBool>,
    ) -> Result<FinishReason, LlmError> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(FinishReason::Cancelled);
        }
        let request = self.build_request(input, tools);
        let options = self.build_options(params);
        self.runtime
            .block_on(self.generate_stream(request, &options, emit, &cancel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{ChatMessage as LlmChatMessage, ToolResult};
    use std::sync::mpsc;
    use std::thread;

    fn test_config(base_url: Option<String>, model: Option<String>) -> ResolvedLlmConfig {
        ResolvedLlmConfig {
            enabled: true,
            cli_tools: false,
            prompt_cache: true,
            reasoning_effort: None,
            provider: "anthropic".to_string(),
            system_prompt: "测试系统提示".to_string(),
            params: GenParams::default(),
            base_url,
            api_key: Some("test-key".to_string()),
            model,
        }
    }

    /// mock server 捕获的请求（URL / 关键请求头 / 请求体）
    struct CapturedRequest {
        url: String,
        x_api_key: Option<String>,
        anthropic_version: Option<String>,
        body: String,
    }

    /// 启动一个只服务一次请求的 mock server，返回给定状态码 + SSE body。
    /// 返回 (端口, 请求接收端)。
    fn spawn_mock(body: &'static str, status: u16) -> (u16, mpsc::Receiver<CapturedRequest>) {
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
                // tiny_http HeaderField::equiv 只收 &'static str，逐个字面量查找
                let find_header = |field: &'static str| {
                    req.headers()
                        .iter()
                        .find(|h| h.field.equiv(field))
                        .map(|h| h.value.as_str().to_string())
                };
                let captured = CapturedRequest {
                    url,
                    x_api_key: find_header("x-api-key"),
                    anthropic_version: find_header("anthropic-version"),
                    body: {
                        let mut buf = String::new();
                        req.as_reader().read_to_string(&mut buf).ok();
                        buf
                    },
                };
                let _ = tx.send(captured);
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
        vec![InputItem::Message(LlmChatMessage::new(
            ChatRole::User,
            text,
        ))]
    }

    fn collect_generate(
        provider: &mut AnthropicProvider,
        input: &[InputItem],
        tools: &[ToolDefinition],
        cancel: Arc<AtomicBool>,
    ) -> (Vec<OutputItem>, Result<FinishReason, LlmError>) {
        let mut items = Vec::new();
        let mut emit = |item: OutputItem| items.push(item);
        let result = provider.generate(input, tools, &GenParams::default(), &mut emit, cancel);
        (items, result)
    }

    /// Anthropic 文本流 SSE：message_start → text×2 → message_delta(end_turn) → message_stop
    const TEXT_STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-haiku-4-5\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你好\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"，世界\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    /// Anthropic tool_use 流 SSE：input_json_delta 分片×2 → message_delta(tool_use)
    const TOOL_USE_STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_2\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-haiku-4-5\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_01ABC\",\"name\":\"get_current_time\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"上海\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":12}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    // ---------- 构造校验 ----------

    #[test]
    fn test_new_requires_model() {
        let cfg = test_config(None, None);
        let err = AnthropicProvider::new(&cfg).err().unwrap();
        assert!(err.to_string().contains("model"), "实际错误：{err}");
    }

    #[test]
    fn test_new_without_base_url_and_api_key_ok() {
        // base_url 缺省用官方端点；api_key 缺省留给 ANTHROPIC_API_KEY 环境变量
        let mut cfg = test_config(None, Some("claude-haiku-4-5".to_string()));
        cfg.api_key = None;
        let provider = AnthropicProvider::new(&cfg).unwrap();
        assert!(provider.is_ready());
    }

    #[test]
    fn test_model_gets_anthropic_namespace_prefix() {
        let cfg = test_config(None, Some("claude-haiku-4-5".to_string()));
        let provider = AnthropicProvider::new(&cfg).unwrap();
        assert_eq!(provider.model, "anthropic::claude-haiku-4-5");
        // 已带前缀（如代理网关命名空间）的不重复加
        let cfg = test_config(None, Some("proxy::my-model".to_string()));
        let provider = AnthropicProvider::new(&cfg).unwrap();
        assert_eq!(provider.model, "proxy::my-model");
    }

    // ---------- build_request 转换 ----------

    #[test]
    fn test_build_request_injects_system_prompt() {
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let req = provider.build_request(&user_input("你好"), &[]);
        assert_eq!(req.system.as_deref(), Some("测试系统提示"));
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, genai::chat::ChatRole::User);
        assert_eq!(req.messages[0].content.first_text(), Some("你好"));
    }

    #[test]
    fn test_build_request_keeps_input_system() {
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let input = vec![
            InputItem::Message(LlmChatMessage::new(ChatRole::System, "自定义系统")),
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "你好")),
        ];
        let req = provider.build_request(&input, &[]);
        // input 自带 system 时用 input 的，不注入配置值
        assert_eq!(req.system.as_deref(), Some("自定义系统"));
        assert_eq!(req.messages.len(), 1);
    }

    #[test]
    fn test_build_request_tool_call_and_result() {
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let input = vec![
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "现在几点")),
            InputItem::ToolCall(ToolCall {
                name: "get_current_time".to_string(),
                arguments: "{\"tz\":\"local\"}".to_string(),
                id: Some("toolu_01ABC".to_string()),
            }),
            InputItem::ToolResult(ToolResult {
                id: "toolu_01ABC".to_string(),
                name: "get_current_time".to_string(),
                content: "2026-08-27T12:00:00+08:00".to_string(),
            }),
        ];
        let req = provider.build_request(&input, &[]);
        assert_eq!(req.messages.len(), 3);
        // assistant 的 tool_use 块
        let calls = req.messages[1].content.tool_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "toolu_01ABC");
        assert_eq!(calls[0].fn_name, "get_current_time");
        assert_eq!(calls[0].fn_arguments, serde_json::json!({"tz": "local"}));
        // tool_result 转 tool 角色消息（adapter 再转 user 消息的 tool_result 块）
        assert_eq!(req.messages[2].role, genai::chat::ChatRole::Tool);
    }

    #[test]
    fn test_build_request_tools() {
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let tools = vec![ToolDefinition {
            name: "get_current_time".to_string(),
            description: "获取当前时间".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let req = provider.build_request(&user_input("几点了"), &tools);
        let tools = req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description.as_deref(), Some("获取当前时间"));
        assert_eq!(
            tools[0].schema,
            Some(serde_json::json!({"type": "object", "properties": {}}))
        );
    }

    // ---------- 采样参数门控 ----------

    #[test]
    fn test_supports_sampling_params_whitelist() {
        // 白名单内：发送 temperature/top_p
        for model in [
            "claude-haiku-4-5",
            "claude-3-5-sonnet-20241022",
            "claude-opus-4-1",
            "claude-opus-4-5",
            "claude-opus-4-6",
            "claude-sonnet-4-5",
            "claude-sonnet-4-6",
            "anthropic::claude-sonnet-4-6",
        ] {
            assert!(supports_sampling_params(model), "{model} 应支持采样参数");
        }
        // 白名单外（新一代模型 / 未知名）：不发送，避免 400
        for model in [
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-mythos-5",
            "custom-proxy-model",
        ] {
            assert!(!supports_sampling_params(model), "{model} 不应发送采样参数");
        }
    }

    #[test]
    fn test_build_options_gates_sampling_params() {
        let params = GenParams::default();
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let options = provider.build_options(&params);
        assert_eq!(options.max_tokens, Some(params.max_tokens as u32));
        assert_eq!(options.temperature, Some(params.temperature as f64));
        assert_eq!(options.top_p, Some(params.top_p as f64));
        assert_eq!(options.capture_tool_calls, Some(true));

        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-opus-5".to_string()))).unwrap();
        let options = provider.build_options(&params);
        assert_eq!(options.max_tokens, Some(params.max_tokens as u32));
        assert_eq!(options.temperature, None);
        assert_eq!(options.top_p, None);
    }

    // ---------- prompt caching / extended thinking ----------

    #[test]
    fn test_build_request_cache_breakpoint_on_last_message() {
        let provider =
            AnthropicProvider::new(&test_config(None, Some("claude-haiku-4-5".to_string())))
                .unwrap();
        let input = vec![
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "第一条")),
            InputItem::Message(LlmChatMessage::new(ChatRole::Assistant, "回复")),
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "第二条")),
        ];
        let req = provider.build_request(&input, &[]);
        // 仅末位消息带 ephemeral 断点
        assert!(req.messages[0].options.is_none());
        assert!(req.messages[1].options.is_none());
        assert!(
            req.messages[2]
                .options
                .as_ref()
                .unwrap()
                .cache_control
                .is_some()
        );
    }

    #[test]
    fn test_build_request_cache_disabled() {
        let mut cfg = test_config(None, Some("claude-haiku-4-5".to_string()));
        cfg.prompt_cache = false;
        let provider = AnthropicProvider::new(&cfg).unwrap();
        let req = provider.build_request(&user_input("你好"), &[]);
        assert!(req.messages[0].options.is_none());
    }

    #[test]
    fn test_new_invalid_reasoning_effort() {
        let mut cfg = test_config(None, Some("claude-sonnet-4-6".to_string()));
        cfg.reasoning_effort = Some("extreme".to_string());
        let err = AnthropicProvider::new(&cfg).err().unwrap();
        assert!(
            err.to_string().contains("reasoning_effort"),
            "实际错误：{err}"
        );
    }

    #[test]
    fn test_build_options_reasoning_disables_sampling() {
        let mut cfg = test_config(None, Some("claude-sonnet-4-6".to_string()));
        cfg.reasoning_effort = Some("low".to_string());
        let provider = AnthropicProvider::new(&cfg).unwrap();
        let options = provider.build_options(&GenParams::default());
        // sonnet-4-6 本在采样白名单内，但开启 thinking 后互斥不发
        assert!(matches!(
            options.reasoning_effort,
            Some(ReasoningEffort::Low)
        ));
        assert_eq!(options.temperature, None);
        assert_eq!(options.top_p, None);
    }

    // ---------- 流式 mock 测试 ----------

    #[test]
    fn test_stream_text_deltas_and_request_contract() {
        let (port, rx) = spawn_mock(TEXT_STREAM, 200);
        // base_url 不带尾斜杠：验证 provider 归一化为 "{base}/" 后拼出 /v1/messages
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}/v1")),
            Some("claude-haiku-4-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let (items, result) = collect_generate(
            &mut provider,
            &user_input("你好"),
            &[],
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.unwrap(), FinishReason::Eos);
        let texts: Vec<&str> = items
            .iter()
            .filter_map(|item| match item {
                OutputItem::MessageDelta(d) => Some(d.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["你好", "，世界"]);

        let req = rx.recv().unwrap();
        assert_eq!(req.url, "/v1/messages");
        assert_eq!(req.x_api_key.as_deref(), Some("test-key"));
        assert_eq!(req.anthropic_version.as_deref(), Some("2023-06-01"));
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        // genai 发请求时剥掉 anthropic:: 命名空间前缀
        assert_eq!(body["model"], "claude-haiku-4-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], GenParams::default().max_tokens as u64);
        // system 是请求顶层字段而非消息
        assert_eq!(body["system"], "测试系统提示");
        assert_eq!(body["messages"][0]["role"], "user");
        // claude-haiku-4-5 在白名单内：采样参数发送
        assert!(body["temperature"].is_number());
        assert!(body["top_p"].is_number());
    }

    #[test]
    fn test_stream_tool_call_aggregated_and_emitted_at_end() {
        let (port, rx) = spawn_mock(TOOL_USE_STREAM, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("claude-haiku-4-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let tools = vec![ToolDefinition {
            name: "get_current_time".to_string(),
            description: "获取当前时间".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];
        let (items, result) = collect_generate(
            &mut provider,
            &user_input("现在几点"),
            &tools,
            Arc::new(AtomicBool::new(false)),
        );

        assert_eq!(result.unwrap(), FinishReason::Eos);
        // 流末恰好一次完整 ToolCall，无文本增量
        assert_eq!(items.len(), 1);
        let OutputItem::ToolCall(call) = &items[0] else {
            panic!("期望 ToolCall，实际：{items:?}");
        };
        assert_eq!(call.name, "get_current_time");
        assert_eq!(call.id.as_deref(), Some("toolu_01ABC"));
        let args: serde_json::Value = serde_json::from_str(&call.arguments).unwrap();
        assert_eq!(args, serde_json::json!({"city": "上海"}));

        let req = rx.recv().unwrap();
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["tools"][0]["name"], "get_current_time");
    }

    #[test]
    fn test_tool_result_feedback_request_body() {
        let (port, rx) = spawn_mock(TEXT_STREAM, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("claude-haiku-4-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        // 模拟 Agent 回灌：assistant tool_use + tool_result 后的第二轮请求
        let input = vec![
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "现在几点")),
            InputItem::ToolCall(ToolCall {
                name: "get_current_time".to_string(),
                arguments: "{}".to_string(),
                id: Some("toolu_01ABC".to_string()),
            }),
            InputItem::ToolResult(ToolResult {
                id: "toolu_01ABC".to_string(),
                name: "get_current_time".to_string(),
                content: "2026-08-27T12:00:00+08:00".to_string(),
            }),
        ];
        let (_items, result) =
            collect_generate(&mut provider, &input, &[], Arc::new(AtomicBool::new(false)));
        assert!(result.is_ok());

        let req = rx.recv().unwrap();
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        // assistant 消息含 tool_use 块（id/name/input 完整回传）
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["id"], "toolu_01ABC");
        assert_eq!(
            body["messages"][1]["content"][0]["name"],
            "get_current_time"
        );
        // tool_result 在 user 消息中，tool_use_id 配对
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            body["messages"][2]["content"][0]["tool_use_id"],
            "toolu_01ABC"
        );
    }

    #[test]
    fn test_newer_model_omits_sampling_params() {
        let (port, rx) = spawn_mock(TEXT_STREAM, 200);
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("claude-opus-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let (_items, result) = collect_generate(
            &mut provider,
            &user_input("你好"),
            &[],
            Arc::new(AtomicBool::new(false)),
        );
        assert!(result.is_ok());

        let req = rx.recv().unwrap();
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["model"], "claude-opus-5");
        // 新一代模型：不发送采样参数（发送即 400）
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
    }

    #[test]
    fn test_prompt_cache_and_reasoning_request_body() {
        let (port, rx) = spawn_mock(TEXT_STREAM, 200);
        let mut cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("claude-sonnet-4-6".to_string()),
        );
        cfg.reasoning_effort = Some("low".to_string());
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let input = vec![
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "第一条")),
            InputItem::Message(LlmChatMessage::new(ChatRole::Assistant, "回复")),
            InputItem::Message(LlmChatMessage::new(ChatRole::User, "第二条")),
        ];
        let (_items, result) =
            collect_generate(&mut provider, &input, &[], Arc::new(AtomicBool::new(false)));
        assert!(result.is_ok());

        let req = rx.recv().unwrap();
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        // thinking：output_config.effort + adaptive thinking（sonnet-4-6 支持），
        // 且与采样参数互斥
        assert_eq!(body["output_config"]["effort"], "low");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_p").is_none());
        // prompt caching：仅末位消息带 ephemeral 断点
        assert!(!body["messages"][0].to_string().contains("ephemeral"));
        assert!(body["messages"][2].to_string().contains("ephemeral"));
    }

    #[test]
    fn test_401_maps_to_backend_unavailable() {
        let (port, _rx) = spawn_mock(
            "{\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid x-api-key\"}}",
            401,
        );
        let cfg = test_config(
            Some(format!("http://127.0.0.1:{port}")),
            Some("claude-haiku-4-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let (_items, result) = collect_generate(
            &mut provider,
            &user_input("你好"),
            &[],
            Arc::new(AtomicBool::new(false)),
        );
        let err = result.err().unwrap();
        assert!(
            matches!(err, LlmError::BackendUnavailable(_)),
            "实际错误：{err}"
        );
        assert!(err.to_string().contains("认证失败"), "实际错误：{err}");
    }

    #[test]
    fn test_cancel_before_request() {
        // 不可达地址 + 预设 cancel：应在发请求前短路返回 Cancelled
        let cfg = test_config(
            Some("http://127.0.0.1:1".to_string()),
            Some("claude-haiku-4-5".to_string()),
        );
        let mut provider = AnthropicProvider::new(&cfg).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let (items, result) = collect_generate(&mut provider, &user_input("你好"), &[], cancel);
        assert!(items.is_empty());
        assert_eq!(result.unwrap(), FinishReason::Cancelled);
    }
}
