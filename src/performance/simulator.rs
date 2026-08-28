//! 表演模拟器：打字与鼠标。
//!
//! 两个 [`PerformanceSource`] 实现，全部事件由内部状态机 + [`Rng`] 生成，
//! 不依赖任何真实输入。键名与 BongoCat `device.rs` 一致（rdev Debug 串），
//! 消费端无需感知来源。

use crate::performance::rng::Rng;
use crate::performance::source::PerformanceSource;
use crate::performance::{DeviceEvent, DeviceEventKind, PerformanceScene, Rect};
use std::collections::VecDeque;
use std::time::Duration;

/// 打字用的常用英文词表（退路：词内缺失键跳过、全缺则随机敲池内键）。
const WORD_LIST: &[&str] = &[
    "the", "be", "to", "of", "and", "a", "in", "that", "have", "i", "it", "for", "not", "on",
    "with", "he", "as", "you", "do", "at", "this", "but", "his", "by", "from", "they", "we", "say",
    "her", "she", "or", "an", "will", "my", "one", "all", "would", "there", "their", "what", "so",
    "up", "out", "if", "about", "who", "get", "which", "go", "me", "when", "make", "can", "like",
    "time", "no", "just", "him", "know", "take", "people", "into", "year", "your", "good", "some",
    "could", "them", "see", "other", "than", "then", "now", "look", "only", "come", "its", "over",
    "think", "also", "back", "after", "use", "two", "how", "our", "work", "first", "well", "way",
    "even", "new", "want", "because", "any", "these", "give", "day", "most", "us", "open", "need",
    "here", "between", "always", "call", "ask", "type", "chat", "code", "file", "main", "list",
    "data", "test", "run", "save", "click", "hello", "world",
];

/// 键池：由模型实际拥有贴图的键名构建（无贴图的键不会出现在事件流中）。
#[derive(Debug, Clone, Default)]
pub struct KeyPool {
    /// 字母键（"KeyA".."KeyZ" 形式）。
    letters: Vec<String>,
    /// 修饰键（Shift 家族，优先用左 Shift）。
    shift: Option<String>,
    /// 其余键（Space/Backspace/Enter/Fn/标点等）。
    misc: Vec<String>,
    /// 全量键（退路）。
    all: Vec<String>,
}

impl KeyPool {
    /// 从键名列表构建键池（键名形如 `KeyA`、`ShiftLeft`、`Space`）。
    pub fn new(keys: Vec<String>) -> Self {
        let mut pool = KeyPool::default();
        for key in keys {
            pool.all.push(key.clone());
            if key.starts_with("Key") && key.len() == 4 {
                pool.letters.push(key);
            } else if matches!(key.as_str(), "Shift" | "ShiftLeft" | "ShiftRight") {
                // 优先保留左 Shift
                if pool.shift.as_deref() != Some("ShiftLeft") {
                    pool.shift = Some(key);
                }
            } else {
                pool.misc.push(key);
            }
        }
        pool
    }

    fn has(&self, key: &str) -> bool {
        self.all.iter().any(|k| k == key)
    }
}

/// 打字模拟器：按词表 + 人类节奏生成键盘事件流。
///
/// 节奏结构：词内按键间隔指数分布（均值 ~110ms），词间 60-250ms，
/// 每 3-10 词一个 0.5-3s 停顿（偶发 5-10s 长歇）。10-20% 大写词产生真实
/// Shift 序列；小概率打错字走「错字母 → Backspace → 正确字母」修正。
pub struct TypingSimulator {
    pool: KeyPool,
    pending: VecDeque<DeviceEvent>,
    next_delay: Duration,
    since_break: u64,
}

impl TypingSimulator {
    /// 以模型实际拥有的键名列表构造（见 [`KeyPool::new`]）。
    pub fn new(keys: Vec<String>) -> Self {
        Self {
            pool: KeyPool::new(keys),
            pending: VecDeque::new(),
            next_delay: Duration::from_millis(300),
            since_break: 0,
        }
    }

    fn key_event(&self, kind: DeviceEventKind, key: &str) -> DeviceEvent {
        DeviceEvent::key(kind, key.to_string())
    }

    fn key_press(&self, key: &str) -> DeviceEvent {
        self.key_event(DeviceEventKind::KeyboardPress, key)
    }

    fn key_release(&self, key: &str) -> DeviceEvent {
        self.key_event(DeviceEventKind::KeyboardRelease, key)
    }

    /// 填装下一个词（或停顿）的按键序列，并设定该词第一个键前的等待时长。
    fn fill_pending(&mut self, rng: &mut Rng) {
        self.since_break += 1;
        self.next_delay = if self.since_break >= 3 + rng.int_range(0, 7) {
            // 每 3-10 词一次停顿
            self.since_break = 0;
            if rng.chance(0.15) {
                Duration::from_secs_f64(rng.range(5.0, 10.0)) // 偶发长歇
            } else {
                Duration::from_secs_f64(rng.range(0.5, 3.0))
            }
        } else {
            Duration::from_millis(rng.int_range(60, 250)) // 词间
        };

        let mut seq = self.generate_word(rng);
        if seq.is_empty() {
            // 键池没有可用字母：退路，随机敲一个池内键（press + release）
            let key = rng.pick(&self.pool.all).clone();
            seq.push_back(self.key_press(&key));
            seq.push_back(self.key_release(&key));
        }
        self.pending.extend(seq);
    }

    /// 生成一个词（含大写包裹、打错修正、词尾标点）。
    fn generate_word(&mut self, rng: &mut Rng) -> VecDeque<DeviceEvent> {
        let mut out = VecDeque::new();
        let word = *rng.pick(WORD_LIST);
        let uppercase = self.pool.shift.is_some() && rng.chance(0.15);
        let shift = self.pool.shift.as_deref().unwrap_or_default();
        if uppercase {
            out.push_back(self.key_press(shift));
        }
        for (i, ch) in word.chars().enumerate() {
            let key = format!("Key{}", ch.to_ascii_uppercase());
            if !self.pool.letters.contains(&key) {
                continue; // 缺失键跳过（视觉上该字母不按）
            }
            // 打错修正（不进词首）
            if i > 0 && self.pool.has("Backspace") && rng.chance(0.06) {
                let wrong = rng.pick(&self.pool.letters).clone();
                out.push_back(self.key_press(&wrong));
                out.push_back(self.key_release(&wrong));
                out.push_back(self.key_press("Backspace"));
                out.push_back(self.key_release("Backspace"));
            }
            out.push_back(self.key_press(&key));
            out.push_back(self.key_release(&key));
        }
        if uppercase {
            out.push_back(self.key_release(shift));
        }
        // 词尾标点
        if rng.chance(0.12) {
            let punct = if self.pool.has("Space") {
                "Space"
            } else if self.pool.has("Period") {
                "Period"
            } else {
                return out;
            };
            out.push_back(self.key_press(punct));
            out.push_back(self.key_release(punct));
        }
        out
    }
}

impl PerformanceSource for TypingSimulator {
    fn scene(&self) -> PerformanceScene {
        PerformanceScene::Typing
    }

    fn next_event(&mut self, rng: &mut Rng) -> Option<(Duration, DeviceEvent)> {
        if self.pending.is_empty() {
            self.fill_pending(rng);
        }
        let event = self.pending.pop_front()?;
        let delay = self.next_delay;
        // 词内按键间隔：指数分布（均值 ~110ms），截断 ≤500ms
        let sample = -0.110_f64 * (1.0 - rng.next_f64()).ln();
        self.next_delay = Duration::from_secs_f64(sample.min(0.5));
        Some((delay, event))
    }
}

/// 鼠标模拟器：随机目标点 + 缓动轨迹 + 概率点击。
///
/// 运动节奏：休息 0.4-2s → 挑目标点（偏向屏幕中部）→ easeOutCubic 缓动 +
/// 微噪声（8-16ms/帧）→ 75% 左键单点 / 8% 双击 / 5% 右键 / 12% 不点 → 休息。
pub struct MouseSimulator {
    area: Rect,
    pos: (f64, f64),
    pending: VecDeque<(Duration, DeviceEvent)>,
}

impl MouseSimulator {
    /// 以物理像素活动区域构造（初始光标在区域中心）。
    pub fn new(area: Rect) -> Self {
        Self {
            area,
            pos: (area.x + area.width / 2.0, area.y + area.height / 2.0),
            pending: VecDeque::new(),
        }
    }

    fn press_event(button: &str) -> DeviceEvent {
        DeviceEvent::key(DeviceEventKind::MousePress, button)
    }

    fn release_event(button: &str) -> DeviceEvent {
        DeviceEvent::key(DeviceEventKind::MouseRelease, button)
    }

    /// 挑一个目标点：偏向屏幕中部（不贴边，视觉自然）。
    fn pick_target(&self, rng: &mut Rng) -> (f64, f64) {
        let x = self.area.x + self.area.width * rng.range(0.15, 0.85);
        let y = self.area.y + self.area.height * rng.range(0.2, 0.8);
        (x, y)
    }

    /// 生成一段运动帧（easeOutCubic + 微噪声），坐标 clamp 到活动区域。
    ///
    /// 独立成函数以便单测缓动单调性。
    fn move_frames(
        &self,
        from: (f64, f64),
        to: (f64, f64),
        rng: &mut Rng,
    ) -> Vec<(Duration, (f64, f64))> {
        let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (dist / 3.0).clamp(4.0, 40.0) as u64;
        let max_x = self.area.x + self.area.width;
        let max_y = self.area.y + self.area.height;
        let mut frames = Vec::with_capacity(steps as usize);
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let eased = 1.0 - (1.0 - t).powi(3);
            let x =
                (from.0 + (to.0 - from.0) * eased + rng.range(-1.5, 1.5)).clamp(self.area.x, max_x);
            let y =
                (from.1 + (to.1 - from.1) * eased + rng.range(-1.5, 1.5)).clamp(self.area.y, max_y);
            frames.push((Duration::from_millis(rng.int_range(8, 16)), (x, y)));
        }
        frames
    }

    /// 填装一段完整运动：休息 → 运动 → 可能点击。
    fn fill_pending(&mut self, rng: &mut Rng) {
        self.pending.push_back((
            Duration::from_secs_f64(rng.range(0.4, 2.0)),
            DeviceEvent::point(self.pos.0, self.pos.1),
        ));

        let to = self.pick_target(rng);
        let from = self.pos;
        for (delay, (x, y)) in self.move_frames(from, to, rng) {
            self.pending.push_back((delay, DeviceEvent::point(x, y)));
        }
        self.pos = to;

        let roll = rng.next_f64();
        if roll < 0.75 {
            // 左键单点
            self.pending
                .push_back((Duration::from_millis(20), Self::press_event("Left")));
            self.pending.push_back((
                Duration::from_millis(rng.int_range(60, 120)),
                Self::release_event("Left"),
            ));
        } else if roll < 0.83 {
            // 双击
            self.pending
                .push_back((Duration::from_millis(20), Self::press_event("Left")));
            self.pending
                .push_back((Duration::from_millis(80), Self::release_event("Left")));
            self.pending
                .push_back((Duration::from_millis(40), Self::press_event("Left")));
            self.pending
                .push_back((Duration::from_millis(80), Self::release_event("Left")));
        } else if roll < 0.88 {
            // 右键
            self.pending
                .push_back((Duration::from_millis(20), Self::press_event("Right")));
            self.pending.push_back((
                Duration::from_millis(rng.int_range(60, 120)),
                Self::release_event("Right"),
            ));
        }
        // 其余 12% 不点击
    }
}

impl PerformanceSource for MouseSimulator {
    fn scene(&self) -> PerformanceScene {
        PerformanceScene::Mouse
    }

    fn next_event(&mut self, rng: &mut Rng) -> Option<(Duration, DeviceEvent)> {
        if self.pending.is_empty() {
            self.fill_pending(rng);
        }
        self.pending.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::performance::DeviceValue;

    /// 收集模拟器前 `n` 个事件的工具。
    fn collect_typing(keys: &[&str], seed: u64, n: usize) -> Vec<(Duration, DeviceEvent)> {
        let keys = keys.iter().map(|s| s.to_string()).collect();
        let mut src = TypingSimulator::new(keys);
        let mut rng = Rng::new(seed);
        (0..n).filter_map(|_| src.next_event(&mut rng)).collect()
    }

    fn collect_mouse(area: Rect, seed: u64, n: usize) -> Vec<(Duration, DeviceEvent)> {
        let mut src = MouseSimulator::new(area);
        let mut rng = Rng::new(seed);
        (0..n).filter_map(|_| src.next_event(&mut rng)).collect()
    }

    /// 标准字母键池（模拟键盘模式预设的键名集合）。
    const LETTER_KEYS: &[&str] = &[
        "KeyA",
        "KeyB",
        "KeyC",
        "KeyD",
        "KeyE",
        "KeyF",
        "KeyG",
        "KeyH",
        "KeyI",
        "KeyJ",
        "KeyK",
        "KeyL",
        "KeyM",
        "KeyN",
        "KeyO",
        "KeyP",
        "KeyQ",
        "KeyR",
        "KeyS",
        "KeyT",
        "KeyU",
        "KeyV",
        "KeyW",
        "KeyX",
        "KeyY",
        "KeyZ",
        "ShiftLeft",
        "ShiftRight",
        "Space",
        "Backspace",
        "Enter",
        "Period",
    ];

    #[test]
    fn typing_events_use_only_pool_keys() {
        let keys = LETTER_KEYS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let events = collect_typing(LETTER_KEYS, 42, 2000);
        let pool: std::collections::HashSet<&str> = keys.iter().map(String::as_str).collect();
        for (_, ev) in &events {
            if let DeviceValue::Key(k) = &ev.value {
                assert!(pool.contains(k.as_str()), "事件键 {k} 不在键池内");
            } else {
                panic!("打字模拟器不应产生坐标事件");
            }
        }
    }

    #[test]
    fn typing_press_release_strictly_paired() {
        let events = collect_typing(LETTER_KEYS, 42, 5000);
        let mut held: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for (_, ev) in &events {
            let DeviceValue::Key(k) = &ev.value else {
                continue;
            };
            match ev.kind {
                DeviceEventKind::KeyboardPress => {
                    let entry = held.entry(k).or_insert(0);
                    assert_eq!(*entry, 0, "键 {k} 重复按下（未释放）");
                    *entry += 1;
                }
                DeviceEventKind::KeyboardRelease => {
                    let entry = held
                        .get_mut(k.as_str())
                        .unwrap_or_else(|| panic!("键 {k} 释放前未按下"));
                    assert!(*entry > 0, "键 {k} 释放计数异常");
                    *entry -= 1;
                }
                _ => {}
            }
        }
        assert!(
            held.values().all(|&c| c == 0),
            "结束时应无悬挂按住的键: {held:?}"
        );
    }

    #[test]
    fn typing_has_human_rhythm() {
        let events = collect_typing(LETTER_KEYS, 99, 8000);
        // 所有间隔严格为正
        for (delay, _) in &events {
            assert!(!delay.is_zero(), "间隔必须严格为正");
        }
        // 存在 >500ms 的停顿（休息结构，词首间隔上限）
        assert!(
            events.iter().any(|(d, _)| *d > Duration::from_millis(500)),
            "应存在停顿间隔"
        );
        // 存在快连击（<200ms，词内节奏）
        assert!(
            events.iter().any(|(d, _)| *d < Duration::from_millis(200)),
            "应存在快连击"
        );
    }

    #[test]
    fn typing_uppercase_uses_shift_sequence() {
        let events = collect_typing(LETTER_KEYS, 1234, 4000);
        let mut found_shift_capital = false;
        let mut i = 0;
        while i + 1 < events.len() {
            let DeviceValue::Key(a) = &events[i].1.value else {
                i += 1;
                continue;
            };
            let DeviceValue::Key(b) = &events[i + 1].1.value else {
                i += 1;
                continue;
            };
            // 模式：Shift 按下 后紧跟 字母按下
            if events[i].1.kind == DeviceEventKind::KeyboardPress
                && a.starts_with("Shift")
                && events[i + 1].1.kind == DeviceEventKind::KeyboardPress
                && b.starts_with("Key")
            {
                found_shift_capital = true;
                break;
            }
            i += 1;
        }
        assert!(found_shift_capital, "应出现大写词（Shift 包裹字母）");
    }

    #[test]
    fn typing_has_backspace_correction() {
        let events = collect_typing(LETTER_KEYS, 7, 8000);
        assert!(
            events
                .iter()
                .any(|(_, ev)| matches!(&ev.value, DeviceValue::Key(k) if k == "Backspace")),
            "应出现 Backspace 打错修正"
        );
    }

    #[test]
    fn typing_without_letters_falls_back_to_pool_keys() {
        // 键池无字母键（如仅 Fn/Enter）时，退路随机敲池内键，仍产出事件且键在池内
        let keys = ["Fn", "Enter", "Space"];
        let mut src = TypingSimulator::new(keys.iter().map(|s| s.to_string()).collect());
        let mut rng = Rng::new(5);
        let mut count = 0;
        for _ in 0..50 {
            if let Some((_, ev)) = src.next_event(&mut rng) {
                let DeviceValue::Key(k) = &ev.value else {
                    panic!()
                };
                assert!(keys.contains(&k.as_str()), "退路键 {k} 不在池内");
                count += 1;
            }
        }
        assert!(count > 0, "退路应产出事件");
    }

    #[test]
    fn mouse_points_stay_in_area() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let events = collect_mouse(area, 42, 3000);
        for (_, ev) in &events {
            if let DeviceValue::Point { x, y } = ev.value {
                assert!(area.contains(x, y), "坐标 ({x},{y}) 越界");
            }
        }
    }

    #[test]
    fn mouse_press_release_paired_and_buttons_valid() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let events = collect_mouse(area, 42, 3000);
        let mut left_held = false;
        let mut right_held = false;
        let mut left_presses = 0;
        for (_, ev) in &events {
            let DeviceValue::Key(button) = &ev.value else {
                continue;
            };
            assert!(
                button == "Left" || button == "Right",
                "鼠标键应为 Left/Right，实际 {button}"
            );
            match ev.kind {
                DeviceEventKind::MousePress => {
                    assert!(
                        (button == "Left" && !left_held) || (button == "Right" && !right_held),
                        "重复按下 {button}"
                    );
                    if button == "Left" {
                        left_held = true;
                        left_presses += 1;
                    } else {
                        right_held = true;
                    }
                }
                DeviceEventKind::MouseRelease => {
                    assert!(
                        (button == "Left" && left_held) || (button == "Right" && right_held),
                        "{button} 释放前未按下"
                    );
                    if button == "Left" {
                        left_held = false;
                    } else {
                        right_held = false;
                    }
                }
                _ => {}
            }
        }
        assert!(left_presses > 0, "应出现左键点击");
        assert!(!left_held && !right_held, "结束时不应有悬挂按住的鼠标键");
    }

    #[test]
    fn mouse_move_frames_approach_target_monotonically() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let sim = MouseSimulator::new(area);
        let from = (100.0, 100.0);
        let to = (1500.0, 800.0);
        let frames = sim.move_frames(from, to, &mut Rng::new(3));
        assert!(frames.len() >= 4, "至少 4 帧");

        let mut prev_dist = f64::INFINITY;
        for (delay, (x, y)) in &frames {
            assert!(*delay >= Duration::from_millis(8) && *delay <= Duration::from_millis(16));
            let dist = ((to.0 - x).powi(2) + (to.1 - y).powi(2)).sqrt();
            assert!(
                dist <= prev_dist + 5.0,
                "距离应总体单调下降，{dist} > {prev_dist}"
            );
            prev_dist = dist;
        }
        // 末帧应贴近目标（噪声 ≤1.5px）
        let last = frames.last().unwrap().1;
        let final_dist = ((to.0 - last.0).powi(2) + (to.1 - last.1).powi(2)).sqrt();
        assert!(final_dist < 5.0, "末帧应贴近目标，实际距离 {final_dist}");
    }

    #[test]
    fn mouse_rest_delay_within_bounds() {
        let area = Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let events = collect_mouse(area, 42, 3000);
        // 每段运动的第一帧 = 休息后的首事件，delay 应 ∈ [0.4s, 2s]
        // 简化断言：所有 delay 中确实存在 ≥0.4s 的休息间隔
        assert!(
            events.iter().any(|(d, _)| *d >= Duration::from_millis(400)),
            "应存在休息间隔"
        );
    }
}
