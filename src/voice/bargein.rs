//! 语音打断（ASR barge-in）纯逻辑：回声比对 + 触发判定。
//!
//! Speaking/Thinking 期间 ASR 持续收音，partial 既可能是用户语音、也可能是 TTS 外放
//! 被麦克风拾回的「回声」。本模块提供两层判定：
//!
//! - [`bigram_dice`] / [`is_echo_leak`]：识别文本与回声参考（最近播出的句子）的字符
//!   bigram 相似度，高相似判回声；
//! - [`VoiceBargeInDetector`]：连续 [`DEFAULT_CONSECUTIVE_HITS`] 个 chunk 同时满足
//!   「RMS 超阈值 + partial 有效（≥[`MIN_PARTIAL_HANZI`] 汉字）+ 与回声参考不相似」
//!   才触发打断（防瞬时噪音）。
//!
//! 另附 [`EchoTracker`]：维护「当前播报句 + 最近播完 1 句」的回声参考窗口与已播句子
//! 累积（语音打断时作为 assistant 消息入历史）。
//!
//! 全部纯逻辑、零依赖，可独立单测（编排接线见 `session.rs`）。

use std::collections::VecDeque;

/// 触发打断所需的连续命中 chunk 数（模式同 WaitingSpeech 的 `speech_hits`）。
pub const DEFAULT_CONSECUTIVE_HITS: u32 = 2;

/// partial 判定「有意义内容」的最小汉字数（去标点/ASCII 后）。
pub const MIN_PARTIAL_HANZI: usize = 2;

/// 回声参考窗口容量：当前播报句 + 最近播完 1 句（回声滞后，partial 常对应上一句）。
const ECHO_WINDOW: usize = 2;

/// 比对归一化：仅保留字母数字并小写化。
///
/// 汉字属 `is_alphanumeric`，故标点/空白/emoji 一次剥净且不受影响；小写化覆盖英文台词。
pub fn normalize_for_match(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// CJK 统一表意文字判定（含扩展 A，覆盖常用汉字全集）。
fn is_han(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// 有效汉字数：partial 的「有意义内容」长度门槛（标点/ASCII 不计入）。
pub fn effective_hanzi_count(s: &str) -> usize {
    s.chars().filter(|&c| is_han(c)).count()
}

/// 字符 bigram Dice 系数（0.0–1.0），入参各自先做 [`normalize_for_match`]。
///
/// 任一侧归一后不足 2 字符（无法构成 bigram）返回 0.0——太短视为「不相似」：
/// 单字 query（如「停」）放行、空参考不过滤，都是这个语义的自然推论。
pub fn bigram_dice(a: &str, b: &str) -> f32 {
    let a: Vec<char> = normalize_for_match(a).chars().collect();
    let b: Vec<char> = normalize_for_match(b).chars().collect();
    if a.len() < 2 || b.len() < 2 {
        return 0.0;
    }
    let mut ga: Vec<[char; 2]> = a.windows(2).map(|w| [w[0], w[1]]).collect();
    let mut gb: Vec<[char; 2]> = b.windows(2).map(|w| [w[0], w[1]]).collect();
    ga.sort_unstable();
    gb.sort_unstable();
    // 双指针求多重集交集大小（重复 bigram 各自计一次）
    let (mut i, mut j, mut inter) = (0usize, 0usize, 0usize);
    while i < ga.len() && j < gb.len() {
        match ga[i].cmp(&gb[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    2.0 * inter as f32 / (ga.len() + gb.len()) as f32
}

/// 后置兜底判定：识别文本与回声参考高相似（Dice ≥ `threshold`）→ 回声漏网。
///
/// 参考归一后不足 2 字符时不过滤（返回 false）——刚开播/纯标点句不构成有效参考。
pub fn is_echo_leak(text: &str, echo_ref: &str, threshold: f32) -> bool {
    if normalize_for_match(echo_ref).chars().count() < 2 {
        return false;
    }
    bigram_dice(text, echo_ref) >= threshold
}

/// 语音打断触发判定器（编排循环每 chunk 调一次 [`VoiceBargeInDetector::observe`]）。
///
/// 三条件同时满足记一次命中：RMS 超阈值、partial 有效（≥[`MIN_PARTIAL_HANZI`] 汉字）、
/// 与回声参考不相似（Dice < 阈值）；任一不满足清零（要求**连续**命中）；连续达
/// [`DEFAULT_CONSECUTIVE_HITS`] 次返回 true 并自行清零（防连续重复触发）。
///
/// 回声参考为空时相似度条件恒过（`bigram_dice` 对空串返 0）——Thinking 阶段还没播过
/// 句、无参考即不过滤。
#[derive(Debug)]
pub struct VoiceBargeInDetector {
    threshold: f32,
    required_hits: u32,
    hits: u32,
}

impl VoiceBargeInDetector {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            required_hits: DEFAULT_CONSECUTIVE_HITS,
            hits: 0,
        }
    }

    /// 吸收一个 chunk 的判定输入：返回 true = 触发语音打断。
    pub fn observe(&mut self, partial: &str, rms: f32, rms_threshold: f32, echo_ref: &str) -> bool {
        let ok = rms > rms_threshold
            && effective_hanzi_count(partial) >= MIN_PARTIAL_HANZI
            && bigram_dice(partial, echo_ref) < self.threshold;
        if !ok {
            self.hits = 0;
            return false;
        }
        self.hits += 1;
        if self.hits >= self.required_hits {
            self.hits = 0;
            true
        } else {
            false
        }
    }

    /// 复位命中计数（打断执行后/新一轮开始时调用）。
    pub fn reset(&mut self) {
        self.hits = 0;
    }
}

/// 回声参考窗口 + 已播句子累积（纯逻辑，可单测）。
///
/// `window` 保存「当前播报句 + 最近播完 1 句」（[`ECHO_WINDOW`]）；`spoken` 累积每次
/// 弹句播放的原句（含标点），语音打断时作为 assistant 消息入历史。
#[derive(Debug, Default)]
pub struct EchoTracker {
    window: VecDeque<String>,
    spoken: String,
}

impl EchoTracker {
    /// 记录一句已开始播放的句子：入窗口（超容挤掉最旧）+ 追加进已播累积。
    pub fn record_played(&mut self, sentence: &str) {
        self.window.push_back(sentence.to_string());
        while self.window.len() > ECHO_WINDOW {
            self.window.pop_front();
        }
        self.spoken.push_str(sentence);
    }

    /// 回声参考文本（窗口内句子按播出顺序拼接；空窗口 → 空串 = 判定不过滤）。
    pub fn reference(&self) -> String {
        self.window.iter().map(String::as_str).collect()
    }

    /// 取走已播句子累积（语音打断时入历史），并清空。
    pub fn take_spoken(&mut self) -> String {
        std::mem::take(&mut self.spoken)
    }

    /// 清空窗口与已播累积（新一轮/非语音打断路径调用）。
    pub fn clear(&mut self) {
        self.window.clear();
        self.spoken.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- bigram_dice ----------

    #[test]
    fn test_bigram_dice_identical_is_one() {
        assert!((bigram_dice("今天天气不错", "今天天气不错") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_bigram_dice_disjoint_is_zero() {
        assert_eq!(bigram_dice("今天天气", "abcdefgh"), 0.0);
    }

    #[test]
    fn test_bigram_dice_prefix_partial_similarity() {
        // 「今天天气不错」的 bigram 中 4/6 与「今天天气」重合 → Dice = 2*4/(4+6) = 0.8
        let d = bigram_dice("今天天气", "今天天气不错");
        assert!(d > 0.7 && d < 0.9, "prefix dice = {d}");
    }

    #[test]
    fn test_bigram_dice_too_short_is_zero() {
        // 任一侧归一后 < 2 字符（含空串、单字）→ 0.0
        assert_eq!(bigram_dice("停", "今天天气不错"), 0.0);
        assert_eq!(bigram_dice("", "今天天气不错"), 0.0);
        assert_eq!(bigram_dice("今天天气", "!!"), 0.0);
    }

    // ---------- normalize_for_match / effective_hanzi_count ----------

    #[test]
    fn test_normalize_strips_punct_and_case() {
        assert_eq!(normalize_for_match("你好，世界！"), "你好世界");
        assert_eq!(normalize_for_match("Hello, World."), "helloworld");
        assert_eq!(normalize_for_match("  。！ "), "");
    }

    #[test]
    fn test_effective_hanzi_count_ignores_punct_and_ascii() {
        assert_eq!(effective_hanzi_count("你好，世界！"), 4);
        assert_eq!(effective_hanzi_count("abc123"), 0);
        assert_eq!(effective_hanzi_count("。。。"), 0);
    }

    // ---------- is_echo_leak ----------

    #[test]
    fn test_echo_leak_identical_true() {
        assert!(is_echo_leak("今天天气不错。", "今天天气不错", 0.5));
    }

    #[test]
    fn test_echo_leak_punct_variant_true() {
        // 标点差异被归一化剥净 → 恒等相似
        assert!(is_echo_leak("今天，天气不错！", "今天天气不错", 0.5));
    }

    #[test]
    fn test_echo_leak_unrelated_false() {
        assert!(!is_echo_leak("帮我讲个故事", "今天天气不错", 0.5));
    }

    #[test]
    fn test_echo_leak_empty_reference_false() {
        // 参考为空（未播过句/纯标点）→ 不过滤
        assert!(!is_echo_leak("今天天气不错", "", 0.5));
        assert!(!is_echo_leak("今天天气不错", "。！？", 0.5));
    }

    #[test]
    fn test_echo_leak_single_char_text_false() {
        // 单字 query（「停」）bigram 为空 → Dice 0 → 放行（保话头语义）
        assert!(!is_echo_leak("停", "今天天气不错", 0.5));
    }

    // ---------- VoiceBargeInDetector ----------

    fn detector() -> VoiceBargeInDetector {
        VoiceBargeInDetector::new(0.5)
    }

    #[test]
    fn test_detector_requires_two_consecutive_valid_chunks() {
        let mut d = detector();
        // 首个有效 chunk：命中但未达连续数
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
        // 第二个连续有效 chunk → 触发
        assert!(d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
    }

    #[test]
    fn test_detector_resets_hits_on_invalid_chunk() {
        let mut d = detector();
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
        // 中间断一次（静音）→ 计数清零，不触发
        assert!(!d.observe("今天天气怎么样", 0.001, 0.02, "昨天很冷"));
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
        assert!(d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
    }

    #[test]
    fn test_detector_rms_below_threshold_no_trigger() {
        let mut d = detector();
        assert!(!d.observe("今天天气怎么样", 0.01, 0.02, "昨天很冷"));
        assert!(!d.observe("今天天气怎么样", 0.01, 0.02, "昨天很冷"));
    }

    #[test]
    fn test_detector_fewer_than_two_hanzi_no_trigger() {
        let mut d = detector();
        assert!(!d.observe("嗯", 0.5, 0.02, "昨天很冷"));
        assert!(!d.observe("嗯。", 0.5, 0.02, "昨天很冷"));
    }

    #[test]
    fn test_detector_echo_similar_no_trigger() {
        // partial 与回声参考高相似（跟读台词）→ 判回声，不触发
        let mut d = detector();
        assert!(!d.observe("今天天气不错哦", 0.5, 0.02, "今天天气不错"));
        assert!(!d.observe("今天天气不错哦", 0.5, 0.02, "今天天气不错"));
    }

    #[test]
    fn test_detector_threshold_boundary() {
        // 「abcde」vs「abxde」：bigram 交集 {ab,de}，Dice = 2*2/(4+4) = 0.5 恰为阈值
        // → 放行条件是 dice < threshold，== 阈值判回声（钉住边界方向）
        let mut d = detector();
        assert!(!d.observe("abcde", 0.5, 0.02, "abxde"));
        assert!(!d.observe("abcde", 0.5, 0.02, "abxde"));
    }

    #[test]
    fn test_detector_empty_echo_ref_passes_through() {
        // 无回声参考（Thinking 阶段未播句）→ 相似度条件恒过，其余满足即触发
        let mut d = detector();
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, ""));
        assert!(d.observe("今天天气怎么样", 0.5, 0.02, ""));
    }

    #[test]
    fn test_detector_reset_clears_hits() {
        let mut d = detector();
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
        d.reset();
        // reset 后需重新连续命中
        assert!(!d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
        assert!(d.observe("今天天气怎么样", 0.5, 0.02, "昨天很冷"));
    }

    // ---------- EchoTracker ----------

    #[test]
    fn test_echo_tracker_window_keeps_last_two() {
        let mut t = EchoTracker::default();
        t.record_played("第一句。");
        assert_eq!(t.reference(), "第一句。");
        t.record_played("第二句。");
        assert_eq!(t.reference(), "第一句。第二句。");
        t.record_played("第三句。");
        // 容量 2：最旧的「第一句」被挤出
        assert_eq!(t.reference(), "第二句。第三句。");
    }

    #[test]
    fn test_echo_tracker_take_spoken_clears() {
        let mut t = EchoTracker::default();
        t.record_played("第一句。");
        t.record_played("第二句。");
        assert_eq!(t.take_spoken(), "第一句。第二句。");
        assert_eq!(t.take_spoken(), "");
    }

    #[test]
    fn test_echo_tracker_clear_resets_both() {
        let mut t = EchoTracker::default();
        t.record_played("第一句。");
        t.clear();
        assert_eq!(t.reference(), "");
        assert_eq!(t.take_spoken(), "");
    }
}
