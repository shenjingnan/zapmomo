//! 模型库共享基础类型。
//!
//! 「模型库」页面（HF 在线目录）已移除，本文件只保留弹窗链路仍引用的类型：
//! `ModelCategory`（install 元数据）、`CompatibilityLevel`（Verified 徽章）、
//! `normalize_repo_id`（verified registry 归一化）。

use serde::{Deserialize, Serialize};

/// 模型分类（LLM/ASR/TTS/KWS）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCategory {
    Llm,
    Asr,
    Tts,
    Kws,
}

impl ModelCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelCategory::Llm => "llm",
            ModelCategory::Asr => "asr",
            ModelCategory::Tts => "tts",
            ModelCategory::Kws => "kws",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "llm" => Some(ModelCategory::Llm),
            "asr" => Some(ModelCategory::Asr),
            "tts" => Some(ModelCategory::Tts),
            "kws" => Some(ModelCategory::Kws),
            _ => None,
        }
    }
}

/// 兼容性等级：
/// - `Verified`：ZapMomo 人工验证过的模型（verified_registry）。
/// - `Compatible`：已检查 runtime/format/required files/文件结构，可确认能运行。
/// - `Possible`：仅凭 summary（pipeline/tags/library/format）看起来可能兼容，未查文件，不保证。
/// - `Unsupported`：已检查并明确发现 runtime 不支持 / 缺 required files / 格式不支持。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityLevel {
    Verified,
    Compatible,
    Possible,
    Unsupported,
}

impl CompatibilityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompatibilityLevel::Verified => "verified",
            CompatibilityLevel::Compatible => "compatible",
            CompatibilityLevel::Possible => "possible",
            CompatibilityLevel::Unsupported => "unsupported",
        }
    }
}

/// repoId 归一化（仅用于去重 key，API/展示仍用原始 repo_id）。
/// 大小写不敏感（与 `paths_equal` 在 Windows 上的语义一致），去首尾空白。
pub fn normalize_repo_id(repo_id: &str) -> String {
    repo_id.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_category_roundtrip() {
        for cat in [
            ModelCategory::Llm,
            ModelCategory::Asr,
            ModelCategory::Tts,
            ModelCategory::Kws,
        ] {
            assert_eq!(ModelCategory::from_str_value(cat.as_str()), Some(cat));
        }
        assert_eq!(ModelCategory::from_str_value("unknown"), None);
    }

    #[test]
    fn test_normalize_repo_id() {
        assert_eq!(
            normalize_repo_id("Qwen/Qwen3-4B-GGUF"),
            "qwen/qwen3-4b-gguf"
        );
        assert_eq!(
            normalize_repo_id("  Qwen/Qwen3-4B-GGUF "),
            normalize_repo_id("qwen/qwen3-4b-gguf")
        );
    }
}
