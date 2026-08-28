/// TTS 输入文本清洗：剥离 markdown/emoji 等无意义符号，丢弃不可朗读句。
///
/// 分层原则：[`super::splitter::SentenceSplitter`] 只管切分（纯标点句照旧产出）；
/// 符号剥离与整句丢弃是本模块职责。清洗只作用于**合成入队句**——
/// 会话历史与上屏 token（`ReplyAccumulator::text`）保持 LLM 原文，不受影响。
///
/// 价值：垃圾句（`"1."`、`"#"`、`"***"`、纯 emoji）会占据串行合成线程整句合成
/// 的时长（秒级），拖慢首响；markdown 符号与 emoji 会被读出或产生异常发音。
///
/// 实现约束：无 regex 依赖（项目约定，同 `asr::offline::clean_sensevoice_text`），
/// 纯字符扫描。三个入口：
/// - [`sanitize_sentence`]：无状态单句清洗（五步管线，可能返回空串）；
/// - [`TtsSanitizer`]：带 fence 状态（``` 代码块整块丢弃），供流式逐句调用；
/// - [`sanitize_for_tts`]：整段一次性清洗（按行 + fence 状态机），供 GUI 试听
///   与欢迎语复用。
///
/// 典型效果：`"1. 第一点"` → `"第一点"`；`"# 标题"` → `"标题"`；
/// `"看 *这个*"` → `"看 这个"`；`"[文档](https://a.com)"` → `"文档"`；
/// `"你好😀"` → `"你好"`；```` ```py\ncode\n``` ```` → 整块丢弃。
use std::iter::Peekable;
use std::str::Chars;

/// 单句清洗（无状态）：空白归一 → 句首前缀剥离 → 行内链接/URL → 符号/emoji 删除
/// → 空白归一。返回清洗后文本（可能为空串；丢弃判定在 [`TtsSanitizer::sanitize`]）。
pub fn sanitize_sentence(text: &str) -> String {
    // 1) 空白归一：后续前缀匹配依赖规整后的首字符
    let s = normalize_whitespace(text);
    // 2) 句首 markdown 前缀（标题/列表/引用）
    let s = strip_block_prefix(&s);
    // 3) 行内结构：图片删除、链接留文字、裸 URL 删除
    let s = strip_inline_links(&s);
    // 4) 装饰符号（*/`/删除线）与 emoji
    let s = strip_marks_and_decorative(&s);
    // 5) 删除残留后的空白收敛
    normalize_whitespace(&s)
}

/// 整段一次性清洗：按行喂 [`TtsSanitizer`]（markdown 的 fence 天然是行粒度），
/// 丢弃不可朗读行，剩余以空格拼接。供 GUI 试听 command 与欢迎语等「整段文本」
/// 场景复用；空结果返回空串。
pub fn sanitize_for_tts(text: &str) -> String {
    let mut st = TtsSanitizer::default();
    text.lines()
        .filter_map(|l| st.sanitize(l))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 带状态清洗器：跟踪 ``` 围栏状态（代码块整块丢弃），供流式链路逐句调用。
///
/// 句子以边界字符收尾是完整单元，fence 行在流式下独立成句（`\n` 是切分边界），
/// 因此状态只需句粒度流转；循环仅为覆盖「正文：```py```」同行混排的罕见形态。
#[derive(Default)]
pub struct TtsSanitizer {
    in_code_block: bool,
}

impl TtsSanitizer {
    /// 清洗一句：返回 `None` = 整句丢弃（代码块内容、裸列表标记残余、
    /// 或清洗后无可朗读内容）。
    pub fn sanitize(&mut self, sentence: &str) -> Option<String> {
        let mut kept: Vec<String> = Vec::new();
        let mut rest = sentence;
        loop {
            if self.in_code_block {
                match rest.strip_prefix("```") {
                    // 闭 fence：复位并继续处理尾随正文（如 "``` 结果如下"）
                    Some(tail) => {
                        self.in_code_block = false;
                        rest = tail;
                    }
                    // 仍在代码块内。守卫：以中文句末标点收尾的长句判定为「误开块」
                    // （LLM 忘写闭 fence，代码行极少以中文句号收尾）——自动闭块当
                    // 正文朗读，避免本轮剩余句子被静默吞掉
                    None => {
                        if looks_like_prose_tail(rest) {
                            self.in_code_block = false;
                            tracing::warn!("检测到未闭合代码块，按正文朗读（疑似漏写闭 fence）");
                            break;
                        }
                        return finish_kept(kept);
                    }
                }
            }
            match rest.find("```") {
                Some(i) => {
                    let head = sanitize_sentence(&rest[..i]);
                    if !head.is_empty() {
                        kept.push(head);
                    }
                    // fence 后到下一个 ``` 之间是 info string（语言标注），丢弃
                    let tail = &rest[i + 3..];
                    match tail.find("```") {
                        // 同行闭合（"```py```"）：继续处理块后文本
                        Some(j) => rest = &tail[j + 3..],
                        // 开 fence：块内句子交给后续调用丢弃
                        None => {
                            self.in_code_block = true;
                            return finish_kept(kept);
                        }
                    }
                }
                None => {
                    let head = sanitize_sentence(rest);
                    if !head.is_empty() {
                        kept.push(head);
                    }
                    return finish_kept(kept);
                }
            }
        }
        // 「误开块」守卫路径：当前句整体当正文清洗
        let head = sanitize_sentence(rest);
        if !head.is_empty() {
            kept.push(head);
        }
        finish_kept(kept)
    }
}

/// 拼接保留片段并过闸门；空、无可朗读内容、或裸列表标记残余 → `None`（整句丢弃）。
fn finish_kept(kept: Vec<String>) -> Option<String> {
    let joined = kept.join(" ");
    if !joined.is_empty() && is_speakable(&joined) && !is_bare_list_marker(&joined) {
        Some(joined)
    } else {
        None
    }
}

/// 「误开块」守卫判定：句长超过常见代码行且以中文句末标点收尾（只取中文标点，
/// 不含 `.`/`\n`——代码行可能合法地以分号/点收尾）。
fn looks_like_prose_tail(s: &str) -> bool {
    let trimmed = normalize_whitespace(s);
    trimmed.chars().count() > 12 && trimmed.ends_with(['。', '！', '？', '；'])
}

/// 是否为裸列表标记残余（`"1."`、`"12、"`、`"3）"`）：splitter 已保证数字后的
/// ASCII `.` 不切句，这类孤立短句只可能是列表编号残余（或行尾冲刷产物），
/// 朗读无意义。小数（`3.14`）不会命中——数字串不含定界符。
fn is_bare_list_marker(s: &str) -> bool {
    let mut chars = s.chars();
    let digits = chars.by_ref().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && digits <= 3
        && chars
            .as_str()
            .chars()
            .all(|c| matches!(c, '.' | '．' | '、' | ')' | '）'))
}

/// 空白归一：tab/回车/不换行空格/全角空格及连续空白 → 单 ASCII 空格，去首尾。
/// （U+00A0/U+3000 显式列出以自文档化，二者本身也在 `is_whitespace` 内。）
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c == '\u{00A0}' || c == '\u{3000}' {
            pending_space = !out.is_empty(); // 首部空白不产出
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

/// 剥句首 markdown 块前缀（循环剥至稳定，上限 4 轮防病态输入）：
/// 标题 `#{1,6}`、bullet（`-` `*` `+` `•` `·`）、引用 `>`、有序列表 `数字+定界符`。
fn strip_block_prefix(s: &str) -> String {
    let mut cur: &str = s;
    for _ in 0..4 {
        let next = strip_block_prefix_once(cur);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur.to_string()
}

fn strip_block_prefix_once(s: &str) -> &str {
    let t = s.trim_start();
    let mut chars = t.chars();
    match chars.next() {
        // 标题：# 计数 1..=6，后必须空白（`#tag`、`#1 是答案` 不动）
        Some('#') => {
            let hashes = t.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) {
                let rest = &t[hashes..]; // '#' 是 ASCII，字节数 = 个数
                if rest.starts_with(' ') {
                    return rest.trim_start();
                }
            }
            t
        }
        // bullet：标记后必须空白（`*加粗*` 不匹配，由符号删除处理）
        Some('-' | '*' | '+' | '•' | '·') => {
            let mark = t.chars().next().unwrap();
            let rest = &t[mark.len_utf8()..];
            if rest.starts_with(' ') {
                rest.trim_start()
            } else {
                t
            }
        }
        // 引用：循环剥 `>`（`>> 内容`）
        Some('>') => t.trim_start_matches('>').trim_start(),
        // 有序列表：1..=3 位数字 + 定界符；ASCII `.` 必须后随空白（保护 `3.14`），
        // 中文定界符（`、`/`)`/`）`）可省空白（`1、内容` 惯例）
        Some(c) if c.is_ascii_digit() => {
            let digits = t.chars().take_while(|ch| ch.is_ascii_digit()).count();
            if digits > 3 {
                return t;
            }
            let rest = &t[digits..]; // 数字是 ASCII，字节数 = 个数
            let mut it = rest.chars();
            match it.next() {
                Some('.') => {
                    let after = &rest[1..]; // '.' 是 ASCII
                    if after.starts_with(' ') {
                        after.trim_start()
                    } else {
                        t
                    }
                }
                Some('、' | ')' | '）') => it.as_str().trim_start(),
                _ => t,
            }
        }
        _ => t,
    }
}

/// 行内结构：图片 `![alt](url)` → 删；链接 `[text](url)` → `text`；
/// 裸 `http(s)://` URL → 删（补空格防前后词粘连）。
fn strip_inline_links(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        // 图片优先于链接（`![` 是 `[` 的前缀形态）
        if chars[i] == '!'
            && chars.get(i + 1) == Some(&'[')
            && let Some((_, end)) = match_markdown_link(&chars, i + 1)
        {
            i = end;
            continue;
        }
        if chars[i] == '['
            && let Some((text, end)) = match_markdown_link(&chars, i)
        {
            out.push_str(&text);
            i = end;
            continue;
        }
        if matches!(chars[i], 'h' | 'H') && starts_with_url(&chars, i) {
            i += url_len(&chars, i);
            // 补空格：URL 删除后前后英文词不粘连（末步 normalize_whitespace 收敛）
            out.push(' ');
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// 从 `chars[start] == '['` 起匹配 `[text](url)`，返回 (链接文字, 结束后索引)。
/// 嵌套方括号不处理（取第一个 `]`）；不匹配返回 `None`（`[` 原样输出）。
fn match_markdown_link(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut j = start + 1;
    let mut text = String::new();
    while j < chars.len() && chars[j] != ']' {
        text.push(chars[j]);
        j += 1;
    }
    if j >= chars.len() || chars.get(j + 1) != Some(&'(') {
        return None;
    }
    let mut k = j + 2;
    while k < chars.len() && chars[k] != ')' {
        k += 1;
    }
    if k >= chars.len() {
        return None;
    }
    Some((text, k + 1))
}

/// `chars[i..]` 是否以 `http://` / `https://` 开头（大小写不敏感）。
fn starts_with_url(chars: &[char], i: usize) -> bool {
    let head: String = chars
        .iter()
        .skip(i)
        .take(8) // 覆盖 "https://" 全长
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("http://") || head.starts_with("https://")
}

/// URL 安全字符长度（消费到空白/CJK/全角标点即停）。
fn url_len(chars: &[char], i: usize) -> usize {
    chars[i..]
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'*+,;=%".contains(c))
        .count()
}

/// 删除装饰符号：`*` 与 `` ` `` 全删（直接删除即无配对问题）；`~~删除线~~` 成对删、
/// 单个 `~`/`～` 保留（「3~5 天」范围语义）；再删 emoji/装饰区段字符。
fn strip_marks_and_decorative(s: &str) -> String {
    let no_tilde = strip_strikethrough(s);
    no_tilde
        .chars()
        .filter(|&c| c != '*' && c != '`' && !is_decorative(c))
        .collect()
}

/// 成对 `~~text~~` → `text`；未配对 `~~` 与单个 `~`/`～` 原样保留。
fn strip_strikethrough(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars: Peekable<Chars> = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' && chars.peek() == Some(&'~') {
            chars.next();
            // 找配对 `~~`；找不到则把已消费的 `~~` 与残余原样吐回
            let mut body = String::new();
            let mut closed = false;
            while let Some(inner) = chars.next() {
                if inner == '~' && chars.peek() == Some(&'~') {
                    chars.next();
                    closed = true;
                    break;
                }
                body.push(inner);
            }
            if closed {
                out.push_str(&body);
            } else {
                out.push_str("~~");
                out.push_str(&body);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 是否为 emoji/装饰符号（朗读无意义）。不碰：CJK 标点、全角形式、带圈数字。
fn is_decorative(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x00A9 | 0x00AE | 0x2122          // © ® ™
        | 0x200D                            // ZWJ（emoji 组合连接符）
        | 0x2190..=0x21FF                   // 箭头（→ 等）
        | 0x2300..=0x23FF                   // 杂项技术（⌘ ⏱）
        | 0x2600..=0x27BF                   // 杂项符号 + 装饰符号（☀ ⚠ ✅ ❤）
        | 0x2B00..=0x2BFF                   // 杂项符号与箭头（⭐ ⬆）
        | 0xFE00..=0xFE0F                   // 变体选择符 VS15/VS16
        | 0x20E3                            // 组合键帽
        | 0x1F000..=0x1FAFF                 // emoji 主区（含肤色修饰符/区域指示符）
        | 0x3030 | 0x303D | 0x3297 | 0x3299 // 〰 〽 日文祝/秘
    )
}

/// 是否含可朗读字符（显式区段白名单）。不用 `char::is_alphanumeric()` 一把梭：
/// 它会把 ①/² 等装饰字符当可读，导致纯装饰句漏过闸门。
pub fn is_speakable(text: &str) -> bool {
    text.chars().any(is_speakable_char)
}

fn is_speakable_char(c: char) -> bool {
    let u = c as u32;
    c.is_ascii_alphanumeric()
        || matches!(u,
            0x4E00..=0x9FFF    // CJK 统一表意
            | 0x3400..=0x4DBF  // 扩展 A
            | 0xF900..=0xFAFF  // 兼容表意
            | 0x3040..=0x30FF  // 平假名/片假名
            | 0x31F0..=0x31FF  // 片假名音标扩展
            | 0xFF10..=0xFF19  // 全角数字
            | 0xFF21..=0xFF3A  // 全角大写字母
            | 0xFF41..=0xFF5A  // 全角小写字母
            | 0xAC00..=0xD7AF  // 谚文音节
            | 0x0400..=0x04FF  // 西里尔
            | 0x0370..=0x03FF  // 希腊
        )
}

#[cfg(test)]
mod tests {
    use super::super::splitter::is_sentence_boundary;
    use super::*;

    // ---------- sanitize_sentence：句首前缀 ----------

    #[test]
    fn test_strip_heading_prefix() {
        assert_eq!(sanitize_sentence("# 标题"), "标题");
        assert_eq!(sanitize_sentence("## 二级标题"), "二级标题");
        assert_eq!(sanitize_sentence("###### 六级"), "六级");
    }

    #[test]
    fn test_heading_negative_cases() {
        // `#tag`、`#1 是答案`：# 后无空白，不是标题
        assert_eq!(sanitize_sentence("#tag 是话题"), "#tag 是话题");
        assert_eq!(sanitize_sentence("#1 是答案"), "#1 是答案");
    }

    #[test]
    fn test_strip_bullet_prefix() {
        assert_eq!(sanitize_sentence("- 项目"), "项目");
        assert_eq!(sanitize_sentence("* 星号项"), "星号项");
        assert_eq!(sanitize_sentence("+ 加号项"), "加号项");
        assert_eq!(sanitize_sentence("• 要点"), "要点");
        assert_eq!(sanitize_sentence("· 要点"), "要点");
    }

    #[test]
    fn test_bullet_negative_emphasis_not_stripped() {
        // `*加粗*` 无空白，不按 bullet 剥；星号由符号删除处理
        assert_eq!(sanitize_sentence("*加粗* 文本"), "加粗 文本");
    }

    #[test]
    fn test_strip_quote_prefix() {
        assert_eq!(sanitize_sentence("> 引用"), "引用");
        assert_eq!(sanitize_sentence(">> 多层引用"), "多层引用");
    }

    #[test]
    fn test_strip_ordered_list_prefix() {
        assert_eq!(sanitize_sentence("1. 第一点"), "第一点");
        assert_eq!(sanitize_sentence("12) 第十二"), "第十二");
        assert_eq!(sanitize_sentence("1、第一点"), "第一点");
        assert_eq!(sanitize_sentence("3）中文括号"), "中文括号");
    }

    #[test]
    fn test_ordered_prefix_negative_decimal() {
        // `3.14`：句点后无空白，不剥序号
        assert_eq!(sanitize_sentence("3.14 是圆周率"), "3.14 是圆周率");
    }

    #[test]
    fn test_stacked_prefixes() {
        // 引用里套列表：循环剥至稳定
        assert_eq!(sanitize_sentence("> - 嵌套项"), "嵌套项");
    }

    // ---------- sanitize_sentence：行内 ----------

    #[test]
    fn test_inline_emphasis_marks_removed() {
        assert_eq!(
            sanitize_sentence("看 *这个* 和 **那个***"),
            "看 这个 和 那个"
        );
        assert_eq!(sanitize_sentence("未配对 * 星号"), "未配对 星号");
    }

    #[test]
    fn test_inline_code_backticks_removed() {
        assert_eq!(sanitize_sentence("`npm run dev` 命令"), "npm run dev 命令");
    }

    #[test]
    fn test_strikethrough() {
        assert_eq!(sanitize_sentence("~~删除~~保留~范围~"), "删除保留~范围~");
    }

    #[test]
    fn test_link_keeps_text() {
        assert_eq!(sanitize_sentence("[链接](https://a.com) 文本"), "链接 文本");
    }

    #[test]
    fn test_image_removed() {
        assert_eq!(sanitize_sentence("![图](https://a.com/p.png) 描述"), "描述");
    }

    #[test]
    fn test_bare_url_removed() {
        assert_eq!(
            sanitize_sentence("详见 https://a.com/x?y=1，然后"),
            "详见 ，然后"
        );
        assert_eq!(
            sanitize_sentence("打开 HTTP://A.COM/Path 之后"),
            "打开 之后"
        );
    }

    #[test]
    fn test_unmatched_bracket_preserved() {
        // `[i]` 后不是 `(`，非 markdown 链接形态，原样保留
        assert_eq!(sanitize_sentence("数组 [i] 取值"), "数组 [i] 取值");
    }

    // ---------- sanitize_sentence：emoji ----------

    #[test]
    fn test_emoji_removed() {
        assert_eq!(sanitize_sentence("你好😀！"), "你好！");
        assert_eq!(sanitize_sentence("A → B → C"), "A B C");
        assert_eq!(sanitize_sentence("版权©️说明"), "版权说明");
    }

    #[test]
    fn test_emoji_zwj_and_modifier_removed() {
        // 肤色修饰符 + ZWJ 家庭组合 + 区域指示符（国旗）
        assert_eq!(sanitize_sentence("赞👍🏻"), "赞");
        assert_eq!(sanitize_sentence("家庭👨‍👩‍👧组合"), "家庭组合");
        assert_eq!(sanitize_sentence("旗🇨🇳子"), "旗子");
    }

    // ---------- 可朗读闸门 ----------

    #[test]
    fn test_is_speakable_table() {
        assert!(!is_speakable("。！？"));
        assert!(!is_speakable("😊 →"));
        assert!(!is_speakable("①②③")); // 带圈数字：白名单外的装饰字符
        assert!(is_speakable("中文"));
        assert!(is_speakable("abc 123"));
        assert!(is_speakable("１２３")); // 全角数字
        assert!(is_speakable("ひらがな"));
        assert!(is_speakable("한국어"));
    }

    // ---------- 空白 ----------

    #[test]
    fn test_whitespace_normalized() {
        assert_eq!(sanitize_sentence("a\tb　c"), "a b c");
        assert_eq!(sanitize_sentence("  多  个   空格  "), "多 个 空格");
    }

    // ---------- 中文回归 ----------

    #[test]
    fn test_plain_chinese_untouched() {
        assert_eq!(sanitize_sentence("你好，世界。"), "你好，世界。");
        assert_eq!(sanitize_sentence("范围 3~5 天"), "范围 3~5 天");
    }

    // ---------- TtsSanitizer：闸门与 fence 状态机 ----------

    #[test]
    fn test_symbol_only_sentence_dropped() {
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("。   。"), None); // 纯标点
        assert_eq!(st.sanitize("①②③"), None); // 纯装饰
        assert_eq!(st.sanitize("# 🔥"), None); // 清洗后无残余
        assert_eq!(st.sanitize("***"), None);
    }

    #[test]
    fn test_bare_list_marker_dropped() {
        // splitter 微调后孤立 "1." 只可能是列表标记残余（如 finish 冲刷产物）
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("1."), None);
        assert_eq!(st.sanitize("12、"), None);
        assert_eq!(st.sanitize("3）"), None);
        // 小数不误伤
        assert_eq!(st.sanitize("3.14"), Some("3.14".to_string()));
    }

    #[test]
    fn test_ordered_list_item_cleaned() {
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("1. 第一点"), Some("第一点".to_string()));
    }

    #[test]
    fn test_code_block_dropped_across_sentences() {
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("如下："), Some("如下：".to_string()));
        assert_eq!(st.sanitize("```python"), None); // 开 fence
        assert_eq!(st.sanitize("print(1)"), None); // 块内
        assert_eq!(st.sanitize("```"), None); // 闭 fence
        assert_eq!(st.sanitize("正文恢复"), Some("正文恢复".to_string()));
    }

    #[test]
    fn test_close_fence_with_tail_text() {
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("```rust"), None);
        // 闭 fence 带尾随正文同行
        assert_eq!(st.sanitize("``` 结果如下"), Some("结果如下".to_string()));
    }

    #[test]
    fn test_inline_fence_same_line() {
        // 同行混排：正文：```py```完毕（罕见形态）
        let mut st = TtsSanitizer::default();
        assert_eq!(
            st.sanitize("代码：```py```完毕"),
            Some("代码： 完毕".to_string())
        );
        assert!(!st.in_code_block);
    }

    #[test]
    fn test_unclosed_fence_prose_guard() {
        // LLM 忘写闭 fence：中文长句触发自动闭块，按正文朗读而非静默吞掉
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("```rust"), None);
        let prose = "这个函数用于计算结果并返回列表。";
        assert_eq!(st.sanitize(prose), Some(sanitize_sentence(prose)));
        assert!(!st.in_code_block);
    }

    #[test]
    fn test_unclosed_fence_short_code_line_still_dropped() {
        // 短代码行（无中文句末标点）仍按块内丢弃
        let mut st = TtsSanitizer::default();
        assert_eq!(st.sanitize("```"), None);
        assert_eq!(st.sanitize("let x = 1;"), None);
    }

    // ---------- sanitize_for_tts：整段入口 ----------

    #[test]
    fn test_sanitize_for_tts_full_document() {
        let doc = "# 标题\n1. 第一点\n2. 第二点\n```py\ncode\n```\n你好😀！";
        assert_eq!(sanitize_for_tts(doc), "标题 第一点 第二点 你好！");
    }

    #[test]
    fn test_sanitize_for_tts_empty_result() {
        assert_eq!(sanitize_for_tts("```\n```\n😀"), "");
    }

    #[test]
    fn test_sanitize_for_tts_plain_text_untouched() {
        assert_eq!(
            sanitize_for_tts("你好，世界。\n第二行。"),
            "你好，世界。 第二行。"
        );
    }

    // ---------- 与 splitter 边界集合的一致性守卫 ----------

    #[test]
    fn test_boundary_chars_still_recognized() {
        // splitter 改动边界集合时此处会提醒复审查守卫/清单是否需要跟进
        for c in ['。', '！', '？', '；', '．', '…', '.', '!', '?', ';', '\n'] {
            assert!(is_sentence_boundary(c), "{c:?} 应为边界");
        }
    }
}
