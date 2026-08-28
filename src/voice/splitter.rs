/// 句子切分器：把 LLM 流式 token 增量切成完整句子。
///
/// sherpa-onnx 的 TTS 是整句一次性合成（无流式 TTS API），因此句级流式播报的关键
/// 是**尽快**切出完整句子交给合成线程：LLM 还在生成后续句时，第一句已开始合成播放。
///
/// 切分规则：
/// - 边界字符（`。！？；` + 英文 `.!?;` + 换行）归属前一句（`"你好。"` 是一句）。
/// - ASCII `.` 前一字符是数字时**不作边界**：保护小数（`3.14` 不拆散）与有序列表
///   标记（`"1. 第一点"` 不切出垃圾句 `"1."`；列表前缀由 `sanitizer` 剥离）。
/// - 无边界持续超 `max_sentence_len` 时在最近空白/边界处兜底切，避免「整段不标点 →
///   永远不开始播报」。英文句点误判（缩写）对 TTS 无实质影响，接受。
///
/// 默认单句最大长度（无标点时的兜底切分阈值）。
pub const DEFAULT_MAX_SENTENCE_LEN: usize = 80;

/// 是否为句子边界字符（中文/英文句读 + 换行）。
pub fn is_sentence_boundary(c: char) -> bool {
    matches!(
        c,
        '。' | '！' | '？' | '；' | '．' | '…' | '.' | '!' | '?' | ';' | '\n'
    )
}

/// 上下文感知的边界判定：ASCII `.` 前一字符是数字时不作边界（保护小数 `3.14`
/// 与有序列表标记 `1.`，避免切出 `"1."` 这类垃圾句送合成）。
///
/// 只收窄 `.` 一个字符的行为：其余边界字符与既有语义完全一致。
fn is_boundary_after(prev: Option<char>, c: char) -> bool {
    if c == '.' && prev.is_some_and(|p| p.is_ascii_digit()) {
        return false;
    }
    is_sentence_boundary(c)
}

/// 是否为「兜底切分点」：句子边界或空白（切分点本身归属前一句）。
fn is_break_point(c: char) -> bool {
    is_sentence_boundary(c) || c.is_whitespace()
}

/// 流式句子切分器（保留未完成半句在内部 buffer）。
pub struct SentenceSplitter {
    buffer: String,
    max_sentence_len: usize,
}

impl Default for SentenceSplitter {
    fn default() -> Self {
        Self::new()
    }
}

impl SentenceSplitter {
    pub fn new() -> Self {
        Self::with_max_len(DEFAULT_MAX_SENTENCE_LEN)
    }

    pub fn with_max_len(max: usize) -> Self {
        Self {
            buffer: String::new(),
            max_sentence_len: max.max(1),
        }
    }

    /// 吸收一段增量文本，返回切分完成的句子（含结尾边界字符、已 trim）。
    ///
    /// 切出后的剩余文本保留在内部 buffer，下一段增量可能补全它（或由 `finish` 冲刷）。
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        let mut out = Vec::new();

        // 1) 按边界字符切分（边界归属前一句；ASCII `.` 数字前不作边界）
        let mut start = 0;
        let mut prev: Option<char> = None;
        for (i, c) in self.buffer.char_indices() {
            if is_boundary_after(prev, c) {
                let end = i + c.len_utf8();
                let sentence = self.buffer[start..end].trim();
                if !sentence.is_empty() {
                    out.push(sentence.to_string());
                }
                start = end;
            }
            prev = Some(c);
        }
        if start > 0 {
            self.buffer = self.buffer[start..].to_string();
        }

        // 2) 无边界超长兜底：在最近空白/边界处切，找不到则硬切
        while self.buffer.chars().count() > self.max_sentence_len {
            let limit = self
                .buffer
                .char_indices()
                .nth(self.max_sentence_len)
                .map(|(i, _)| i)
                .unwrap_or(self.buffer.len());
            let prefix = &self.buffer[..limit];
            // 从后往前找最近切分点（含它本身），否则硬切
            let cut = prefix
                .char_indices()
                .rev()
                .find(|&(_, c)| is_break_point(c))
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(limit);
            let sentence = self.buffer[..cut].trim();
            if !sentence.is_empty() {
                out.push(sentence.to_string());
            }
            self.buffer = self.buffer[cut..].to_string();
        }

        out
    }

    /// 生成结束，冲刷尾部残余（返回剩余文本；空串 = 无需合成）。
    pub fn finish(&mut self) -> String {
        let rest = self.buffer.trim().to_string();
        self.buffer.clear();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_splits_chinese_period() {
        let mut s = SentenceSplitter::new();
        assert_eq!(s.push("你好，世界。"), vec!["你好，世界。".to_string()]);
        assert_eq!(s.push("今天天气不错。"), vec!["今天天气不错。".to_string()]);
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_splits_multiple_boundaries() {
        let mut s = SentenceSplitter::new();
        let out = s.push("第一句！第二句？第三句；第四句。");
        assert_eq!(
            out,
            vec![
                "第一句！".to_string(),
                "第二句？".to_string(),
                "第三句；".to_string(),
                "第四句。".to_string(),
            ]
        );
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_splits_english_punctuation() {
        let mut s = SentenceSplitter::new();
        assert_eq!(
            s.push("Hello. World! "),
            vec!["Hello.".to_string(), "World!".to_string()]
        );
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_splits_newline() {
        let mut s = SentenceSplitter::new();
        assert_eq!(
            s.push("第一点\n第二点\n"),
            vec!["第一点".to_string(), "第二点".to_string()]
        );
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_ordered_list_marker_not_split() {
        // 「1.」的句点不作边界：整行随换行切出，列表前缀由 sanitizer 剥离
        let mut s = SentenceSplitter::new();
        assert_eq!(
            s.push("1. 第一点\n2. 第二点\n"),
            vec!["1. 第一点".to_string(), "2. 第二点".to_string()]
        );
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_decimal_not_split() {
        // 小数不拆散：整句保留到中文句号兜底（旧行为会切成「价格是 3.」+「14 元。」）
        let mut s = SentenceSplitter::new();
        assert_eq!(
            s.push("价格是 3.14 元。"),
            vec!["价格是 3.14 元。".to_string()]
        );
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_digit_period_across_pushes() {
        // 跨 token：「版本 2」+「. 更新内容」两段到达，`.` 在 buffer 内可见 prev，不切
        let mut s = SentenceSplitter::new();
        assert_eq!(s.push("版本 2"), Vec::<String>::new());
        assert_eq!(s.push(". 更新内容\n"), vec!["版本 2. 更新内容".to_string()]);
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_boundary_belongs_to_previous_sentence() {
        let mut s = SentenceSplitter::new();
        // 「。」归属前一句：不 trim 掉标点
        assert_eq!(s.push("你好。"), vec!["你好。".to_string()]);
    }

    #[test]
    fn test_incremental_finishes_at_boundary() {
        // 分多次喂同一句话，只有句号后才切出
        let mut s = SentenceSplitter::new();
        assert_eq!(s.push("你"), Vec::<String>::new());
        assert_eq!(s.push("好"), Vec::<String>::new());
        assert_eq!(s.push("。"), vec!["你好。".to_string()]);
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_finish_flushes_tail() {
        let mut s = SentenceSplitter::new();
        assert_eq!(s.push("这句话没有标点"), Vec::<String>::new());
        assert_eq!(s.finish(), "这句话没有标点");
    }

    #[test]
    fn test_overlong_without_boundary_falls_back() {
        let mut s = SentenceSplitter::with_max_len(10);
        // 20 个字符无标点，应兜底切成两段（每段 ≤ 10）
        let text = "啊".repeat(20);
        let out = s.push(&text);
        assert_eq!(
            out.len(),
            1,
            "一次 push 只兜底切一段，剩余留在 buffer 等下一段"
        );
        assert!(out[0].chars().count() <= 10);
        // finish 冲刷残余
        let tail = s.finish();
        assert!(!tail.is_empty());
        assert!(tail.chars().count() <= 10);
        // 两段合计覆盖全部原文
        assert_eq!(out[0].chars().count() + tail.chars().count(), 20);
    }

    #[test]
    fn test_overlong_breaks_at_whitespace() {
        let mut s = SentenceSplitter::with_max_len(8);
        // 在空白处兜底切，而不是硬切
        let out = s.push("一二三四五 六七八九十");
        assert_eq!(out, vec!["一二三四五".to_string()]);
        assert_eq!(s.finish(), "六七八九十");
    }

    #[test]
    fn test_empty_and_whitespace_safe() {
        let mut s = SentenceSplitter::new();
        assert_eq!(s.push(""), Vec::<String>::new());
        assert_eq!(s.push("   "), Vec::<String>::new());
        assert_eq!(s.push("\n\n"), Vec::<String>::new());
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn test_whitespace_only_sentence_skipped() {
        let mut s = SentenceSplitter::new();
        // 连续标点/空白之间没有内容，不产出空句子
        assert_eq!(s.push("。   。"), vec!["。".to_string(), "。".to_string()]);
    }

    #[test]
    fn test_is_sentence_boundary_table() {
        for c in ['。', '！', '？', '；', '.', '!', '?', ';', '\n', '．', '…'] {
            assert!(is_sentence_boundary(c), "{c:?} 应为边界");
        }
        for c in ['，', '、', '：', '"', ' ', '你', '1'] {
            assert!(!is_sentence_boundary(c), "{c:?} 不应为边界");
        }
    }
}
