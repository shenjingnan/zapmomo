/// dsh 桥事件：deepseek-harness 插件推送到 `/dsh/events` 的语义化任务事件。
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 任务事件（序列化为 kebab-case `type` 判别字段，与前端 `DshEventInfo` 对应）。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DshEvent {
    /// 会话 idle → running（dsh `agent/status`）
    TaskStarted {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// turn 结束且 reason.kind = completed
    TaskFinished {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// turn 结束且 reason.kind = error
    TaskFailed {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// turn 结束且 reason.kind 为 aborted/interrupted/max-tokens/blocked 等
    TaskInterrupted {
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// 插件心跳（控制事件）：dsh 插件启动即发 + 周期重发，桥据此判定「插件在线」。
    /// 只在 tauri 侧 `handle_dsh_event` 顶部拦截回写状态，不进节流/播报/历史管线。
    PluginHello,
}

impl DshEvent {
    /// 事件类型名（kebab-case，节流 key / 日志用）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TaskStarted { .. } => "task-started",
            Self::TaskFinished { .. } => "task-finished",
            Self::TaskFailed { .. } => "task-failed",
            Self::TaskInterrupted { .. } => "task-interrupted",
            Self::PluginHello => "plugin-hello",
        }
    }

    /// 会话 id（节流 key 用）。
    pub fn session_id(&self) -> &str {
        match self {
            Self::TaskStarted { session_id, .. }
            | Self::TaskFinished { session_id, .. }
            | Self::TaskFailed { session_id, .. }
            | Self::TaskInterrupted { session_id, .. } => session_id,
            // 心跳无会话语义；不进节流（tauri 侧提前拦截），此退化值仅满足接口
            Self::PluginHello => "",
        }
    }

    /// 任务标题（模板台词用）。
    pub fn title(&self) -> Option<&str> {
        match self {
            Self::TaskStarted { title, .. }
            | Self::TaskFinished { title, .. }
            | Self::TaskFailed { title, .. }
            | Self::TaskInterrupted { title, .. } => title.as_deref(),
            Self::PluginHello => None,
        }
    }
}

/// 宽容解析用的原始载荷：全字段可缺省且类型漂移不致命（`Value` 逐字段提取），
/// 未知 `type` 不报错（前向兼容）。
#[derive(Debug, Deserialize)]
struct RawEvent {
    #[serde(default)]
    r#type: Value,
    #[serde(default)]
    session_id: Value,
    #[serde(default)]
    title: Option<Value>,
    #[serde(default)]
    reason: Option<Value>,
    #[serde(default)]
    detail: Option<Value>,
}

/// 从 JSON 值中提取字符串（null / 类型漂移 → None，字段坏只丢字段不丢事件）。
fn value_as_string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// 规范化文本字段：trim 首尾空白（不进气泡文本）、截断 200 字符（多生产者护栏），
/// 空白视为缺失。
fn normalize_text(s: Option<String>) -> Option<String> {
    let s = s?.trim().to_owned();
    (!s.is_empty()).then(|| s.chars().take(200).collect())
}

/// 解析一条事件载荷（逐字段宽容：字段类型漂移只丢字段不丢事件）。
///
/// - 非法 JSON / 非 JSON 对象 → `Err`（HTTP 层回 400）
/// - `type` 缺失/null/非字符串或未知值 → `Ok(None)`（调用方记 debug 后忽略）
/// - 已知 `type` → 规范化为 [`DshEvent`]：文本字段 trim + 截断 200 字符、空白视为
///   缺失；`session_id` 类型漂移退化为空串（节流 key 退化为 `("", kind)`，属已知行为）
pub fn parse_event(body: &str) -> Result<Option<DshEvent>, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("事件载荷不是合法 JSON 对象: {e}"))?;
    if !value.is_object() {
        return Err(format!("事件载荷不是 JSON 对象: {value}"));
    }
    let RawEvent {
        r#type,
        session_id,
        title,
        reason,
        detail,
    } = serde_json::from_value(value).map_err(|e| format!("事件载荷不是合法 JSON 对象: {e}"))?;
    // `type` 非字符串（null/数字/对象）按未知类型处理，整条忽略而非报错
    let Some(r#type) = value_as_string(&r#type) else {
        return Ok(None);
    };
    let session_id = value_as_string(&session_id).unwrap_or_default();
    let title = normalize_text(title.as_ref().and_then(value_as_string));
    let reason = normalize_text(reason.as_ref().and_then(value_as_string));
    let detail = normalize_text(detail.as_ref().and_then(value_as_string));
    Ok(match r#type.as_str() {
        "task-started" => Some(DshEvent::TaskStarted { session_id, title }),
        "task-finished" => Some(DshEvent::TaskFinished {
            session_id,
            title,
            reason,
        }),
        "task-failed" => Some(DshEvent::TaskFailed {
            session_id,
            title,
            reason,
            detail,
        }),
        "task-interrupted" => Some(DshEvent::TaskInterrupted {
            session_id,
            title,
            reason,
        }),
        // 心跳：无字段语义，多余字段（如 session_id）忽略
        "plugin-hello" => Some(DshEvent::PluginHello),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_started_full() {
        let ev = parse_event(
            r#"{"type":"task-started","session_id":"s1","title":"修复登录超时","extra":"未知字段"}"#,
        )
        .unwrap()
        .expect("已知类型应返回 Some");
        assert_eq!(
            ev,
            DshEvent::TaskStarted {
                session_id: "s1".to_string(),
                title: Some("修复登录超时".to_string()),
            }
        );
        assert_eq!(ev.kind(), "task-started");
        assert_eq!(ev.session_id(), "s1");
        assert_eq!(ev.title(), Some("修复登录超时"));
    }

    #[test]
    fn test_parse_failed_truncates_detail() {
        let long = "x".repeat(300);
        let ev = parse_event(&format!(
            r#"{{"type":"task-failed","session_id":"s2","detail":"{long}"}}"#
        ))
        .unwrap()
        .unwrap();
        match ev {
            DshEvent::TaskFailed { detail, .. } => {
                assert_eq!(detail.as_deref().map(str::len), Some(200));
            }
            other => panic!("应为 TaskFailed: {other:?}"),
        }
    }

    #[test]
    fn test_parse_unknown_type_returns_none() {
        assert!(
            parse_event(r#"{"type":"todo-changed","session_id":"s"}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_parse_plugin_hello() {
        // 心跳：多余字段（session_id）忽略；无 session_id 也照常解析
        let ev = parse_event(r#"{"type":"plugin-hello","session_id":"plugin"}"#)
            .unwrap()
            .expect("心跳应返回 Some");
        assert_eq!(ev, DshEvent::PluginHello);
        assert_eq!(ev.kind(), "plugin-hello");
        let ev = parse_event(r#"{"type":"plugin-hello"}"#).unwrap().unwrap();
        assert_eq!(ev, DshEvent::PluginHello);
    }

    #[test]
    fn test_empty_title_treated_as_missing() {
        let ev = parse_event(r#"{"type":"task-started","session_id":"s","title":"  "}"#)
            .unwrap()
            .unwrap();
        assert_eq!(ev.title(), None);
    }

    #[test]
    fn test_parse_invalid_json_errs() {
        assert!(parse_event("不是json").is_err());
        assert!(parse_event(r#""裸字符串""#).is_err());
        // 数组载荷：Value 化后 serde 派生 visit_seq 被打开，对象守卫必须拦截
        assert!(parse_event("[1]").is_err());
        assert!(parse_event(r#"["task-started","s1"]"#).is_err());
    }

    #[test]
    fn test_all_kinds() {
        for (body, kind) in [
            (
                r#"{"type":"task-started","session_id":"s"}"#,
                "task-started",
            ),
            (
                r#"{"type":"task-finished","session_id":"s"}"#,
                "task-finished",
            ),
            (r#"{"type":"task-failed","session_id":"s"}"#, "task-failed"),
            (
                r#"{"type":"task-interrupted","session_id":"s"}"#,
                "task-interrupted",
            ),
            (r#"{"type":"plugin-hello"}"#, "plugin-hello"),
        ] {
            assert_eq!(parse_event(body).unwrap().unwrap().kind(), kind);
        }
    }

    #[test]
    fn test_serialize_type_tag_matches_kind() {
        // 判别串存在于三处（serde rename_all 推导 / kind() / parse_event match），
        // 此测试锁定序列化 tag 与 kind() 的一致性契约
        for ev in [
            DshEvent::TaskStarted {
                session_id: "s".to_string(),
                title: None,
            },
            DshEvent::TaskFinished {
                session_id: "s".to_string(),
                title: None,
                reason: None,
            },
            DshEvent::TaskFailed {
                session_id: "s".to_string(),
                title: None,
                reason: None,
                detail: None,
            },
            DshEvent::TaskInterrupted {
                session_id: "s".to_string(),
                title: None,
                reason: None,
            },
            DshEvent::PluginHello,
        ] {
            assert_eq!(
                serde_json::to_value(&ev).unwrap()["type"].as_str(),
                Some(ev.kind())
            );
        }
    }

    #[test]
    fn test_field_type_drift_only_drops_field() {
        // dsh 原生 turn/end 的 reason 是对象 {"kind":...}，类型漂移只丢字段不丢事件
        let ev = parse_event(
            r#"{"type":"task-finished","session_id":"s","title":5,"reason":{"kind":"completed"}}"#,
        )
        .unwrap()
        .unwrap();
        match ev {
            DshEvent::TaskFinished {
                session_id,
                title,
                reason,
            } => {
                assert_eq!(session_id, "s");
                assert_eq!(title, None);
                assert_eq!(reason, None);
            }
            other => panic!("应为 TaskFinished: {other:?}"),
        }
        // session_id 类型漂移 → 退化为空串，事件仍解析成功
        let ev = parse_event(r#"{"type":"task-started","session_id":123}"#)
            .unwrap()
            .unwrap();
        assert_eq!(ev.session_id(), "");
    }

    #[test]
    fn test_null_type_treated_as_unknown() {
        assert!(
            parse_event(r#"{"type":null,"session_id":"s"}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_all_fields_drift_still_delivers() {
        // 全字段漂移组合：session_id 数字/title 数字/reason 空对象 → 全部降级，事件仍到达
        let ev = parse_event(r#"{"type":"task-finished","session_id":123,"title":5,"reason":{}}"#)
            .unwrap()
            .unwrap();
        match ev {
            DshEvent::TaskFinished {
                session_id,
                title,
                reason,
            } => {
                assert_eq!(session_id, "");
                assert_eq!(title, None);
                assert_eq!(reason, None);
            }
            other => panic!("应为 TaskFinished: {other:?}"),
        }
    }

    #[test]
    fn test_empty_object_returns_none() {
        // 空对象全默认：type 为 null → 按未知类型忽略
        assert!(parse_event("{}").unwrap().is_none());
    }

    #[test]
    fn test_missing_session_id_defaults_empty() {
        // 已知行为：空 session_id 会让节流 key 退化为 ("", kind)，3s 窗口内不同任务互相吞
        let ev = parse_event(r#"{"type":"task-started"}"#).unwrap().unwrap();
        assert_eq!(ev.session_id(), "");
    }

    #[test]
    fn test_detail_truncation_cjk_safe() {
        // 按字符（非字节）截断：CJK 多字节字符不被切断、不产生 replacement char
        let long = "汉".repeat(300);
        let ev = parse_event(&format!(
            r#"{{"type":"task-failed","session_id":"s","detail":"{long}"}}"#
        ))
        .unwrap()
        .unwrap();
        match ev {
            DshEvent::TaskFailed { detail, .. } => {
                let d = detail.expect("CJK detail 截断后仍应保留");
                assert_eq!(d.chars().count(), 200);
                assert!(d.chars().all(|c| c == '汉'));
            }
            other => panic!("应为 TaskFailed: {other:?}"),
        }
    }
}
