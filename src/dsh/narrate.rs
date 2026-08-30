/// dsh 事件的 LLM 播报文案：薄字段事件 → LLM 输入构造 + 输出后处理 + 降级决策。
///
/// 设计（DSH_LLM_NARRATION_DESIGN.md）：dsh 事件只有 title/reason/detail 薄字段，
/// LLM 的职责是把事件转述成有陪伴感的一句话文案（替代固定模板），而非总结任务
/// 实质内容。全部纯函数，不触 IO，便于单测；引擎获取/流式消费在 src-tauri 侧
/// worker 完成。
use crate::dsh::event::DshEvent;
use crate::llm::LlmEngine;
use crate::llm::LlmEvent;
use crate::llm::types::{ChatMessage, ChatRole, FinishReason, GenParams, InputItem};
use crate::voice::sanitizer::sanitize_for_tts;
use crate::voice::thinking::ThinkingFilter;
use std::time::Duration;

/// 播报文案的 max_tokens。
///
/// 可见文案目标只有一句话，但推理型模型（如 deepseek-v4）会先输出思考块再给
/// 正文——预算太小会在思考阶段耗尽、正文零输出（表现为「清洗后无文本」回退
/// 模板）。放宽到 1024 给思考 + 正文都留空间；思考块由 [`ThinkingFilter`] 剥离，
/// 不进气泡/TTS。
const NARRATE_MAX_TOKENS: usize = 1024;

/// 单次播报生成的超时：远程 API 偶发挂起时不让事件无限等待，超时取消并降级。
pub const NARRATE_TIMEOUT: Duration = Duration::from_secs(15);

/// 播报专用 system prompt：一句话转述、纯文本、克制篇幅（对齐
/// [`crate::llm::config::default_system_prompt`] 的 TTS 减负约束），
/// 语气与 [`crate::dsh::lines`] 模板台词的陪伴感一致。
pub fn system_prompt() -> String {
    "你是 ZapMomo，用户的桌面 AI 伙伴。用户的开发任务状态刚刚变化，请以伙伴的\
     口吻用一句亲切自然的中文转述这件事，可以带一点情绪：任务完成就庆祝夸夸，\
     失败就温柔安慰。硬性要求：只输出这一句话本身，不要引号、前后缀或任何说明；\
     纯文本，不要 Markdown，不要 emoji 或颜文字；不超过 40 个字；只依据给出的\
     事件信息转述，不要编造任务细节或结果。"
        .to_string()
}

/// 事件状态的一句话描述（user 消息用）。
fn status_text(event: &DshEvent) -> &'static str {
    match event {
        DshEvent::TaskStarted { .. } => "任务已开始",
        DshEvent::TaskFinished { .. } => "任务已完成",
        DshEvent::TaskFailed { .. } => "任务失败",
        DshEvent::TaskInterrupted { .. } => "任务被中断",
        // 不可达：心跳在桥 sink 已拦截，不进 LLM 播报管线（穷尽性要求）
        DshEvent::PluginHello => "插件心跳",
    }
}

/// 「字段名：值」行（值为空白/缺失返回 `None`）。
fn field_line(name: &str, value: Option<&str>) -> Option<String> {
    let v = value?.trim();
    (!v.is_empty()).then(|| format!("{name}：{v}"))
}

/// 把事件拼成结构化 user 消息文本（缺失字段自动省略行；detail/reason 已在上游
/// `parse_event` 规范化为 trim + 截断 200，此处防御性再 trim 一次）。
pub fn build_user_text(event: &DshEvent) -> String {
    let mut lines = Vec::new();
    let title = event.title();
    if let Some(line) = field_line("任务标题", title) {
        lines.push(line);
    }
    lines.push(format!("事件：{}", status_text(event)));
    let (reason, detail) = match event {
        DshEvent::TaskStarted { .. } => (None, None),
        DshEvent::TaskFinished { reason, .. } => (reason.as_deref(), None),
        DshEvent::TaskFailed { reason, detail, .. } => (reason.as_deref(), detail.as_deref()),
        DshEvent::TaskInterrupted { reason, .. } => (reason.as_deref(), None),
        // 不可达：心跳不进 LLM 播报管线（穷尽性要求）
        DshEvent::PluginHello => (None, None),
    };
    if let Some(line) = field_line("结束原因", reason) {
        lines.push(line);
    }
    if let Some(line) = field_line("详情", detail) {
        lines.push(line);
    }
    lines.join("\n")
}

/// 构造传给 LLM 的输入：播报 system prompt + 事件 user 消息（单轮，无共享
/// history——不污染 voice 会话上下文）。
pub fn build_llm_input(event: &DshEvent) -> Vec<InputItem> {
    vec![
        InputItem::Message(ChatMessage::new(ChatRole::System, system_prompt())),
        InputItem::Message(ChatMessage::new(ChatRole::User, build_user_text(event))),
    ]
}

/// 播报生成参数：max_tokens 收紧为 [`NARRATE_MAX_TOKENS`]，其余继承会话配置。
pub fn gen_params(base: &GenParams) -> GenParams {
    GenParams {
        max_tokens: NARRATE_MAX_TOKENS,
        ..base.clone()
    }
}

/// 是否走 LLM 播报（否则回退模板台词）。任一条件不满足即降级：
/// dsh 侧开关 / LLM 侧开关 / 引擎就绪 / 引擎空闲（voice、GUI 生成中不抢引擎）。
pub fn should_use_llm(
    dsh_llm_enabled: bool,
    llm_enabled: bool,
    engine_ready: bool,
    engine_generating: bool,
) -> bool {
    dsh_llm_enabled && llm_enabled && engine_ready && !engine_generating
}

/// 单次 LLM 播报生成（阻塞到完成/失败/超时）。
///
/// 订阅引擎事件流 → 发起生成 → 流式吸收 token（过滤 `<think>` 块）→
/// `Finished` 后清洗产出。返回 `None`（调用方降级模板台词）的情形：
/// - `generate` 发起失败（引擎 `Busy`：voice/GUI 正在生成，或 worker 已退出）
/// - 生成错误 / 事件流断开（引擎被更换）
/// - 超过 `timeout` 未完成（主动 `cancel` 引擎后放弃）
/// - 被取消（`Finished(Cancelled)`）/ 清洗后无文本
pub fn generate_narration(
    engine: &LlmEngine,
    event: &DshEvent,
    llm_params: &GenParams,
    timeout: Duration,
) -> Option<String> {
    let rx = engine.subscribe();
    let params = gen_params(llm_params);
    if let Err(e) = engine.generate(build_llm_input(event), params) {
        tracing::info!("dsh LLM 播报生成发起失败（{e}），回退模板台词");
        return None;
    }
    let deadline = std::time::Instant::now() + timeout;
    let mut buf = NarrationBuffer::default();
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            tracing::warn!(
                "dsh LLM 播报超时（{}s），取消并回退模板台词",
                timeout.as_secs()
            );
            engine.cancel();
            return None;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(LlmEvent::Token(delta)) => buf.push(&delta.text),
            Ok(LlmEvent::Finished(FinishReason::Cancelled)) => {
                tracing::info!("dsh LLM 播报被取消，回退模板台词");
                return None;
            }
            Ok(LlmEvent::Finished(_)) => {
                let text = buf.finish();
                if text.is_none() {
                    tracing::info!("dsh LLM 播报清洗后无文本，回退模板台词");
                }
                return text;
            }
            Ok(LlmEvent::Error(e)) => {
                tracing::warn!("dsh LLM 播报生成失败（{e}），回退模板台词");
                return None;
            }
            // 加载/卸载状态与播报无关，跳过
            Ok(LlmEvent::Status { .. }) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                tracing::warn!(
                    "dsh LLM 播报事件流超时（{}s），取消并回退模板台词",
                    timeout.as_secs()
                );
                engine.cancel();
                return None;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                tracing::info!("dsh LLM 播报事件流断开（引擎已更换），回退模板台词");
                return None;
            }
        }
    }
}

/// 流式文案累积器：吸收 LLM token 增量（过滤 `<think>` 块），结束时清洗并产出
/// 最终文本。
///
/// 清洗用 [`sanitize_for_tts`]（markdown/emoji/fence 剥离），与 TTS 播报口径
/// 一致——气泡 / 语音 / 落盘共用这一份文本。清洗后为空（如整段是代码块）返回
/// `None`，调用方按生成失败降级模板台词。
#[derive(Default)]
pub struct NarrationBuffer {
    filter: ThinkingFilter,
    text: String,
}

impl NarrationBuffer {
    /// 吸收一段 token 增量（思考块内容不进文本）。
    pub fn push(&mut self, delta: &str) {
        let visible = self.filter.feed(delta);
        self.text.push_str(&visible);
    }

    /// 结束：冲刷过滤器残余 → 清洗 → trim；空返回 `None`（视为生成失败）。
    pub fn finish(&mut self) -> Option<String> {
        let tail = self.filter.finish();
        self.text.push_str(&tail);
        let cleaned = sanitize_for_tts(&self.text);
        let t = cleaned.trim().to_string();
        (!t.is_empty()).then_some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, title: Option<&str>) -> DshEvent {
        let session_id = "s".to_string();
        let title = title.map(str::to_string);
        match kind {
            "task-started" => DshEvent::TaskStarted { session_id, title },
            "task-finished" => DshEvent::TaskFinished {
                session_id,
                title,
                reason: None,
            },
            "task-failed" => DshEvent::TaskFailed {
                session_id,
                title,
                reason: None,
                detail: None,
            },
            _ => DshEvent::TaskInterrupted {
                session_id,
                title,
                reason: None,
            },
        }
    }

    #[test]
    fn test_system_prompt_constraints() {
        let p = system_prompt();
        // 纯文本约束与篇幅约束必须显式存在（TTS 朗读减负、气泡一行以内）
        assert!(p.contains("纯文本"));
        assert!(p.contains("40 个字"));
        assert!(p.contains("不要编造"));
    }

    #[test]
    fn test_user_text_full_fields() {
        let e = DshEvent::TaskFailed {
            session_id: "s".to_string(),
            title: Some("修复登录超时".to_string()),
            reason: Some("assertion failed".to_string()),
            detail: Some("at line 42".to_string()),
        };
        let t = build_user_text(&e);
        assert!(t.contains("任务标题：修复登录超时"), "{t}");
        assert!(t.contains("事件：任务失败"), "{t}");
        assert!(t.contains("结束原因：assertion failed"), "{t}");
        assert!(t.contains("详情：at line 42"), "{t}");
    }

    #[test]
    fn test_user_text_status_per_kind() {
        assert!(build_user_text(&ev("task-started", None)).contains("事件：任务已开始"));
        assert!(build_user_text(&ev("task-finished", None)).contains("事件：任务已完成"));
        assert!(build_user_text(&ev("task-failed", None)).contains("事件：任务失败"));
        assert!(build_user_text(&ev("task-interrupted", None)).contains("事件：任务被中断"));
    }

    #[test]
    fn test_user_text_omits_missing_fields() {
        // started 无 reason/detail；全缺时只留事件行
        let t = build_user_text(&ev("task-started", None));
        assert_eq!(t, "事件：任务已开始");
        // 空白 reason 视为缺失
        let e = DshEvent::TaskInterrupted {
            session_id: "s".to_string(),
            title: None,
            reason: Some("   ".to_string()),
        };
        assert_eq!(build_user_text(&e), "事件：任务被中断");
    }

    #[test]
    fn test_build_llm_input_system_then_user() {
        let input = build_llm_input(&ev("task-finished", Some("T")));
        assert_eq!(input.len(), 2);
        assert!(matches!(
            &input[0],
            InputItem::Message(m) if m.role == ChatRole::System && m.content == system_prompt()
        ));
        assert!(matches!(
            &input[1],
            InputItem::Message(m) if m.role == ChatRole::User && m.content.contains("任务已完成")
        ));
    }

    #[test]
    fn test_gen_params_clamps_max_tokens_only() {
        let mut base = GenParams::default();
        base.temperature = 0.3;
        let p = gen_params(&base);
        assert_eq!(p.max_tokens, NARRATE_MAX_TOKENS);
        assert_eq!(p.temperature, 0.3, "其余参数应继承会话配置");
    }

    #[test]
    fn test_should_use_llm_truth_table() {
        // 全条件满足才走 LLM
        assert!(should_use_llm(true, true, true, false));
        // 逐项降级：dsh 开关 / llm 开关 / 未就绪 / 生成中
        assert!(!should_use_llm(false, true, true, false));
        assert!(!should_use_llm(true, false, true, false));
        assert!(!should_use_llm(true, true, false, false));
        assert!(!should_use_llm(true, true, true, true));
    }

    #[test]
    fn test_narration_buffer_plain_text() {
        let mut b = NarrationBuffer::default();
        b.push("「修复登录超时」");
        b.push("搞定啦！");
        assert_eq!(b.finish().as_deref(), Some("「修复登录超时」搞定啦！"));
    }

    #[test]
    fn test_narration_buffer_filters_think_blocks() {
        let mut b = NarrationBuffer::default();
        b.push("<think>让我想想怎么夸</think>");
        b.push("任务搞定啦！");
        assert_eq!(b.finish().as_deref(), Some("任务搞定啦！"));
    }

    #[test]
    fn test_narration_buffer_split_think_tag_across_pushes() {
        // 标签跨 push 残片不得漏进正文
        let mut b = NarrationBuffer::default();
        b.push("<th");
        b.push("ink>思考</th");
        b.push("ink>正文");
        assert_eq!(b.finish().as_deref(), Some("正文"));
    }

    #[test]
    fn test_narration_buffer_strips_markdown_and_emoji() {
        let mut b = NarrationBuffer::default();
        b.push("## 任务完成 🎉 很棒哦～");
        assert_eq!(b.finish().as_deref(), Some("任务完成 很棒哦～"));
    }

    #[test]
    fn test_narration_buffer_empty_or_unspeakable_is_none() {
        // 空 → None
        let mut b = NarrationBuffer::default();
        assert_eq!(b.finish(), None);
        // 整段在代码围栏内（不可朗读）→ None（调用方降级模板）
        let mut b = NarrationBuffer::default();
        b.push("```rust\nfn main() {}\n```");
        assert_eq!(b.finish(), None);
        // 未闭合思考块整体丢弃 → None
        let mut b = NarrationBuffer::default();
        b.push("<think>只有思考没有正文");
        assert_eq!(b.finish(), None);
    }

    /// 构造指向本机不可达端口的 openai provider 引擎（连接立即被拒，不发真实请求）。
    ///
    /// `resolve` / `LlmEngine::new` 均为纯内存操作（不读 HOME），无需 temp home 隔离。
    fn unreachable_engine() -> LlmEngine {
        let settings = crate::config::settings::LlmSettings {
            enabled: Some(true),
            provider: Some("openai".to_string()),
            base_url: Some("http://127.0.0.1:9".to_string()),
            model: Some("test-model".to_string()),
            ..Default::default()
        };
        let cfg = crate::llm::config::resolve(Some(&settings)).unwrap();
        LlmEngine::new(cfg).unwrap()
    }

    #[test]
    fn test_generate_narration_busy_returns_none() {
        // 引擎生成互斥：已有生成在途时播报应让位（None → 调用方降级模板）
        let engine = unreachable_engine();
        let event = ev("task-finished", Some("T"));
        engine
            .generate(build_llm_input(&event), GenParams::default())
            .expect("首次生成应成功入队");
        let text = generate_narration(&engine, &event, &GenParams::default(), NARRATE_TIMEOUT);
        assert_eq!(text, None, "Busy 时应返回 None");
        engine.cancel();
    }

    #[test]
    fn test_generate_narration_unreachable_endpoint_returns_none() {
        // 远程端点不可达（连接立即拒绝）→ 事件流 Error → None
        let engine = unreachable_engine();
        let event = ev("task-failed", None);
        let text = generate_narration(&engine, &event, &GenParams::default(), NARRATE_TIMEOUT);
        assert_eq!(text, None, "生成失败应返回 None");
    }
}
