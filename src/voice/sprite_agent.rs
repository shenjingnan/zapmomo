//! 形象切换 subagent：LLM 回复结束后，后台单次调用自动决策是否切换角色形象。
//!
//! 动机：对话内 `set_character_sprite` 工具依赖主 LLM 自主调用，实测存在三大
//! 故障——模型倾向用台词「演」情绪而不调工具；max_tokens 截断挤掉工具调用；
//! 长文件名逐字复现失败。本模块把决策从主对话流剥离：
//!
//! - 主对话不再注册 `set_character_sprite`（`[llm] sprite_agent` 门控，
//!   见 [`crate::llm::tools::ToolRuntime::with_sprite_tool`]）；
//! - [`crate::voice::session::VoiceSession`] 在 `ReplyFinished` 后把
//!   「用户话 + 回复 + 当前形象 + 可用形象表」交给 [`SpriteAgentHandle`]；
//! - 决策成功且名字合法时调 [`crate::companion_sprites::apply_sprite`] 落地，
//!   前端经既有 `companion-sprite-changed` 事件无感生效。
//!
//! 失败即静默：解析失败 / 超时 / 网络错误 / 线程 panic 一律保持当前形象，
//! 绝不影响对话主链路。长文件名由「编号 + 名字」双通道绕开——模型只需
//! 回编号（如 `"3"`），宿主侧 [`SpriteCatalog::resolve`] 映射回真实 stem。
//!
//! P1 边界：不做冷却期 / 多轮上下文 / 本地小模型降级；形象为会话态
//! （重启回 default，与 [`crate::companion_sprites`] 现有语义一致）。

use crate::companion_sprites::{DEFAULT_SPRITE_NAME, SpriteInfo};
use crate::llm::types::{ChatMessage, ChatRole, GenParams, InputItem};
use crate::voice::thinking::ThinkingFilter;

/// 决策输出的 max_tokens：JSON 本体远小于此，但思考型模型会先吐 `<think>` 块，
/// 预算太小会在思考期耗尽、正文零输出（与主对话截断故障同型，故不设更小）。
pub const SPRITE_AGENT_MAX_TOKENS: usize = 512;
/// 单次决策的接收侧超时：远程 provider 的 HTTP 请求无自带 timeout，
/// 挂起请求由 `poll` 的 recv_timeout 兜底放弃（cancel 标志同步置位）。
pub const SPRITE_AGENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// 用户话注入 prompt 的最大字符数（超出截断，按 char 边界）。
const USER_TEXT_MAX_CHARS: usize = 1000;
/// 角色回复注入 prompt 的最大字符数（超出截断，按 char 边界）。
const REPLY_TEXT_MAX_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// 纯函数层：catalog / prompt / 解析（无 IO、无线程，可独立单测）
// ---------------------------------------------------------------------------

/// 可用形象表中的一个条目：`label`（编号）给模型看，`name` 是真实文件名 stem。
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteEntry {
    pub label: String,
    pub name: String,
}

/// 可用形象表：`entries[0]` 恒为 `default`（默认立绘），其余按 stem 升序编号。
///
/// 编号是模型回填的首选通道（无复现风险）；名字通道保留作兜底——短英文名
/// （happy / angry）抄对概率极高，白赚的容错。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpriteCatalog {
    entries: Vec<SpriteEntry>,
}

impl SpriteCatalog {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[SpriteEntry] {
        &self.entries
    }

    /// 三级解析：纯数字编号 → stem 大小写不敏感匹配（含 `default`）。
    /// 命中返回真实 stem；任何失败返回 `None`（宿主侧静默保持现状）。
    pub fn resolve(&self, raw: &str) -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        // 1) 编号通道（serde_json 数字统一转成字符串后仍走这里）
        if let Ok(idx) = raw.parse::<usize>() {
            return self.entries.get(idx).map(|e| e.name.clone());
        }
        // 2) 名字通道：与 apply_tool_call 的匹配语义对齐（ASCII 大小写不敏感）
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(raw))
            .map(|e| e.name.clone())
    }
}

/// 从磁盘枚举结果构建编号表（default 恒占 0，其余按传入顺序——
/// `list_active_sprites` 已按 stem 升序返回）。
pub fn build_catalog(sprites: &[SpriteInfo]) -> SpriteCatalog {
    let mut entries = vec![SpriteEntry {
        label: "0".to_string(),
        name: DEFAULT_SPRITE_NAME.to_string(),
    }];
    for s in sprites {
        if s.name.eq_ignore_ascii_case(DEFAULT_SPRITE_NAME) {
            continue; // sprites/default.png 是保留名冲突（切不过去），不进表
        }
        entries.push(SpriteEntry {
            label: entries.len().to_string(),
            name: s.name.clone(),
        });
    }
    SpriteCatalog { entries }
}

/// 一次决策的全部输入。
#[derive(Debug, Clone)]
pub struct DecisionInput {
    /// 本轮用户说的话
    pub user_text: String,
    /// 本轮角色的完整可见回复
    pub reply_text: String,
    /// 当前形象（canonical stem；会话开始为 default）
    pub current_sprite: String,
    /// 可用形象表（派发前由会话线程现读磁盘构建）
    pub catalog: SpriteCatalog,
}

/// 决策结果。`Switch` 携带 canonical stem。
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Keep,
    Switch(String),
}

/// 决策专用 system prompt：强偏置（保持是默认 / 台词情绪词不算 / 显式指令无条件切）。
pub fn system_prompt() -> String {
    "你是桌面 AI 伙伴的形象切换决策助手。你的唯一任务是根据每轮对话判断是否切换\
     角色的形象（表情立绘），不参与对话本身，不生成任何角色台词。\n\n\
     规则：\n\
     1. 保持当前形象是默认选项。只有当对话情绪发生明显且持续的变化时才切换。\n\
     2. 台词、玩笑、引用、角色扮演文字里出现的情绪词不构成切换理由；要看整轮\
     对话的真实情绪走向。\n\
     3. 用户明确要求切换表情或形象时（例如「换个开心的表情」「用悲伤的表情」），\
     必须无条件切换到最匹配的形象。\n\
     4. 只输出一行 JSON，不要输出任何其他文字：\n\
     保持现状：{\"action\":\"keep\"}\n\
     切换形象：{\"action\":\"switch\",\"sprite\":\"<可用形象表中的编号或名字>\"}"
        .to_string()
}

/// 拼装 user 消息：结构化区块（用户话 / 回复 / 当前形象 / 可用形象表）。
pub fn build_user_text(input: &DecisionInput) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "【用户说的话】{}\n",
        truncate_chars(&input.user_text, USER_TEXT_MAX_CHARS)
    ));
    out.push_str(&format!(
        "【角色回复】{}\n",
        truncate_chars(&input.reply_text, REPLY_TEXT_MAX_CHARS)
    ));
    out.push_str(&format!("【当前形象】{}\n", input.current_sprite));
    out.push_str("【可用形象表】\n");
    for e in input.catalog.entries() {
        if e.label == "0" {
            out.push_str(&format!("{} = {}（默认立绘）\n", e.label, e.name));
        } else {
            out.push_str(&format!("{} = {}\n", e.label, e.name));
        }
    }
    out
}

/// 决策调用的 LLM 输入：`[System, User]` 两消息单轮（不共享主对话 history，
/// 自带 system 消息，provider 不会重复注入配置的 system_prompt）。
pub fn build_llm_input(input: &DecisionInput) -> Vec<InputItem> {
    vec![
        InputItem::Message(ChatMessage::new(ChatRole::System, system_prompt())),
        InputItem::Message(ChatMessage::new(ChatRole::User, build_user_text(input))),
    ]
}

/// 决策专用采样参数：max_tokens 收紧、temperature 归零（「保持是默认」的偏置
/// 稳定可复现），其余继承主配置。
pub fn subagent_params(base: &GenParams) -> GenParams {
    GenParams {
        max_tokens: SPRITE_AGENT_MAX_TOKENS,
        temperature: 0.0,
        ..base.clone()
    }
}

/// 解析模型输出为决策。
///
/// 容忍：```` ```json ```` 围栏、JSON 夹在寒暄文字中、`<think>` 思考块、
/// `sprite` 为编号（数字或字符串）或名字、`action` 写作 `decision`。
/// 返回 `None` 表示无法得出可靠决策（宿主侧等效 keep，静默保持现状）。
pub fn parse_decision(raw: &str, catalog: &SpriteCatalog, _current: &str) -> Option<Decision> {
    // 剥思考块：思考型模型正文前可能混入 `<think>...</think>`
    let mut filter = ThinkingFilter::default();
    let visible = format!("{}{}", filter.feed(raw), filter.finish());
    let json = extract_json(&visible)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let obj = value.as_object()?;

    let action = obj
        .get("action")
        .or_else(|| obj.get("decision"))?
        .as_str()?
        .trim()
        .to_ascii_lowercase();
    match action.as_str() {
        "keep" => Some(Decision::Keep),
        "switch" | "change" | "set" => {
            let sprite_val = obj.get("sprite").or_else(|| obj.get("name"))?;
            let raw = match sprite_val {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => return None,
            };
            let stem = catalog.resolve(&raw)?;
            Some(Decision::Switch(stem))
        }
        _ => None,
    }
}

/// 同名折叠：决策结果与当前形象一致（忽略大小写）时折叠为 `Keep`，
/// 避免重复切换事件触发前端无意义的重载。
pub fn fold_noop(decision: Decision, current: &str) -> Decision {
    match decision {
        Decision::Switch(name) if name.eq_ignore_ascii_case(current.trim()) => Decision::Keep,
        other => other,
    }
}

/// 从模型输出中提取第一个 `{` 到最后一个 `}` 的片段（容忍围栏与夹带文字）。
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

/// 按 char 边界截断（绝不留下半个多字节字符），超长时附省略标记。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}…（已截断）")
}

// ---------------------------------------------------------------------------
// IO 层：SpriteAgentHandle（每轮临时 provider + 专用线程，用完即弃）
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::llm::config::ResolvedLlmConfig;
use crate::llm::types::OutputItem;

/// 形象决策的后台句柄：`VoiceSession` 持有，主循环每轮 [`poll`](Self::poll)。
///
/// 每次 [`dispatch`](Self::dispatch) spawn 一个 `"sprite-agent"` 线程，线程内
/// 临时 [`create_provider`](crate::llm::create_provider)（`LlmProvider` 无
/// `Send`，必须在目标线程内构建），用完即弃——不占用主 `LlmEngine`（避免
/// 与下一轮对话的 `Busy` 互斥冲突），崩溃隔离最好（线程 panic 只损失当轮）。
pub struct SpriteAgentHandle {
    rx: mpsc::Receiver<Option<Decision>>,
    /// 派发时的会话代数：`poll(current_gen)` 不匹配则丢弃过期结果
    /// （打断 / 新一轮已开始，见 `session.rs` 的 gen 递增点）
    dispatch_gen: u64,
    /// 在途任务取消标志：`invalidate` / 新派发时置位，让 provider 尽早中止
    cancel: Option<Arc<AtomicBool>>,
    /// 本轮决策的放弃时限（接收侧超时；远程 provider 的 HTTP 无自带 timeout）
    deadline: Option<Instant>,
    timeout: std::time::Duration,
}

impl Default for SpriteAgentHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SpriteAgentHandle {
    pub fn new() -> Self {
        Self::with_timeout(SPRITE_AGENT_TIMEOUT)
    }

    /// 超时可注入（测试用短超时验证接收侧兜底）。
    pub fn with_timeout(timeout: std::time::Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        drop(tx); // 初始 Disconnected：poll 直接 None
        Self {
            rx,
            dispatch_gen: 0,
            cancel: None,
            deadline: None,
            timeout,
        }
    }

    /// 派发一次决策：作废在途任务，spawn 新线程执行单次 LLM 调用。
    ///
    /// `gen` 传会话当前代数，`poll` 时比对；catalog 应由调用方（会话线程）
    /// 现读磁盘构建后放入 `input`。
    pub fn dispatch(
        &mut self,
        dispatch_gen: u64,
        llm_config: &ResolvedLlmConfig,
        input: DecisionInput,
    ) {
        self.invalidate();
        let (tx, rx) = mpsc::channel();
        self.rx = rx;
        self.dispatch_gen = dispatch_gen;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.deadline = Some(Instant::now() + self.timeout);
        let cfg = llm_config.clone();
        let spawn = std::thread::Builder::new()
            .name("sprite-agent".to_string())
            .spawn(move || {
                let decision = run_decision(&cfg, &input, cancel);
                let _ = tx.send(decision);
            });
        if spawn.is_err() {
            // rx 已随闭包创建但线程未起：永远 Disconnected → poll 返回 None
            tracing::debug!("形象决策线程创建失败，本轮保持当前形象");
        }
    }

    /// 主循环轮询（非阻塞）：返回本轮已就绪的决策；无 / 过期 / 失败 → `None`。
    ///
    /// `None` 对宿主只有一个含义：本轮无可执行的切换（保持现状）。
    pub fn poll(&mut self, current_gen: u64) -> Option<Decision> {
        match self.rx.try_recv() {
            Ok(maybe) => {
                self.cancel = None;
                self.deadline = None;
                if self.dispatch_gen != current_gen {
                    return None; // 过期决策（打断 / 新一轮已开始）
                }
                maybe
            }
            Err(mpsc::TryRecvError::Empty) => {
                // 接收侧超时兜底：远程 provider 的 HTTP 请求无自带 timeout，
                // 挂起时置位 cancel 让流式请求尽早中止
                if let Some(deadline) = self.deadline
                    && Instant::now() >= deadline
                {
                    tracing::debug!("形象决策超时（{:?}），放弃本轮并取消在途请求", self.timeout);
                    self.invalidate();
                }
                None
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }

    /// 作废在途决策：置位 cancel 并清空时限（不替换 rx，迟到结果由 gen 丢弃）。
    pub fn invalidate(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.cancel = None;
        self.deadline = None;
    }
}

/// 线程体：临时 provider → 单轮 `generate`（无工具）→ 解析决策。
/// 任何一步失败返回 `None`（静默保持现状，绝不影响对话主链路）。
fn run_decision(
    cfg: &ResolvedLlmConfig,
    input: &DecisionInput,
    cancel: Arc<AtomicBool>,
) -> Option<Decision> {
    let mut provider = match crate::llm::create_provider(cfg.clone()) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("形象决策 provider 创建失败（{e}），保持当前形象");
            return None;
        }
    };
    if let Err(e) = provider.load() {
        tracing::debug!("形象决策 provider 加载失败（{e}），保持当前形象");
        return None;
    }
    let items = build_llm_input(input);
    let params = subagent_params(&cfg.params);

    let mut text = String::new();
    let mut filter = ThinkingFilter::default();
    let result = {
        let mut emit = |item: OutputItem| {
            if let OutputItem::MessageDelta(delta) = item {
                text.push_str(&filter.feed(&delta.text));
            }
        };
        provider.generate(&items, &[], &params, &mut emit, cancel)
    };
    text.push_str(&filter.finish());
    if let Err(e) = result {
        tracing::debug!("形象决策生成失败（{e}），保持当前形象");
        return None;
    }
    let decision = parse_decision(&text, &input.catalog, &input.current_sprite);
    if decision.is_none() {
        tracing::debug!("形象决策输出无法解析（{text:.200}），保持当前形象");
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三形象表：default(0) / angry(1) / happy(2) / 长中文名(3)。
    fn catalog() -> SpriteCatalog {
        build_catalog(&[
            SpriteInfo {
                name: "angry".to_string(),
                path: std::path::PathBuf::from("/x/angry.png"),
            },
            SpriteInfo {
                name: "happy".to_string(),
                path: std::path::PathBuf::from("/x/happy.png"),
            },
            SpriteInfo {
                name: "如果芙宁娜又悲又愤请使用这个表情".to_string(),
                path: std::path::PathBuf::from("/x/如果芙宁娜又悲又愤请使用这个表情.png"),
            },
        ])
    }

    // ---------- build_catalog ----------

    #[test]
    fn test_build_catalog_default_first_and_labels_sequential() {
        let c = catalog();
        assert_eq!(c.entries()[0].name, "default");
        assert_eq!(c.entries()[0].label, "0");
        let labels: Vec<&str> = c.entries().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["0", "1", "2", "3"], "编号连续");
        // stem 保留原大小写与原文
        assert_eq!(c.entries()[3].name, "如果芙宁娜又悲又愤请使用这个表情");
    }

    #[test]
    fn test_build_catalog_skips_default_conflict_stem() {
        // sprites/default.png 是保留名冲突（apply_tool_call 永远路由到 character.png），
        // 不应进表占用编号
        let c = build_catalog(&[SpriteInfo {
            name: "default".to_string(),
            path: std::path::PathBuf::from("/x/sprites/default.png"),
        }]);
        assert_eq!(c.entries().len(), 1, "只剩内置 default");
        assert_eq!(c.entries()[0].name, "default");
    }

    // ---------- resolve ----------

    #[test]
    fn test_resolve_three_channels() {
        let c = catalog();
        // 编号通道
        assert_eq!(c.resolve("0"), Some("default".to_string()));
        assert_eq!(c.resolve("1"), Some("angry".to_string()));
        assert_eq!(
            c.resolve("3"),
            Some("如果芙宁娜又悲又愤请使用这个表情".to_string())
        );
        // 名字通道（大小写不敏感）
        assert_eq!(c.resolve("happy"), Some("happy".to_string()));
        assert_eq!(c.resolve("HAPPY"), Some("happy".to_string()));
        assert_eq!(c.resolve("  angry  "), Some("angry".to_string()), "trim");
        assert_eq!(c.resolve("default"), Some("default".to_string()));
        // 长中文名精确匹配
        assert_eq!(
            c.resolve("如果芙宁娜又悲又愤请使用这个表情"),
            Some("如果芙宁娜又悲又愤请使用这个表情".to_string())
        );
    }

    #[test]
    fn test_resolve_failures_return_none() {
        let c = catalog();
        for bad in ["99", "-1", "nope", "", "   ", " happy extra"] {
            assert_eq!(c.resolve(bad), None, "应拒绝：{bad:?}");
        }
    }

    // ---------- parse_decision ----------

    #[test]
    fn test_parse_keep_forms() {
        let c = catalog();
        for raw in [
            r#"{"action":"keep"}"#,
            r#"{"decision":"keep"}"#,
            "```json\n{\"action\": \"keep\"}\n```",
            "好的，我保持这个形象。{\"action\":\"keep\"}",
            "{\"action\":\"KEEP\"}",
        ] {
            assert_eq!(
                parse_decision(raw, &c, "default"),
                Some(Decision::Keep),
                "{raw}"
            );
        }
    }

    #[test]
    fn test_parse_switch_number_name_and_fenced() {
        let c = catalog();
        // sprite 为数字
        assert_eq!(
            parse_decision(r#"{"action":"switch","sprite":2}"#, &c, "default"),
            Some(Decision::Switch("happy".to_string()))
        );
        // sprite 为编号字符串
        assert_eq!(
            parse_decision(r#"{"action":"switch","sprite":"1"}"#, &c, "default"),
            Some(Decision::Switch("angry".to_string()))
        );
        // sprite 为名字（长中文名精确复现）
        assert_eq!(
            parse_decision(
                r#"{"action":"switch","sprite":"如果芙宁娜又悲又愤请使用这个表情"}"#,
                &c,
                "default"
            ),
            Some(Decision::Switch(
                "如果芙宁娜又悲又愤请使用这个表情".to_string()
            ))
        );
        // 围栏 + 换行 + 夹带文字
        assert_eq!(
            parse_decision(
                "我换个表情！\n```json\n{\"action\":\"switch\",\"sprite\":\"happy\"}\n```",
                &c,
                "default"
            ),
            Some(Decision::Switch("happy".to_string()))
        );
        // action 同义字段 / sprite 同义字段
        assert_eq!(
            parse_decision(r#"{"decision":"switch","name":"happy"}"#, &c, "default"),
            Some(Decision::Switch("happy".to_string()))
        );
    }

    #[test]
    fn test_parse_think_block_stripped() {
        let c = catalog();
        let raw =
            "<think>用户似乎生气了，我该切 angry</think>{\"action\":\"switch\",\"sprite\":\"1\"}";
        assert_eq!(
            parse_decision(raw, &c, "default"),
            Some(Decision::Switch("angry".to_string()))
        );
    }

    #[test]
    fn test_parse_failures_return_none() {
        let c = catalog();
        for raw in [
            r#"{"action":"switch","sprite":"99"}"#,   // 越界编号
            r#"{"action":"switch","sprite":"nope"}"#, // 未知名
            r#"{"action":"switch"}"#,                 // 缺 sprite
            r#"{"action":"dance","sprite":"happy"}"#, // 未知 action
            "保持现状，不换表情",                     // 纯文本无 JSON（宿主等效 keep）
            "",                                       // 空输出
            "{broken json",                           // 坏 JSON
        ] {
            assert_eq!(parse_decision(raw, &c, "default"), None, "{raw}");
        }
    }

    #[test]
    fn test_parse_long_name_via_number_is_core_acceptance() {
        // 核心验收：长中文文件名场景，模型回编号即可命中
        let c = catalog();
        let raw = r#"{"action":"switch","sprite":"3"}"#;
        assert_eq!(
            parse_decision(raw, &c, "default"),
            Some(Decision::Switch(
                "如果芙宁娜又悲又愤请使用这个表情".to_string()
            ))
        );
    }

    // ---------- fold_noop ----------

    #[test]
    fn test_fold_noop_folds_same_name() {
        assert_eq!(
            fold_noop(Decision::Switch("happy".into()), "happy"),
            Decision::Keep
        );
        assert_eq!(
            fold_noop(Decision::Switch("HAPPY".into()), "happy"),
            Decision::Keep
        );
        assert_eq!(
            fold_noop(Decision::Switch("happy".into()), " happy "),
            Decision::Keep
        );
        assert_eq!(
            fold_noop(Decision::Switch("sad".into()), "happy"),
            Decision::Switch("sad".to_string()),
            "不同名不折叠"
        );
        assert_eq!(fold_noop(Decision::Keep, "happy"), Decision::Keep);
    }

    // ---------- build_user_text / build_llm_input / params / prompt ----------

    #[test]
    fn test_build_user_text_blocks_and_truncation() {
        let c = catalog();
        let long_reply = "啊".repeat(REPLY_TEXT_MAX_CHARS + 100);
        let input = DecisionInput {
            user_text: "你好生气啊".to_string(),
            reply_text: long_reply,
            current_sprite: "happy".to_string(),
            catalog: c,
        };
        let text = build_user_text(&input);
        assert!(text.contains("【用户说的话】你好生气啊"));
        assert!(text.contains("【当前形象】happy"));
        assert!(text.contains("0 = default（默认立绘）"));
        assert!(text.contains("3 = 如果芙宁娜又悲又愤请使用这个表情"));
        // 超长截断：不留半个多字节字符，附省略标记
        assert!(text.contains("【角色回复】"), "区块存在");
        assert!(text.contains("…（已截断）"));
        let reply_line = text
            .lines()
            .find(|l| l.starts_with("【角色回复】"))
            .unwrap();
        let body = reply_line.trim_start_matches("【角色回复】");
        assert_eq!(
            body.chars().count(),
            REPLY_TEXT_MAX_CHARS + "…（已截断）".chars().count()
        );
    }

    #[test]
    fn test_build_llm_input_system_plus_user() {
        let input = DecisionInput {
            user_text: "u".to_string(),
            reply_text: "r".to_string(),
            current_sprite: "default".to_string(),
            catalog: catalog(),
        };
        let items = build_llm_input(&input);
        assert_eq!(items.len(), 2);
        let InputItem::Message(sys) = &items[0] else {
            panic!("首条应是消息");
        };
        assert_eq!(sys.role, ChatRole::System);
        assert!(sys.content.contains("形象切换决策助手"));
        let InputItem::Message(user) = &items[1] else {
            panic!("次条应是消息");
        };
        assert_eq!(user.role, ChatRole::User);
        assert!(user.content.contains("【可用形象表】"));
    }

    #[test]
    fn test_subagent_params_tighten_and_inherit() {
        let mut base = GenParams::default();
        base.max_tokens = 4096;
        base.top_p = 0.9;
        let p = subagent_params(&base);
        assert_eq!(p.max_tokens, SPRITE_AGENT_MAX_TOKENS);
        assert_eq!(p.temperature, 0.0);
        assert_eq!(p.top_p, 0.9, "其余参数继承主配置");
    }

    #[test]
    fn test_system_prompt_bias_keywords() {
        let p = system_prompt();
        assert!(p.contains("保持当前形象是默认选项"), "强偏置：默认保持");
        assert!(p.contains("无条件切换"), "强偏置：显式指令必须切");
        assert!(p.contains("JSON"), "输出契约");
    }

    // ---------- IO 层：dispatch / poll / 降级 ----------

    use std::time::Duration;

    use crate::companion_sprites::SpriteEvent;
    use crate::config::settings::LlmSettings;
    use crate::llm::config::resolve;

    /// 启动只服务一次请求的 mock server，返回给定 SSE body。
    /// 返回 (端口, 请求接收端)：接收端收到 (路径, 请求体)。
    fn spawn_mock(body: &'static str) -> (u16, mpsc::Receiver<(String, String)>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!("显式绑定 127.0.0.1"),
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(mut req) = server.recv() {
                let url = req.url().to_string();
                let mut req_body = String::new();
                req.as_reader().read_to_string(&mut req_body).ok();
                let _ = tx.send((url, req_body));
                let header =
                    tiny_http::Header::from_bytes("Content-Type", "text/event-stream").unwrap();
                let resp = tiny_http::Response::from_string(body).with_header(header);
                req.respond(resp).ok();
            }
        });
        (port, rx)
    }

    /// 构造两 chunk 的 OpenAI 兼容 SSE：一段 content + finish_reason。
    fn sse_body(content: &str, finish: &str) -> String {
        format!(
            "data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n\
             data: {{\"id\":\"1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\n\
             data: [DONE]\n\n"
        )
    }

    /// 决策 JSON 文本（嵌入 SSE content 前已按 JSON 字符串转义）。
    fn decision_json(action: &str, sprite: Option<&str>) -> String {
        match sprite {
            Some(s) => format!("{{\\\"action\\\":\\\"{action}\\\",\\\"sprite\\\":\\\"{s}\\\"}}"),
            None => format!("{{\\\"action\\\":\\\"{action}\\\"}}"),
        }
    }

    /// 指向 mock 端口的 ResolvedLlmConfig。
    fn mock_llm_config(port: u16) -> ResolvedLlmConfig {
        resolve(Some(&LlmSettings {
            enabled: Some(true),
            base_url: Some(format!("http://127.0.0.1:{port}/v1")),
            api_key: Some("test-key".to_string()),
            model: Some("test-model".to_string()),
            ..Default::default()
        }))
        .unwrap()
    }

    /// 轮询直到拿到结果或超时（粒度 10ms，模拟主循环 poll 节奏）。
    fn wait_decision(
        h: &mut SpriteAgentHandle,
        dispatch_gen: u64,
        max: Duration,
    ) -> Option<Decision> {
        let deadline = Instant::now() + max;
        loop {
            match h.poll(dispatch_gen) {
                Some(d) => return Some(d),
                None if Instant::now() >= deadline => return None,
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    fn decision_input(catalog: SpriteCatalog) -> DecisionInput {
        DecisionInput {
            user_text: "你好生气啊".to_string(),
            reply_text: "哼，谁让你惹我了。".to_string(),
            current_sprite: "default".to_string(),
            catalog,
        }
    }

    #[test]
    fn test_unreachable_endpoint_degrades_to_none() {
        // 不可达端口：连接立即拒绝（不发真实请求）→ 决策 None
        let cfg = resolve(Some(&LlmSettings {
            enabled: Some(true),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            model: Some("m".to_string()),
            ..Default::default()
        }))
        .unwrap();
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_millis(1500));
        h.dispatch(1, &cfg, decision_input(catalog()));
        assert_eq!(wait_decision(&mut h, 1, Duration::from_secs(5)), None);
    }

    #[test]
    fn test_create_provider_failure_degrades_to_none() {
        // 无 base_url / model → provider 创建失败 → None
        let cfg = resolve(Some(&LlmSettings {
            enabled: Some(true),
            ..Default::default()
        }))
        .unwrap();
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_millis(1500));
        h.dispatch(1, &cfg, decision_input(catalog()));
        assert_eq!(wait_decision(&mut h, 1, Duration::from_secs(5)), None);
    }

    #[test]
    fn test_never_responding_mock_times_out_and_handle_reusable() {
        // mock 收请求后永不应答 → 接收侧超时兜底 → None；之后 handle 可复用
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[cfg(unix)]
            tiny_http::ListenAddr::Unix(_) => unreachable!(),
        };
        std::thread::spawn(move || {
            // 收下请求但故意不 respond（模拟挂起的 HTTP）
            let _ = server.recv();
            std::thread::sleep(Duration::from_secs(30));
        });
        let cfg = mock_llm_config(port);
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_millis(200));
        h.dispatch(1, &cfg, decision_input(catalog()));
        // 超时（200ms）后 poll 放弃：等 1s 必为 None
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(h.poll(1), None, "超时后应放弃本轮");
        // handle 可复用：换正常 mock 重新派发
        let body: &'static str =
            Box::leak(sse_body(&decision_json("switch", Some("1")), "stop").into_boxed_str());
        let (port2, _rx) = spawn_mock(body);
        h.dispatch(2, &mock_llm_config(port2), decision_input(catalog()));
        assert_eq!(
            wait_decision(&mut h, 2, Duration::from_secs(5)),
            Some(Decision::Switch("angry".to_string())),
            "超时后 handle 应可继续派发"
        );
    }

    #[test]
    fn test_mock_switch_decision_and_request_shape() {
        let body: &'static str =
            Box::leak(sse_body(&decision_json("switch", Some("2")), "stop").into_boxed_str());
        let (port, rx) = spawn_mock(body);
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_secs(3));
        h.dispatch(1, &mock_llm_config(port), decision_input(catalog()));
        assert_eq!(
            wait_decision(&mut h, 1, Duration::from_secs(5)),
            Some(Decision::Switch("happy".to_string())),
            "编号 2 → happy"
        );

        // 请求体契约：单轮、无 tools、system 消息在前、参数收紧
        let (_, req_body) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&req_body).unwrap();
        assert!(
            v.get("tools")
                .map_or(true, |t| t.as_array().map_or(true, |a| a.is_empty())),
            "决策调用不应携带工具：{req_body}"
        );
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "单轮 [system, user]");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(v["max_tokens"], 512);
        assert_eq!(v["temperature"], 0.0);
    }

    #[test]
    fn test_maxtokens_finish_still_parsed() {
        // MaxTokens 截断结束：只要正文 JSON 完整就照常解析（plan：仍走解析）
        let body: &'static str =
            Box::leak(sse_body(&decision_json("keep", None), "length").into_boxed_str());
        let (port, _rx) = spawn_mock(body);
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_secs(3));
        h.dispatch(1, &mock_llm_config(port), decision_input(catalog()));
        assert_eq!(
            wait_decision(&mut h, 1, Duration::from_secs(5)),
            Some(Decision::Keep)
        );
    }

    #[test]
    fn test_malformed_output_degrades_and_reusable() {
        // 畸形输出（解析失败）→ None；handle 可复用
        let body: &'static str = Box::leak(sse_body("不是JSON的输出", "stop").into_boxed_str());
        let (port, _rx) = spawn_mock(body);
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_secs(3));
        h.dispatch(1, &mock_llm_config(port), decision_input(catalog()));
        assert_eq!(wait_decision(&mut h, 1, Duration::from_secs(5)), None);

        let body2: &'static str =
            Box::leak(sse_body(&decision_json("switch", Some("1")), "stop").into_boxed_str());
        let (port2, _rx2) = spawn_mock(body2);
        h.dispatch(2, &mock_llm_config(port2), decision_input(catalog()));
        assert_eq!(
            wait_decision(&mut h, 2, Duration::from_secs(5)),
            Some(Decision::Switch("angry".to_string()))
        );
    }

    #[test]
    fn test_stale_gen_discarded() {
        let body: &'static str =
            Box::leak(sse_body(&decision_json("switch", Some("1")), "stop").into_boxed_str());
        let (port, _rx) = spawn_mock(body);
        let mut h = SpriteAgentHandle::with_timeout(Duration::from_secs(3));
        h.dispatch(1, &mock_llm_config(port), decision_input(catalog()));
        // 会话代数已推进（新一轮开始 / 打断）：迟到决策被丢弃
        assert_eq!(wait_decision(&mut h, 2, Duration::from_secs(5)), None);
    }

    // ---------- 集成：磁盘角色包 + 真实枚举 + SpriteEvent ----------

    /// 构造最小合法角色包（character.md + character.png + sprites/ 三图）。
    fn import_active_pack_with_sprites(home: &std::path::Path) {
        let src = home.join("furina");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("character.md"), "# 芙宁娜\n\n你是芙宁娜。\n").unwrap();
        std::fs::write(src.join("character.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
        std::fs::create_dir_all(src.join("sprites")).unwrap();
        std::fs::write(src.join("sprites/happy.png"), b"png").unwrap();
        std::fs::write(src.join("sprites/angry.png"), b"png").unwrap();
        std::fs::write(src.join("sprites/sad.png"), b"png").unwrap();
        crate::companion::import_character_from_dir(&src).unwrap();
    }

    #[test]
    fn test_dispatch_to_apply_sprite_notifies_frontend_event() {
        crate::test_util::run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let (tx, rx) = mpsc::channel();
            crate::companion_sprites::register_notifier(tx);

            // 真实枚举（磁盘）→ 编号表
            let sprites = crate::companion_sprites::list_active_sprites();
            let catalog = build_catalog(&sprites);

            let body: &'static str =
                Box::leak(sse_body(&decision_json("switch", Some("1")), "stop").into_boxed_str());
            let (port, _req_rx) = spawn_mock(body);
            let mut h = SpriteAgentHandle::with_timeout(Duration::from_secs(3));
            h.dispatch(1, &mock_llm_config(port), decision_input(catalog));

            let decision = wait_decision(&mut h, 1, Duration::from_secs(5)).unwrap();
            let stem = match fold_noop(decision, "default") {
                Decision::Keep => panic!("应为切换决策"),
                Decision::Switch(s) => s,
            };
            let applied = crate::companion_sprites::apply_sprite(&stem);
            assert_eq!(
                applied.ok().as_deref(),
                Some("angry"),
                "编号 1 → angry 落地"
            );

            let ev = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let SpriteEvent { name, .. } = ev;
            assert_eq!(name, "angry", "前端事件应携带 canonical stem");

            crate::companion_sprites::reset_notifier_for_test();
        });
    }
}
