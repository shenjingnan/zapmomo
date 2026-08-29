//! 角色窗口拖动时聊天气泡联动的纯逻辑层。
//!
//! 联动语义（单向）：拖动角色窗口时，气泡窗口按相同位移平移，保持两者
//! 相对距离不变（像一组窗口）；反向不联动——单独拖动气泡只影响气泡。
//! 气泡被联动移动后由其自身 `onMoved` → debounce 写回 `[bubble.window_position]`，
//! 相对关系随启动恢复自然还原，无需新增持久化字段。
//!
//! 与 [`crate::companion_click_through`] 同理：只放**可纯计算**的函数，
//! 放根 crate 是因为 CI 的 `cargo test` 只编译 workspace default-members，
//! src-tauri 内嵌测试不进 CI。

/// 计算气泡跟随角色移动后的新位置（物理像素）。
///
/// - `origin`：角色窗口上一次位置缓存；`None`（尚无基准，理论不可达，
///   防御保留）→ `None`，不动气泡；
/// - `companion`：角色窗口本次 `Moved` 位置；
/// - `bubble`：气泡窗口当前位置；
/// - 位移为零（角色实际未动）→ `None`，跳过对气泡的冗余移动调用；
/// - 其余 → `Some(气泡新位置)` = 气泡当前位置 + 角色位移。
///
/// 全程物理像素直接平移，不做 DPI 换算：同屏/同倍率下精确守恒；跨倍率
/// 显示器间拖动时物理位移守恒是最佳近似。
pub fn bubble_follow_position(
    origin: Option<(f64, f64)>,
    companion: (f64, f64),
    bubble: (f64, f64),
) -> Option<(f64, f64)> {
    let (ox, oy) = origin?;
    let (dx, dy) = (companion.0 - ox, companion.1 - oy);
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    Some((bubble.0 + dx, bubble.1 + dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_follow_moves_bubble_by_same_delta() {
        // 角色右移 50、下移 60 → 气泡平移相同位移。
        let next = bubble_follow_position(Some((100.0, 200.0)), (150.0, 260.0), (500.0, 400.0));
        assert_eq!(next, Some((550.0, 460.0)));
    }

    #[test]
    fn test_follow_negative_delta() {
        // 向左上拖动：位移为负同样守恒。
        let next = bubble_follow_position(Some((100.0, 200.0)), (40.0, 120.0), (300.0, 500.0));
        assert_eq!(next, Some((240.0, 420.0)));
    }

    #[test]
    fn test_follow_without_origin_is_none() {
        // 无基准（首次 Moved 前的防御路径）：不动气泡。
        assert_eq!(
            bubble_follow_position(None, (150.0, 260.0), (500.0, 400.0)),
            None
        );
    }

    #[test]
    fn test_follow_zero_delta_is_none() {
        // 角色未动：跳过对气泡的冗余移动。
        assert_eq!(
            bubble_follow_position(Some((100.0, 200.0)), (100.0, 200.0), (500.0, 400.0)),
            None
        );
    }

    #[test]
    fn test_follow_independent_of_bubble_distance() {
        // 始终联动语义：气泡被单独挪到再远，拖角色仍按相同位移跟随。
        let next = bubble_follow_position(Some((1000.0, 800.0)), (1050.0, 860.0), (2400.0, -300.0));
        assert_eq!(next, Some((2450.0, -240.0)));
    }
}
