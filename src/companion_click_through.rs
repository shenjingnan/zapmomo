//! 角色窗口智能穿透的纯逻辑层。
//!
//! 智能穿透 = 光标落在角色不透明区域上时窗口接收鼠标，其余区域整窗穿透
//! （WebView 架构下无 OS 级逐像素命中，区域近似是业界通行做法，见
//! docs/plans/2026-08-28-companion-smart-click-through-design.md）。本模块只放**可纯计算**的决策函数，
//! 供 `src-tauri` 的轮询线程与单一写点消费；放根 crate 是因为 CI 的
//! `cargo test` 只编译 workspace default-members（根 crate），
//! src-tauri 内嵌测试不进 CI。
//!
//! 决策优先级（[`desired_ignore_cursor_events`]，true = 穿透）：
//! 不可见 > 置底（Back）> 强制穿透（`click_through`）> 智能穿透（holding /
//! cursor_hit）> 智能关闭（整窗可交互，即历史行为）。

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

use crate::config::settings::{CompanionWindowLayer, Live2dSettings};

/// 进入命中区的判定外扩（逻辑像素）：比离开阈值小，形成迟滞带防边缘抖动；
/// 同时覆盖 Live2D 呼吸等小幅姿态造成的包围盒外溢。
pub const ENTER_MARGIN_PX: f64 = 10.0;

/// 离开命中区的判定外扩（逻辑像素）：大于进入阈值，光标须明显离开才切穿透。
pub const EXIT_MARGIN_PX: f64 = 24.0;

/// 角色窗口可命中矩形（窗口内逻辑像素，原点 = 窗口左上角）。
///
/// 语义是窗口内逻辑像素，与全屏物理像素活动区域（原表演模块的 `Rect`）不同。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HitRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl HitRect {
    /// 点是否落在矩形内（含边界）。
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    /// 四周外扩 `margin`（负值收缩）。width/height 保证 > 0（收缩最多到 0 由调用方保证）。
    pub fn expand(&self, margin: f64) -> Self {
        Self {
            x: self.x - margin,
            y: self.y - margin,
            width: self.width + margin * 2.0,
            height: self.height + margin * 2.0,
        }
    }
}

/// 穿透决策的全部策略输入快照（`visible` 由写点处现查覆盖，此处携带仅为
/// 让决策函数自完备、可纯测）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompanionPointerPolicy {
    pub visible: bool,
    pub layer: CompanionWindowLayer,
    /// 强制穿透（原「点击穿透」，语义升级）：用户最高优先级。
    pub force_click_through: bool,
    /// 智能穿透开关（缺省 true）。
    pub smart_enabled: bool,
}

/// 单一权威决策：true = 应忽略鼠标事件（穿透）。
pub fn desired_ignore_cursor_events(
    policy: CompanionPointerPolicy,
    cursor_hit: bool,
    holding: bool,
) -> bool {
    if !policy.visible {
        return true;
    }
    if policy.layer == CompanionWindowLayer::Back {
        return true;
    }
    if policy.force_click_through {
        return true;
    }
    if policy.smart_enabled {
        if holding {
            return false;
        }
        return !cursor_hit;
    }
    false
}

/// 光标是否命中角色区域（含迟滞与三态区域语义）。
///
/// - `None`（前端未就绪：启动 / 模型加载中 / 加载失败）→ fail-open 判命中，
///   保证角色永远可点（不会因上报缺失而「丢桌宠」）；
/// - `Some([])`（模型卸载清屏）→ 判未命中（穿透）;
/// - `Some(rects)` → 按 `current_ignore` 选迟滞侧：已穿透用进入阈值，
///   未穿透用离开阈值（更大，光标须明显离开才切穿透）。
pub fn cursor_hit(region: Option<&[HitRect]>, x: f64, y: f64, current_ignore: bool) -> bool {
    let Some(rects) = region else {
        return true;
    };
    if rects.is_empty() {
        return false;
    }
    let margin = if current_ignore {
        ENTER_MARGIN_PX
    } else {
        EXIT_MARGIN_PX
    };
    rects.iter().any(|r| r.expand(margin).contains(x, y))
}

/// hold（保护期）推进：`moved`（拖动中，窗口刚移动过）顺延 `hold`；
/// 未过期保持原值；过期或无 hold 清空。
pub fn next_hold(
    current: Option<Instant>,
    now: Instant,
    moved: bool,
    hold: Duration,
) -> Option<Instant> {
    if moved {
        return Some(now + hold);
    }
    match current {
        Some(until) if now < until => Some(until),
        _ => None,
    }
}

/// 解析智能穿透开关：缺省（未配置 / 旧版配置）视为**开启**（新默认行为）。
pub fn resolve_smart_click_through(live2d: Option<&Live2dSettings>) -> bool {
    live2d.and_then(|l| l.smart_click_through).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(
        visible: bool,
        layer: CompanionWindowLayer,
        force: bool,
        smart: bool,
    ) -> CompanionPointerPolicy {
        CompanionPointerPolicy {
            visible,
            layer,
            force_click_through: force,
            smart_enabled: smart,
        }
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> HitRect {
        HitRect {
            x,
            y,
            width,
            height,
        }
    }

    // ---- desired_ignore_cursor_events：优先级矩阵 ----

    #[test]
    fn test_desired_invisible_passthrough() {
        // 不可见优先于一切（含强制穿透关、置顶、命中）。
        let p = policy(false, CompanionWindowLayer::Front, false, true);
        assert!(desired_ignore_cursor_events(p, true, true));
    }

    #[test]
    fn test_desired_layer_back_passthrough() {
        let p = policy(true, CompanionWindowLayer::Back, false, true);
        assert!(desired_ignore_cursor_events(p, true, false));
    }

    #[test]
    fn test_desired_force_click_through_wins() {
        // 强制穿透压过智能命中 / holding。
        let p = policy(true, CompanionWindowLayer::Front, true, true);
        assert!(desired_ignore_cursor_events(p, true, true));
        assert!(desired_ignore_cursor_events(p, false, false));
    }

    #[test]
    fn test_desired_smart_hit_interactive() {
        let p = policy(true, CompanionWindowLayer::Front, false, true);
        assert!(!desired_ignore_cursor_events(p, true, false));
    }

    #[test]
    fn test_desired_smart_miss_passthrough() {
        let p = policy(true, CompanionWindowLayer::Front, false, true);
        assert!(desired_ignore_cursor_events(p, false, false));
    }

    #[test]
    fn test_desired_holding_overrides_miss() {
        // 保护期（拖动/菜单）强制可交互，即使光标已离开区域。
        let p = policy(true, CompanionWindowLayer::Front, false, true);
        assert!(!desired_ignore_cursor_events(p, false, true));
    }

    #[test]
    fn test_desired_smart_disabled_interactive() {
        // 智能关闭 = 历史行为：整窗可交互（与 hit / holding 无关）。
        let p = policy(true, CompanionWindowLayer::Front, false, false);
        assert!(!desired_ignore_cursor_events(p, false, false));
        assert!(!desired_ignore_cursor_events(p, false, true));
        assert!(!desired_ignore_cursor_events(p, true, false));
    }

    // ---- cursor_hit：三态 + 迟滞 ----

    #[test]
    fn test_cursor_hit_none_region_fail_open() {
        assert!(cursor_hit(None, 1_000_000.0, 1_000_000.0, false));
        assert!(cursor_hit(None, -50.0, -50.0, true));
    }

    #[test]
    fn test_cursor_hit_empty_region_clear_screen() {
        assert!(!cursor_hit(Some(&[]), 0.0, 0.0, false));
    }

    #[test]
    fn test_cursor_hit_inside_rect() {
        let rects = [rect(100.0, 100.0, 200.0, 400.0)];
        assert!(cursor_hit(Some(&rects), 200.0, 300.0, false));
        assert!(cursor_hit(Some(&rects), 100.0, 100.0, true)); // 含边界
    }

    #[test]
    fn test_cursor_hit_outside_rect() {
        let rects = [rect(100.0, 100.0, 200.0, 400.0)];
        assert!(!cursor_hit(Some(&rects), 400.0, 300.0, false));
        assert!(!cursor_hit(Some(&rects), 50.0, 50.0, true));
    }

    #[test]
    fn test_cursor_hit_hysteresis_margins() {
        // rect x ∈ [100, 300]；点 y=150 在竖直范围内。
        // ENTER=10 → 命中区 x ∈ [90, 310]；EXIT=24 → x ∈ [76, 324]。
        let rects = [rect(100.0, 100.0, 200.0, 100.0)];
        // x=305：两侧都命中（紧贴边缘 → 保持命中，不抖动）。
        assert!(cursor_hit(Some(&rects), 305.0, 150.0, true));
        assert!(cursor_hit(Some(&rects), 305.0, 150.0, false));
        // x=320：迟滞带 —— 已穿透（ENTER 侧）不命中，未穿透（EXIT 侧）命中。
        assert!(!cursor_hit(Some(&rects), 320.0, 150.0, true));
        assert!(cursor_hit(Some(&rects), 320.0, 150.0, false));
        // x=330：两侧都不命中（明确离开 → 穿透）。
        assert!(!cursor_hit(Some(&rects), 330.0, 150.0, true));
        assert!(!cursor_hit(Some(&rects), 330.0, 150.0, false));
    }

    #[test]
    fn test_cursor_hit_any_rect() {
        let rects = [rect(0.0, 0.0, 10.0, 10.0), rect(500.0, 500.0, 20.0, 20.0)];
        assert!(cursor_hit(Some(&rects), 510.0, 510.0, false));
    }

    // ---- HitRect：contains / expand ----

    #[test]
    fn test_hit_rect_contains_boundaries() {
        let r = rect(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(10.0, 20.0));
        assert!(r.contains(110.0, 70.0));
        assert!(!r.contains(110.1, 20.0));
        assert!(!r.contains(9.9, 20.0));
        assert!(!r.contains(50.0, 70.1));
    }

    #[test]
    fn test_hit_rect_expand_zero_and_positive() {
        let r = rect(10.0, 10.0, 100.0, 100.0);
        let zero = r.expand(0.0);
        assert_eq!(zero, r);
        let grown = r.expand(5.0);
        assert_eq!(grown, rect(5.0, 5.0, 110.0, 110.0));
        assert!(grown.contains(5.0, 5.0));
    }

    // ---- next_hold ----

    #[test]
    fn test_next_hold_moved_extends() {
        let now = Instant::now();
        // 拖动中：无论旧值如何都顺延。
        assert_eq!(
            next_hold(None, now, true, Duration::from_millis(600)),
            Some(now + Duration::from_millis(600))
        );
    }

    #[test]
    fn test_next_hold_unexpired_kept() {
        let now = Instant::now();
        let until = now + Duration::from_millis(300);
        assert_eq!(
            next_hold(Some(until), now, false, Duration::from_millis(600)),
            Some(until)
        );
    }

    #[test]
    fn test_next_hold_expired_cleared() {
        let now = Instant::now();
        let expired = now - Duration::from_millis(1);
        assert_eq!(
            next_hold(Some(expired), now, false, Duration::from_millis(600)),
            None
        );
        assert_eq!(
            next_hold(None, now, false, Duration::from_millis(600)),
            None
        );
    }

    // ---- resolve_smart_click_through ----

    #[test]
    fn test_resolve_smart_click_through_defaults_true() {
        assert!(resolve_smart_click_through(None));
        assert!(resolve_smart_click_through(
            Some(&Live2dSettings::default())
        ));
    }

    #[test]
    fn test_resolve_smart_click_through_reads_flag() {
        let on = Live2dSettings {
            smart_click_through: Some(true),
            ..Default::default()
        };
        let off = Live2dSettings {
            smart_click_through: Some(false),
            ..Default::default()
        };
        assert!(resolve_smart_click_through(Some(&on)));
        assert!(!resolve_smart_click_through(Some(&off)));
    }
}
