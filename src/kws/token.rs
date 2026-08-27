//! 用户自定义关键词 → sherpa-onnx 可编码的 ppinyin token。
//!
//! 本项目模型（zipformer-zh-en）的 ppinyin 分词把每个汉字拆成「声母 + 韵母」：
//! `文` → `w` `én`，`索` → `s` `uǒ`。用户直接输入原始中文（如 `你好小智`）时，
//! 需要先转成这样的 token 序列，sherpa-onnx 才能编码；否则编码失败返回空指针流，
//! 后续喂音频会直接段错误。
//!
//! 用法：`encode_custom_keywords` 把用户输入（原始中文 / 已 tokenized 拼音 /
//! 带 `@` 显示词 / 多个关键词用 `/` 或换行分隔）统一编码成 sherpa 可接受的格式。
use std::collections::HashSet;
use std::path::Path;

use pinyin::ToPinyin;

use super::english;

/// 双字母声母（先匹配，避免 `zh` 被拆成 `z h`）。
const INITIALS_2: [&str; 3] = ["zh", "ch", "sh"];
/// 单字母声母（ppinyin 约定把 `y`/`w` 也当声母，如 `文` = `w én`）。
const INITIALS_1: [&str; 20] = [
    "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r", "z", "c", "s", "y",
    "w",
];

/// 无标准声母/韵母拆分的整音节（整个作为 token）。
const SPECIAL_SYLLABLES: [&str; 6] = ["hm", "hng", "ń", "ň", "ḿ", "ǹ"];

/// 是否为 CJK 汉字（基本区 + 扩展 A + 兼容区）。
fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{f900}'..='\u{faff}')
}

/// 读取 tokens.txt（每行 `token id`），返回 token 集合（取第一列）。
pub fn load_token_set(tokens_path: &Path) -> Result<HashSet<String>, String> {
    let content = std::fs::read_to_string(tokens_path)
        .map_err(|e| format!("无法读取 tokens.txt {}: {}", tokens_path.display(), e))?;
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_string)
        .collect())
}

/// 把一个带声调拼音音节（如 `nǐ`）拆成「声母 + 韵母」。
/// 返回 `(声母, 韵母)`；无声母时声母为空串。
fn split_syllable(syl: &str, tokens: &HashSet<String>) -> Result<(String, Option<String>), String> {
    if SPECIAL_SYLLABLES.contains(&syl) {
        return Ok((String::new(), Some(syl.to_string())));
    }
    for init in INITIALS_2 {
        if let Some(rest) = syl.strip_prefix(init)
            && !rest.is_empty()
            && tokens.contains(rest)
        {
            return Ok((init.to_string(), Some(rest.to_string())));
        }
    }
    for init in INITIALS_1 {
        if let Some(rest) = syl.strip_prefix(init)
            && !rest.is_empty()
            && tokens.contains(rest)
        {
            return Ok((init.to_string(), Some(rest.to_string())));
        }
    }
    // 无标准拆分：整个音节应是 tokens 中的韵母或特殊音节
    if tokens.contains(syl) {
        return Ok((String::new(), Some(syl.to_string())));
    }
    Err(format!(
        "无法把拼音 `{syl}` 拆分为模型 token（tokens.txt 中无匹配韵母）"
    ))
}

/// 把汉字文本转成 ppinyin token 序列。
///
/// 例：`你好小智` → `["n", "ǐ", "h", "ǎo", "x", "iǎo", "zh", "ì"]`
pub fn hanzi_to_ppinyin(text: &str, tokens: &HashSet<String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for p in text.to_pinyin() {
        match p {
            Some(p) => {
                let (init, fin) = split_syllable(p.with_tone(), tokens)?;
                if !init.is_empty() {
                    out.push(init);
                }
                if let Some(f) = fin {
                    out.push(f);
                }
            }
            None => {
                // 非汉字（空格、标点）跳过
            }
        }
    }
    if out.is_empty() {
        return Err(format!("文本 `{text}` 中没有可转换的汉字"));
    }
    Ok(out)
}

/// 校验 token 序列中的每个 token 都在模型 tokens 中。
fn validate_tokens(token_str: &str, tokens: &HashSet<String>) -> Result<(), String> {
    for tok in token_str.split_whitespace() {
        if !tokens.contains(tok) {
            return Err(format!(
                "token `{tok}` 不在模型 tokens.txt 中。\n\
                 请使用模型支持的拼音/音素格式，或直接输入中文由程序自动转换。"
            ));
        }
    }
    Ok(())
}

/// 当前模型支持的自定义唤醒词语言已固定为中英双语（zh-en 模型）：中文走 ppinyin，
/// 英文走 en.phone ARPAbet g2p（见 [`english`]）。
///
/// 把用户输入的自定义关键词编码成 sherpa-onnx 可接受的格式。
///
/// 支持输入：
/// - 原始中文（自动转 ppinyin token，显示词取原文）：`你好小智`
/// - 英文单词/短语（自动转 ARPAbet token，显示词用 `_` 连接）：`hi hello` → `HH AY1 HH AH0 L OW1 @hi_hello`
/// - 已 tokenized 拼音：`n ǐ h ǎo x iǎo zh ì`
/// - 已 tokenized 音素：`L AY1 T AH1 P`
/// - 显式显示词：`n ǐ h ǎo x iǎo zh ì @你好小智`
/// - 多个关键词：用 `/` 或换行分隔
pub fn encode_custom_keywords(input: &str, tokens_path: &Path) -> Result<String, String> {
    let tokens = load_token_set(tokens_path)?;
    // en.phone 与 tokens.txt 同在模型根目录（缺失时英文退化为纯 g2p）
    let en_phone_path = tokens_path.with_file_name("en.phone");
    let mut lines = Vec::new();
    for raw in input.split(['/', '\n']) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        lines.push(encode_keyword(raw, &tokens, &en_phone_path)?);
    }
    if lines.is_empty() {
        return Err("未提供任何关键词".to_string());
    }
    Ok(lines.join("\n"))
}

/// 编码单个关键词。
fn encode_keyword(
    raw: &str,
    tokens: &HashSet<String>,
    en_phone_path: &Path,
) -> Result<String, String> {
    // 拆出 `@` 后的显式显示词（可选）
    let (token_part, display) = match raw.rsplit_once('@') {
        Some((t, d)) => (t.trim(), Some(d.trim().to_string())),
        None => (raw, None),
    };
    let token_part_has_cjk = token_part.chars().any(is_cjk);

    // 原始中文 → ppinyin；已是 token 序列（手写拼音/音素）→ 透传；否则按英文单词转换。
    // `auto_display` 标记是否自动把原文作为显示词（中文与英文转换均需，纯 token 序列不需）。
    let (token_str, auto_display) = if token_part_has_cjk {
        (hanzi_to_ppinyin(token_part, tokens)?.join(" "), true)
    } else if is_token_sequence(token_part, tokens) {
        (token_part.to_string(), false)
    } else if looks_like_english_words(token_part, tokens) {
        (
            english::english_phrase_to_tokens(token_part, en_phone_path)?.join(" "),
            true,
        )
    } else {
        // 混合/含非字母（如手写音素打错）：透传，交由 validate_tokens 报清晰错误
        (token_part.to_string(), false)
    };

    validate_tokens(&token_str, tokens)?;

    // 自动显示词用 `_` 连接（sherpa-onnx 的关键词显示名不能含空格，模型自带如 `LIGHT_UP`）
    let display = display.or_else(|| {
        auto_display.then(|| token_part.split_whitespace().collect::<Vec<_>>().join("_"))
    });
    match display {
        Some(d) => Ok(format!("{token_str} @{d}")),
        None => Ok(token_str),
    }
}

/// 判断一段非中文输入是否「已是合法 token 序列」（每个空白分隔 token 都在模型 token 集中）。
fn is_token_sequence(s: &str, tokens: &HashSet<String>) -> bool {
    let parts: Vec<&str> = s.split_whitespace().collect();
    !parts.is_empty() && parts.iter().all(|p| tokens.contains(*p))
}

/// 判断一段非中文输入是否「看起来像英文自然词」（每个词都是纯 ASCII 字母且都不在 token 集中）。
///
/// 与 `is_token_sequence` 配合区分三种情况：全部合法 → 手写音素/拼音；
/// 全都不在集且是字母 → 英文单词；混合 → 视为拼写错误，透传交由 `validate_tokens` 报清晰错误。
fn looks_like_english_words(s: &str, tokens: &HashSet<String>) -> bool {
    let parts: Vec<&str> = s.split_whitespace().collect();
    !parts.is_empty()
        && parts.iter().all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphabetic()) && !tokens.contains(*p)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_tokens(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    /// 真实模型 tokens.txt 的简化版（`token id` 两列），覆盖测试用到的拼音 token。
    fn real_tokens() -> tempfile::NamedTempFile {
        temp_tokens(
            "<blk> 0\nAA0 3\nB 21\nn 131\nǚ 259\nǐ 251\nh 88\nǎo 249\nx 179\niǎo 124\nzh 182\nì 203\nf 86\nǎ 245\ng 87\nuó 163\n",
        )
    }

    #[test]
    fn test_load_token_set_takes_first_column() {
        let f = real_tokens();
        let set = load_token_set(f.path()).unwrap();
        assert!(set.contains("n"));
        assert!(set.contains("ǚ"));
        assert!(!set.contains("n 131"), "应取第一列，不含 id");
    }

    #[test]
    fn test_hanzi_to_ppinyin_ni_hao_xiao_zhi() {
        let f = real_tokens();
        let tokens = load_token_set(f.path()).unwrap();
        let out = hanzi_to_ppinyin("你好小智", &tokens).unwrap();
        assert_eq!(out, vec!["n", "ǐ", "h", "ǎo", "x", "iǎo", "zh", "ì"]);
    }

    #[test]
    fn test_encode_raw_chinese() {
        let f = real_tokens();
        let encoded = encode_custom_keywords("你好小智", f.path()).unwrap();
        assert_eq!(encoded, "n ǐ h ǎo x iǎo zh ì @你好小智");
    }

    #[test]
    fn test_encode_tokenized_pinyin() {
        let f = real_tokens();
        let encoded = encode_custom_keywords("n ǐ h ǎo x iǎo zh ì", f.path()).unwrap();
        assert_eq!(encoded, "n ǐ h ǎo x iǎo zh ì");
    }

    #[test]
    fn test_encode_with_explicit_display() {
        let f = real_tokens();
        let encoded = encode_custom_keywords("n ǐ h ǎo x iǎo zh ì @测试", f.path()).unwrap();
        assert_eq!(encoded, "n ǐ h ǎo x iǎo zh ì @测试");
    }

    #[test]
    fn test_encode_multiple_keywords_slash_separated() {
        let f = real_tokens();
        let encoded = encode_custom_keywords("你好/法国", f.path()).unwrap();
        // 你好 = n ǐ h ǎo；法国 = f ǎ g uó
        assert_eq!(encoded, "n ǐ h ǎo @你好\nf ǎ g uó @法国");
    }

    #[test]
    fn test_encode_invalid_token_errors() {
        let f = real_tokens();
        // 混合：`n` 在 token 集中、`zz` 不在 → 视为手写音素打错，透传后由 validate 报清晰错误
        let err = encode_custom_keywords("n zz", f.path()).unwrap_err();
        assert!(err.contains("zz"), "err: {err}");
    }

    #[test]
    fn test_encode_english_phrase_via_dict() {
        let dir = tempfile::tempdir().unwrap();
        let tokens_path = dir.path().join("tokens.txt");
        std::fs::write(
            &tokens_path,
            "<blk> 0\nHH 36\nAY1 19\nAH0 9\nL 45\nOW1 50\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("en.phone"),
            "HI HH AY1\nHELLO HH AH0 L OW1\n",
        )
        .unwrap();

        // 词典命中：hi → HH AY1、hello → HH AH0 L OW1；显示词取原文
        let encoded = encode_custom_keywords("hi hello", &tokens_path).unwrap();
        assert_eq!(encoded, "HH AY1 HH AH0 L OW1 @hi_hello");
    }

    #[test]
    fn test_encode_manual_arpabet_passthrough() {
        let f = temp_tokens("<blk> 0\nL 45\nAY1 19\nT 59\nAH1 10\nP 55\n");
        let encoded = encode_custom_keywords("L AY1 T AH1 P", f.path()).unwrap();
        assert_eq!(encoded, "L AY1 T AH1 P");
    }

    #[test]
    fn test_encode_empty_errors() {
        let f = real_tokens();
        assert!(encode_custom_keywords("", f.path()).is_err());
        assert!(encode_custom_keywords("  \n/ ", f.path()).is_err());
    }

    #[test]
    fn test_is_cjk_boundaries() {
        assert!(is_cjk('大'));
        assert!(is_cjk('㐀'));
        assert!(!is_cjk('A'));
        assert!(!is_cjk('ǐ'));
    }
}
