/// dsh 事件的模板台词（固定模板起步；LLM 生成留待后续抽象）。
use super::event::DshEvent;
use std::sync::atomic::{AtomicU64, Ordering};

/// 有标题变体（`{t}` = 任务标题占位符）
const STARTED: &[&str] = &[
    "「{t}」开工啦，我会盯着你的～",
    "新任务「{t}」来了，冲鸭！",
    "收到，「{t}」跑起来了，去忙别的吧。",
];
/// 无标题变体
const STARTED_PLAIN: &[&str] = &[
    "任务开工啦，我会盯着你的～",
    "新任务跑起来了，冲鸭！",
    "收到收到，盯上了～",
];

const FINISHED: &[&str] = &[
    "「{t}」搞定啦！",
    "「{t}」跑完了，结果不错哦～",
    "叮～「{t}」完成了，夸夸你！",
];
const FINISHED_PLAIN: &[&str] = &["任务搞定啦！", "跑完了跑完了，一切正常～", "叮～任务完成！"];

const FAILED: &[&str] = &[
    "唔……「{t}」失败了，要不要看看日志？",
    "「{t}」出错了，抱抱你，别灰心。",
    "哎呀，「{t}」没跑成，检查一下？",
];
const FAILED_PLAIN: &[&str] = &[
    "唔……任务失败了，要不要看看日志？",
    "任务出错了，抱抱你。",
    "哎呀，没跑成，检查一下？",
];

const INTERRUPTED: &[&str] = &["「{t}」先停下来了～", "「{t}」被中断了，等你回来。"];
const INTERRUPTED_PLAIN: &[&str] = &["任务先停下来了～", "被中断了，等你回来。"];

/// roll 计数器：黄金比例散列（无 rand 依赖；测试用显式 roll 注入）。
static ROLL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 下一次 pick_line 用的 roll 值（0.0..1.0，单调循环不重复）。
pub fn next_roll() -> f32 {
    let n = ROLL_COUNTER.fetch_add(1, Ordering::Relaxed);
    ((n.wrapping_mul(2654435761) % 997) as f32) / 997.0
}

/// 按事件类型选一句台词。
///
/// `roll`（0.0..1.0）决定同列表内选哪句（越界 clamp）；有 `title` 用带标题变体。
pub fn pick_line(event: &DshEvent, roll: f32) -> String {
    let (titled, plain) = match event {
        DshEvent::TaskStarted { .. } => (STARTED, STARTED_PLAIN),
        DshEvent::TaskFinished { .. } => (FINISHED, FINISHED_PLAIN),
        DshEvent::TaskFailed { .. } => (FAILED, FAILED_PLAIN),
        DshEvent::TaskInterrupted { .. } => (INTERRUPTED, INTERRUPTED_PLAIN),
        // 不可达：心跳在桥 sink 已拦截，不进播报管线（穷尽性要求）
        DshEvent::PluginHello => (STARTED, STARTED_PLAIN),
    };
    let candidates: Vec<String> = match event.title() {
        Some(t) => titled.iter().map(|s| s.replace("{t}", t)).collect(),
        None => plain.iter().map(|s| s.to_string()).collect(),
    };
    let idx =
        ((roll.clamp(0.0, 0.9999) * candidates.len() as f32) as usize).min(candidates.len() - 1);
    candidates[idx].clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsh::event::DshEvent;

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
    fn test_pick_line_with_title_contains_title() {
        let line = pick_line(&ev("task-finished", Some("修复登录超时")), 0.0);
        assert!(line.contains("修复登录超时"), "台词应含标题: {line}");
        assert!(!line.contains("{t}"), "占位符应被替换: {line}");
    }

    #[test]
    fn test_pick_line_without_title_uses_plain() {
        let line = pick_line(&ev("task-finished", None), 0.0);
        assert!(!line.is_empty());
    }

    #[test]
    fn test_roll_selects_variants_and_clamps() {
        let e = ev("task-started", Some("T"));
        let first = pick_line(&e, 0.0);
        let last = pick_line(&e, 1.0);
        assert_eq!(pick_line(&e, -0.5), first, "roll 越界 clamp 到首句");
        assert_eq!(pick_line(&e, 5.0), last, "roll 越界 clamp 到末句");
    }

    #[test]
    fn test_next_roll_in_range() {
        for _ in 0..100 {
            let r = next_roll();
            assert!((0.0..1.0).contains(&r), "roll 越界: {r}");
        }
    }

    #[test]
    fn test_all_event_types_produce_non_empty() {
        for kind in [
            "task-started",
            "task-finished",
            "task-failed",
            "task-interrupted",
        ] {
            let line = pick_line(&ev(kind, Some("测试")), 0.5);
            assert!(!line.is_empty(), "{kind} 有标题应产出非空台词");
            assert!(!line.contains("{t}"), "{kind} 占位符应被替换");
            let line2 = pick_line(&ev(kind, None), 0.5);
            assert!(!line2.is_empty(), "{kind} 无标题应产出非空台词");
        }
    }
}
