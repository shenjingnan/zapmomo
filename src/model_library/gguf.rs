//! GGUF 文件名解析 / split 校验 / 推荐（纯业务逻辑，可单测）。
//!
//! - [`parse_gguf_identity`]：从 filename 解析 base_stem / quantization / shard。
//! - [`GgufFileIdentity`]：Artifact grouping 依据（base_stem + quantization + shard_group）。
//! - [`VariantRecommendation`]：推荐逻辑放业务层，绝不写死在 UI；无法可靠判断时不返回推荐。

/// llama.cpp 常见量化（按长度降序，避免 `Q4_K_M` 与 `Q4_0` 混淆）。
const GGUF_QUANTS: &[&str] = &[
    "IQ3_XXS", "IQ2_XXS", "IQ4_NL", "IQ4_XS", "IQ3_XS", "IQ2_XS", "IQ1_S", "IQ1_M", "IQ5_XS",
    "IQ5_M", "IQ6_S", "IQ6_M", "Q3_K_L", "Q3_K_S", "Q3_K_M", "Q4_K_S", "Q4_K_M", "Q5_K_S",
    "Q5_K_M", "Q2_K_S", "TQ1_0", "TQ2_0", "Q8_0", "Q8_K", "Q6_K", "Q5_0", "Q5_1", "Q4_0", "Q4_1",
    "Q3_K", "Q2_K", "F16", "F32", "BF16", "FP16", "F8", "Q8", "Q6", "Q5", "Q4", "Q3", "Q2",
];

/// 一个 GGUF 文件的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GgufFileIdentity {
    /// 去量化/去 shard 后的 base model stem（如 "Qwen3-4B-Instruct"）。
    pub base_stem: String,
    /// 量化（如 "Q4_K_M"；无法识别则为 None）。
    pub quantization: Option<String>,
    pub shard_index: Option<usize>,
    pub shard_total: Option<usize>,
}

/// 判断文件名是否为 GGUF（扩展名大小写不敏感）。
pub fn is_gguf_filename(name: &str) -> bool {
    name.to_lowercase().ends_with(".gguf")
}

/// 解析 GGUF 文件名身份。
///
/// 规则：
/// - 去掉 `.gguf` 与 `-NNNNN-of-NNNNN` shard 后缀。
/// - 从剩余部分按已知量化清单识别量化（长匹配优先，词边界为 `-`/`_`/`.`）。
/// - base_stem = 去量化后剩余部分（trim 尾部 `-`/`_`）。
pub fn parse_gguf_identity(filename: &str) -> Option<GgufFileIdentity> {
    if !is_gguf_filename(filename) {
        return None;
    }
    let stem = &filename[..filename.len() - ".gguf".len()];
    let (stem, shard) = strip_shard(stem);
    let (base_stem, quant) = extract_quant(&stem);
    let base_stem = if base_stem.is_empty() {
        stem
    } else {
        base_stem
    };
    Some(GgufFileIdentity {
        base_stem,
        quantization: quant,
        shard_index: shard.map(|(i, _)| i),
        shard_total: shard.map(|(_, t)| t),
    })
}

/// 提取 `-NNNNN-of-NNNNN`（容忍 `1-of-2` 非零填充），返回 (去后缀 stem, (index, total))。
fn strip_shard(stem: &str) -> (String, Option<(usize, usize)>) {
    let Some(pos) = stem.rfind("-of-") else {
        return (stem.to_string(), None);
    };
    let Ok(total) = stem[pos + 4..].trim().parse::<usize>() else {
        return (stem.to_string(), None);
    };
    let head = &stem[..pos];
    let Some(dash) = head.rfind('-') else {
        return (stem.to_string(), None);
    };
    let Ok(idx) = head[dash + 1..].parse::<usize>() else {
        return (stem.to_string(), None);
    };
    (head[..dash].to_string(), Some((idx, total)))
}

/// 从 stem 中提取量化（长匹配优先），返回 (去量化后的 base, 规范化量化)。
fn extract_quant(stem: &str) -> (String, Option<String>) {
    let upper = stem.to_uppercase();
    for q in GGUF_QUANTS {
        let Some(pos) = upper.find(q) else { continue };
        let before_ok = pos == 0 || matches!(stem.as_bytes()[pos - 1], b'-' | b'_' | b'.');
        let end = pos + q.len();
        let after_ok = end == stem.len() || matches!(stem.as_bytes()[end], b'-' | b'_' | b'.');
        if !before_ok || !after_ok {
            continue;
        }
        let canonical = canonical_quant(q);
        let base = format!("{}{}", &stem[..pos], &stem[end..]);
        let base = base.trim_end_matches(['-', '_', '.']).to_string();
        return (base, Some(canonical));
    }
    (stem.to_string(), None)
}

/// 规范化量化名（FP16 → F16）。
fn canonical_quant(raw: &str) -> String {
    let up = raw.to_uppercase();
    if up == "FP16" { "F16".to_string() } else { up }
}

/// 解析参数量（best-effort：匹配 `4B` / `1.5b` 等模式），拿不到返回 None。
pub fn parse_parameter_count(filename: &str) -> Option<String> {
    let lower = filename.to_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        if i > start && i + 1 < bytes.len() && bytes[i] == b'b' {
            let num: &str = &lower[start..i];
            if num.parse::<f64>().is_ok() && num.contains('.') {
                return Some(format!("{num}B"));
            }
            if num.bytes().all(|b| b.is_ascii_digit()) {
                return Some(format!("{num}B"));
            }
        }
        i = (i + 1).max(start + 1);
    }
    None
}

// ---------------------------------------------------------------------------
// VariantRecommendation（业务层）
// ---------------------------------------------------------------------------

/// 推荐优先级（越低越优先）：质量/内存/速度均衡 → Q4_K_M 典型。
const RECOMMEND_PRIORITY: &[&str] = &[
    "Q4_K_M", "Q5_K_M", "Q6_K", "Q4_0", "Q5_0", "Q8_0", "Q8_K", "Q3_K_M", "Q2_K", "F16",
];

fn quant_priority(quant: &str) -> usize {
    RECOMMEND_PRIORITY
        .iter()
        .position(|p| p.eq_ignore_ascii_case(quant))
        .unwrap_or(usize::MAX)
}

/// 从可用量化中推荐默认版本（无法可靠判断时返回 None，不推荐错误 Variant）。
pub fn recommend_quantization(quants: &[String]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, q) in quants.iter().enumerate() {
        let p = quant_priority(q);
        if p == usize::MAX {
            continue;
        }
        if best.is_none_or(|b| p < quant_priority(&quants[b])) {
            best = Some(i);
        }
    }
    best
}

/// 按设备资源推荐：在可用 RAM 预算内选择优先级最高的量化。
///
/// `available_ram_bytes` 为 None 时退化为无资源感知的 [`recommend_quantization`]。
pub fn recommend_for_ram(
    variants: &[(String, u64)],
    available_ram_bytes: Option<u64>,
) -> Option<usize> {
    match available_ram_bytes {
        Some(ram) => {
            let budget = ram / 2;
            let mut best: Option<usize> = None;
            for (i, (q, size)) in variants.iter().enumerate() {
                if *size <= budget {
                    let p = quant_priority(q);
                    if p == usize::MAX {
                        continue;
                    }
                    if best.is_none_or(|b| p < quant_priority(&variants[b].0)) {
                        best = Some(i);
                    }
                }
            }
            best
        }
        None => {
            let quants: Vec<String> = variants.iter().map(|(q, _)| q.clone()).collect();
            recommend_quantization(&quants)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_quant() {
        let id = parse_gguf_identity("Qwen3-4B-Q4_K_M.gguf").unwrap();
        assert_eq!(id.base_stem, "Qwen3-4B");
        assert_eq!(id.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(id.shard_index, None);
    }

    #[test]
    fn test_parse_instruct_quant() {
        let id = parse_gguf_identity("Qwen3-4B-Instruct-Q5_K_M.gguf").unwrap();
        assert_eq!(id.base_stem, "Qwen3-4B-Instruct");
        assert_eq!(id.quantization.as_deref(), Some("Q5_K_M"));
    }

    #[test]
    fn test_parse_shard_split() {
        let id = parse_gguf_identity("model-Q4_K_M-00001-of-00002.gguf").unwrap();
        assert_eq!(id.base_stem, "model");
        assert_eq!(id.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(id.shard_index, Some(1));
        assert_eq!(id.shard_total, Some(2));
    }

    #[test]
    fn test_parse_nonzero_padded_shard() {
        let id = parse_gguf_identity("foo-Q8_0-1-of-2.gguf").unwrap();
        assert_eq!(id.shard_index, Some(1));
        assert_eq!(id.shard_total, Some(2));
    }

    #[test]
    fn test_parse_fp16_normalized() {
        assert_eq!(
            parse_gguf_identity("model-fp16.gguf")
                .unwrap()
                .quantization
                .as_deref(),
            Some("F16")
        );
        assert_eq!(
            parse_gguf_identity("model-F16.gguf")
                .unwrap()
                .quantization
                .as_deref(),
            Some("F16")
        );
    }

    #[test]
    fn test_parse_unknown_quant_none() {
        let id = parse_gguf_identity("mystery-model.gguf").unwrap();
        assert_eq!(id.quantization, None);
        assert_eq!(id.base_stem, "mystery-model");
    }

    #[test]
    fn test_non_gguf_returns_none() {
        assert!(parse_gguf_identity("model.onnx").is_none());
        assert!(parse_gguf_identity("README.md").is_none());
    }

    #[test]
    fn test_quant_not_confused_q4_k_m_vs_q4_0() {
        assert_eq!(
            parse_gguf_identity("A-Q4_K_M.gguf")
                .unwrap()
                .quantization
                .as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(
            parse_gguf_identity("B-Q4_0.gguf")
                .unwrap()
                .quantization
                .as_deref(),
            Some("Q4_0")
        );
    }

    #[test]
    fn test_parameter_count() {
        assert_eq!(
            parse_parameter_count("Qwen3-4B-Q4_K_M.gguf").as_deref(),
            Some("4B")
        );
        assert_eq!(
            parse_parameter_count("Qwen3-1.5B-Instruct.gguf").as_deref(),
            Some("1.5B")
        );
        assert_eq!(parse_parameter_count("random-model.gguf"), None);
    }

    #[test]
    fn test_recommend_prefers_q4_k_m() {
        let quants = vec!["Q2_K".to_string(), "Q8_0".to_string(), "Q4_K_M".to_string()];
        let idx = recommend_quantization(&quants).unwrap();
        assert_eq!(quants[idx], "Q4_K_M");
    }

    #[test]
    fn test_recommend_falls_to_next_balanced() {
        let quants = vec!["Q2_K".to_string(), "Q8_0".to_string()];
        let idx = recommend_quantization(&quants).unwrap();
        // 无 Q4_K_M → Q8_0（Q8 优先于 Q2_K）
        assert_eq!(quants[idx], "Q8_0");
    }

    #[test]
    fn test_recommend_unknown_returns_none() {
        let quants = vec!["SUPERWEIRD".to_string()];
        assert!(recommend_quantization(&quants).is_none());
    }

    #[test]
    fn test_recommend_for_ram_respects_budget() {
        let variants = vec![
            ("Q4_K_M".to_string(), 2_000_000_000),
            ("Q8_0".to_string(), 4_000_000_000),
            ("Q2_K".to_string(), 1_000_000_000),
        ];
        // RAM 4GB → 预算 2GB → 只 Q4_K_M(2G) 与 Q2_K(1G) 可选 → 选 Q4_K_M
        let idx = recommend_for_ram(&variants, Some(4_000_000_000)).unwrap();
        assert_eq!(variants[idx].0, "Q4_K_M");
        // RAM 1GB → 预算 0.5GB → 都不满足 → None（宁可无推荐）
        assert!(recommend_for_ram(&variants, Some(1_000_000_000)).is_none());
    }
}
