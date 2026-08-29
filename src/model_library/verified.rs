//! ZapMomo Verified Registry（**验证 overlay**）。
//!
//! 只保存验证信息（repo 映射 / recommended variant / architecture hint / notes），
//! **不复制** `model_registry.json` 的完整模型定义（下载 URL / required files / install spec）。
//! 避免双 Registry 数据漂移：Model Registry = Model Definition；Verified Registry = Verification Overlay。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::catalog::normalize_repo_id;

/// Overlay 顶层。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifiedRegistry {
    #[serde(rename = "schema_version")]
    pub schema_version: u32,
    pub entries: Vec<VerifiedEntry>,
}

/// 单条验证信息。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerifiedEntry {
    /// 关联的 model_registry.json model_id（内置精选）。
    pub model_id: String,
    /// `huggingface` = 对应 HF repo；`null` = 纯内置（如 sherpa，来自 GitHub releases）。
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub repo_id: Option<String>,
    /// 能力类型（llm/asr/tts/kws）。
    #[serde(default)]
    pub model_type: Option<String>,
    /// architecture hint（如 llama-cpp-gguf / sherpa-asr-streaming-zipformer）。
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub recommended_variant: Option<String>,
    #[serde(default)]
    pub compatibility_notes: Option<String>,
}

const REGISTRY_JSON: &str = include_str!("../../models/verified_registry.json");

impl VerifiedRegistry {
    /// 编译期内嵌解析一次并缓存。
    pub fn builtin() -> &'static VerifiedRegistry {
        static CACHE: OnceLock<VerifiedRegistry> = OnceLock::new();
        CACHE.get_or_init(|| {
            serde_json::from_str(REGISTRY_JSON).expect("内嵌 verified registry 无效")
        })
    }

    pub fn all(&self) -> &[VerifiedEntry] {
        &self.entries
    }

    /// 该 repo 是否经过 ZapMomo 验证（repo_id 大小写不敏感）。
    pub fn is_verified_repo(&self, repo_id: &str) -> bool {
        self.entry_for_repo(repo_id).is_some()
    }

    pub fn entry_for_repo(&self, repo_id: &str) -> Option<&VerifiedEntry> {
        let key = normalize_repo_id(repo_id);
        self.entries.iter().find(|e| {
            e.repo_id
                .as_deref()
                .is_some_and(|r| normalize_repo_id(r) == key)
        })
    }

    pub fn entry_for_model(&self, model_id: &str) -> Option<&VerifiedEntry> {
        self.entries.iter().find(|e| e.model_id == model_id)
    }

    /// 已验证且映射到 HF 的 repo 列表（供内置精选注入 HF 搜索结果）。
    pub fn hf_repo_ids(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| e.repo_id.as_deref())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_parses_and_is_verified() {
        let reg = VerifiedRegistry::builtin();
        assert_eq!(reg.entries.len(), 3, "应与内置精选一一对应");
        // 不复制 model_registry 的定义字段：这里只应有验证字段
        assert!(
            reg.all()
                .iter()
                .all(|e| e.repo_id.is_none() || e.architecture.is_some())
        );
        assert!(!reg.is_verified_repo("Some/Unknown"));
    }

    #[test]
    fn test_entry_lookup() {
        let reg = VerifiedRegistry::builtin();
        let m = reg.entry_for_model("kws-zipformer-zh-en-3m").unwrap();
        assert!(m.repo_id.is_none(), "sherpa 内置无 HF repo");
        assert_eq!(m.model_type.as_deref(), Some("kws"));
    }

    #[test]
    fn test_hf_repo_ids_empty_after_llm_removal() {
        // 本地 LLM 推理移除后，verified overlay 不再含任何 HF repo 映射
        // （剩余 sherpa 内置均来自 GitHub releases，无 HF repo）
        let repos = VerifiedRegistry::builtin().hf_repo_ids();
        assert!(repos.is_empty(), "LLM 条目移除后不应再有 HF repo 映射");
        assert!(!repos.contains(&"kws-zipformer-zh-en-3m"));
    }
}
