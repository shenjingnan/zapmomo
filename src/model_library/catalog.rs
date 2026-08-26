//! 模型目录（Catalog）领域类型。
//!
//! 只定义 **Provider 无关** 的类型与 trait：`CatalogProvider`、`CatalogQuery`、
//! `RemoteModelSummary/Detail/File`、`CanonicalModelKey`、`CompatibilityLevel`、
//! `UnifiedModelItem`。Hugging Face 专属实现见 [`super::huggingface`]。
//!
//! 关键边界（详见模块顶层文档）：
//! - `RemoteModel*`：模型**原始数据**（字段可空，绝不伪造）。
//! - `CompatibilityLevel`：ZapMomo 兼容性判断（四级，无第五级）。
//! - `LocalModelInstall`（见 `install.rs`）：本地安装事实。
//! - 三者合并为 `UnifiedModelItem` 展示，但代码层不混为一个来源。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Catalog Provider
// ---------------------------------------------------------------------------

/// CatalogProvider 标识（模型信息从哪里发现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogProviderId {
    /// 内置精选（无在线 Provider，来自本地 registry）。
    Builtin,
    /// Hugging Face Hub。
    HuggingFace,
    /// ModelScope（预留，未来）。
    ModelScope,
}

impl CatalogProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            CatalogProviderId::Builtin => "builtin",
            CatalogProviderId::HuggingFace => "huggingface",
            CatalogProviderId::ModelScope => "modelscope",
        }
    }
}

/// 模型目录 Provider 抽象（UI/Domain 不依赖具体 Provider）。
pub trait CatalogProvider {
    fn provider_id(&self) -> CatalogProviderId;

    /// 分页搜索模型（只请求 Summary 所需字段，不逐模型拉详情）。
    fn search(&self, query: &CatalogQuery)
    -> Result<CatalogPage<RemoteModelSummary>, CatalogError>;

    /// 模型详情（repo 元数据；**不**包含完整文件树）。
    fn model_detail(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<RemoteModelDetail, CatalogError>;

    /// 模型文件树（懒加载）。
    fn model_files(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Vec<RemoteModelFile>, CatalogError>;

    /// 模型 README（markdown 原文，懒加载）。
    fn model_readme(
        &self,
        repo_id: &str,
        revision: Option<&str>,
    ) -> Result<Option<String>, CatalogError>;
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// 模型分类（UI Tab：全部/LLM/ASR/TTS/KWS）。
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

/// 排序方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSort {
    /// 推荐（ZapMomo 兼容性 boost + 综合下载/点赞/更新）。
    #[default]
    Recommended,
    Downloads,
    Likes,
    LastModified,
    Trending,
}

/// 参数量范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterRange {
    Under1B,
    B1to3,
    B3to7,
    B7to14,
    Over14,
}

impl ParameterRange {
    /// HF `num_parameters=min:..,max:..` 的取值（best-effort，部分 repo 无该元数据）。
    pub fn min_max(&self) -> (Option<&'static str>, Option<&'static str>) {
        match self {
            ParameterRange::Under1B => (None, Some("1B")),
            ParameterRange::B1to3 => (Some("1B"), Some("3B")),
            ParameterRange::B3to7 => (Some("3B"), Some("7B")),
            ParameterRange::B7to14 => (Some("7B"), Some("14B")),
            ParameterRange::Over14 => (Some("14B"), None),
        }
    }
}

/// 目录查询（任何 filter 改变 → reset pagination → fetch page 1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogQuery {
    /// None = 全部类型。
    #[serde(default)]
    pub category: Option<ModelCategory>,
    #[serde(default)]
    pub search: Option<String>,
    /// BCP-47 语言（如 "zh" / "en"），来自 HF `language:` tag。
    #[serde(default)]
    pub language: Option<String>,
    /// SPDX 许可证（如 "apache-2.0" / "mit"）。
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub parameters: Option<ParameterRange>,
    #[serde(default)]
    pub sort: CatalogSort,
    /// 0-based 页号。
    #[serde(default)]
    pub page: u32,
    /// 每页数量（建议 20～30，HF limit 上限 100）。
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// 是否包含 Unsupported（默认隐藏）。
    #[serde(default)]
    pub include_unsupported: bool,
}

fn default_page_size() -> u32 {
    20
}

impl Default for CatalogQuery {
    fn default() -> Self {
        Self {
            category: None,
            search: None,
            language: None,
            license: None,
            parameters: None,
            sort: CatalogSort::Recommended,
            page: 0,
            page_size: default_page_size(),
            include_unsupported: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Remote 数据（Hugging Face 原始数据映射，字段可空、不伪造）
// ---------------------------------------------------------------------------

/// HF 列表页一条模型（Summary 字段）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelSummary {
    pub repo_id: String,
    #[serde(default)]
    pub author: String,
    /// 展示名（repo_id 去掉作者前缀；拿不到则不伪造，用 repo_id）。
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub library_name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub trending_score: Option<f64>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// 来自 `license:` 前缀 tag。
    #[serde(default)]
    pub license: Option<String>,
    /// 来自 `language:` 前缀 tag。
    #[serde(default)]
    pub languages: Vec<String>,
    /// 来自 `params:` 前缀 tag 或 `num_parameters`。
    #[serde(default)]
    pub parameter_count: Option<String>,
    /// gated 原始值（bool 或 "auto" 字符串）。
    #[serde(default)]
    pub gated: Option<String>,
    #[serde(default)]
    pub private: Option<bool>,
    /// revision / commit sha。
    #[serde(default)]
    pub sha: Option<String>,
}

/// 模型详情（repo 元数据；不含完整文件树，文件走 [`CatalogProvider::model_files`]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelDetail {
    pub repo_id: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub library_name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// revision / commit sha。
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub gated: Option<String>,
    #[serde(default)]
    pub private: Option<bool>,
    /// cardData（config 等原始数据，序列化为 Value，不在此强类型化）。
    #[serde(default)]
    pub card_data: Option<serde_json::Value>,
    /// 基本文件引用（rfilename；**不含大小**，大小需 files API）。
    #[serde(default)]
    pub siblings: Vec<String>,
}

/// LFS 元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLfs {
    pub sha256: String,
    pub size: u64,
}

/// 文件树条目（懒加载）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelFile {
    pub path: String,
    /// 目录时为 None（tree 展开）。
    #[serde(default)]
    pub size: Option<u64>,
    /// "file" | "directory"。
    #[serde(rename = "type")]
    pub file_type: String,
    #[serde(default)]
    pub lfs: Option<FileLfs>,
    /// 非 LFS 小文件的 sha256（HF 提供时）。
    #[serde(default)]
    pub sha256: Option<String>,
}

// ---------------------------------------------------------------------------
// 兼容性
// ---------------------------------------------------------------------------

/// ZapMomo 兼容性（四级，无第五级）。
///
/// - `Verified`：官方 Registry 明确验证过（overlay 命中），无需在线扫描。
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

    /// 是否允许一键安装（Verified / Compatible）。
    pub fn is_installable(&self) -> bool {
        matches!(
            self,
            CompatibilityLevel::Verified | CompatibilityLevel::Compatible
        )
    }
}

// ---------------------------------------------------------------------------
// Canonical 身份
// ---------------------------------------------------------------------------

/// 统一去重身份（跨 Verified Registry / HF / Local）。
///
/// - `huggingface:<normalized repoId>`
/// - `builtin:<registry_id>`
/// - `local:<stable local identity>`
///
/// 同一 `CanonicalModelKey` 只允许一个 `UnifiedModelItem`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CanonicalModelKey(String);

impl CanonicalModelKey {
    pub fn huggingface(repo_id: &str) -> Self {
        Self(format!("huggingface:{}", normalize_repo_id(repo_id)))
    }

    pub fn builtin(registry_id: &str) -> Self {
        Self(format!("builtin:{registry_id}"))
    }

    pub fn local(local_id: &str) -> Self {
        Self(format!("local:{local_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanonicalModelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// repoId 归一化（仅用于去重 key，API/展示仍用原始 repo_id）。
/// 大小写不敏感（与 `paths_equal` 在 Windows 上的语义一致），去首尾空白。
pub fn normalize_repo_id(repo_id: &str) -> String {
    repo_id.trim().to_lowercase()
}

// ---------------------------------------------------------------------------
// 本地状态聚合
// ---------------------------------------------------------------------------

/// 模型级本地状态聚合（仅 UI summary；Source of Truth 在 artifact/install 级）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelSummary {
    #[serde(default)]
    pub installed_artifact_count: usize,
    #[serde(default)]
    pub has_current_artifact: bool,
    #[serde(default)]
    pub active_download_count: usize,
}

// ---------------------------------------------------------------------------
// Unified 条目（merge 后结果）
// ---------------------------------------------------------------------------

/// 内置精选的展示信息（来自 registry，**非 HF 数据**；remote 为 None 时使用）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinModelSummary {
    pub display_name: String,
    pub description: String,
    /// llm/asr/tts/kws。
    pub model_type: String,
    #[serde(default)]
    pub runtime: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub parameter_count: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// 模型库卡片最终展示条目 = Remote + Compatibility + Local 三源合并。
///
/// merge 优先级：Identity → Remote → Verified overlay 增强 → Compatibility →
/// Local installs → local summary。同一 `canonical_key` 只出现一次。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedModelItem {
    pub canonical_key: String,
    /// 稳定 model id（HF repo_id / registry id / local id）。
    pub model_id: String,
    pub provider: String,
    /// 是否有远程元数据（HF 动态结果 / verified 内建）。
    #[serde(default)]
    pub remote: Option<RemoteModelSummary>,
    /// 内置精选展示信息（remote 为 None 时前端用此渲染）。
    #[serde(default)]
    pub builtin: Option<BuiltinModelSummary>,
    /// llm/asr/tts/kws（前端分类用）。
    #[serde(default)]
    pub model_type: Option<String>,
    pub compatibility: CompatibilityLevel,
    /// 兼容性说明 / 备注（overlay 提供）。
    #[serde(default)]
    pub compatibility_notes: Option<String>,
    /// 推荐 variant（GGUF 量化名等，无法可靠判断则为 None）。
    #[serde(default)]
    pub recommended_variant: Option<String>,
    /// 已安装/下载中的具体安装实例（artifact 级）。
    #[serde(default)]
    pub installs: Vec<LocalInstallView>,
    #[serde(default)]
    pub local_summary: LocalModelSummary,
    /// 是否已确认兼容（files 检查后缓存；用于列表"已确认兼容" Badge）。
    #[serde(default)]
    pub confirmed: bool,
}

/// 列表级本地安装视图（由 `install.rs` 的 `LocalModelInstall` 派生）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInstallView {
    pub install_id: String,
    pub artifact_id: String,
    #[serde(default)]
    pub variant: Option<String>,
    /// "installed" | "installing" | "error"（install_state 聚合）。
    pub state: String,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub local_path: Option<String>,
}

// ---------------------------------------------------------------------------
// 分页
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage<T> {
    pub items: Vec<T>,
    /// 是否还有下一页（只由远程分页决定，与本地 overlay 数量无关）。
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 目录/下载错误（区分 gated / 需要 token / 无权限 / 不存在 / 限流）。
#[derive(Debug, Clone, thiserror::Error)]
pub enum CatalogError {
    #[error("网络错误：{0}")]
    Network(String),
    #[error("服务端错误（HTTP {status}）：{detail}")]
    Http { status: u16, detail: String },
    /// 401/403 且无有效 token → 需要 Hugging Face Token。
    #[error("需要 Hugging Face Token")]
    AuthRequired,
    /// gated 模型 → 需要接受模型协议。
    #[error("该模型需要接受使用协议")]
    GatedRequiresAgreement,
    /// 404 → 模型不存在（或 private 且无权限）。
    #[error("模型不存在或无权访问")]
    NotFound,
    #[error("请求过于频繁，请稍后重试")]
    RateLimited,
    #[error("响应解析失败：{0}")]
    Parse(String),
    #[error("下载源配置无效：{0}")]
    Endpoint(String),
}

// ---------------------------------------------------------------------------
// merge 工具（Phase 4 起使用）
// ---------------------------------------------------------------------------

/// 按 canonical key 合并的模型集合容器。
pub type UnifiedModelMap = BTreeMap<CanonicalModelKey, UnifiedModelItem>;

/// 内置精选（registry 17）→ UnifiedModelItem（经 verified overlay；遵守 category/search 过滤）。
pub fn curated_unified(
    query: &CatalogQuery,
    local_summary: &std::collections::HashMap<String, LocalModelSummary>,
) -> Vec<UnifiedModelItem> {
    use super::registry;
    use super::verified::VerifiedRegistry;

    let verified = VerifiedRegistry::builtin();
    let mut out = Vec::new();
    for reg in registry::all_models() {
        let entry = verified.entry_for_model(&reg.id);
        let repo_id = entry.and_then(|e| e.repo_id.as_deref());
        // category 过滤（Tab 点击进入 query state）
        if let Some(cat) = query.category
            && reg.model_type.as_str() != cat.as_str()
        {
            continue;
        }
        // search 过滤（搜索 Qwen 只注入匹配的 Verified；搜 Whisper 只注入匹配的 Whisper 模型）
        if let Some(q) = query.search.as_deref().filter(|s| !s.trim().is_empty()) {
            let hay = format!(
                "{} {} {} {}",
                reg.display_name,
                reg.description,
                reg.name,
                reg.tags.join(" ")
            )
            .to_lowercase();
            if !hay.contains(&q.to_lowercase()) {
                continue;
            }
        }
        let model_id = repo_id.unwrap_or(&reg.id).to_string();
        let key = if let Some(r) = repo_id {
            CanonicalModelKey::huggingface(r)
        } else {
            CanonicalModelKey::builtin(&reg.id)
        };
        out.push(UnifiedModelItem {
            canonical_key: key.as_str().into(),
            model_id: model_id.clone(),
            provider: if repo_id.is_some() {
                "huggingface"
            } else {
                "builtin"
            }
            .into(),
            remote: None,
            builtin: Some(BuiltinModelSummary {
                display_name: reg.display_name.clone(),
                description: reg.description.clone(),
                model_type: reg.model_type.as_str().into(),
                runtime: reg.runtime.clone(),
                format: reg.format.clone(),
                languages: reg.languages.clone(),
                tags: reg.tags.clone(),
                parameter_count: reg.parameter_count.clone(),
                size_bytes: reg.size_bytes,
            }),
            model_type: Some(reg.model_type.as_str().into()),
            compatibility: CompatibilityLevel::Verified,
            compatibility_notes: entry.and_then(|e| e.compatibility_notes.clone()),
            recommended_variant: entry.and_then(|e| e.recommended_variant.clone()),
            installs: Vec::new(),
            local_summary: local_summary.get(&model_id).cloned().unwrap_or_default(),
            confirmed: true,
        });
    }
    out
}

/// 合并 curated + HF 远程页 + 本地状态为 UnifiedModelItem（canonical key 去重）。
///
/// - 同一 `CanonicalModelKey` 只出现一次（Verified 只是增强，不生成第二张卡）。
/// - `has_more` 只由远程分页决定；Verified/local 一次性 overlay。
pub fn merge_catalog(
    remote: CatalogPage<RemoteModelSummary>,
    curated: Vec<UnifiedModelItem>,
    local_summary: &std::collections::HashMap<String, LocalModelSummary>,
    category: Option<ModelCategory>,
) -> CatalogPage<UnifiedModelItem> {
    let mut map: UnifiedModelMap = BTreeMap::new();
    for c in curated {
        map.insert(CanonicalModelKey(c.canonical_key.clone()), c);
    }
    for r in &remote.items {
        let key = CanonicalModelKey::huggingface(&r.repo_id);
        let mut item = UnifiedModelItem {
            canonical_key: key.as_str().into(),
            model_id: r.repo_id.clone(),
            provider: "huggingface".into(),
            remote: Some(r.clone()),
            builtin: None,
            model_type: None,
            // 列表阶段可运行预判（gguf/sherpa 信号；Unsupported 默认被前端隐藏）
            compatibility: super::compat::assess_summary(r, category),
            compatibility_notes: None,
            recommended_variant: None,
            installs: Vec::new(),
            local_summary: local_summary.get(&r.repo_id).cloned().unwrap_or_default(),
            confirmed: false,
        };
        if let Some(existing) = map.remove(&key)
            && existing.compatibility == CompatibilityLevel::Verified
        {
            item.compatibility = CompatibilityLevel::Verified;
            item.compatibility_notes = existing.compatibility_notes.clone();
            item.recommended_variant = existing.recommended_variant.clone();
            item.model_type = existing.model_type.clone();
            item.installs = existing.installs.clone();
            // 同一 map 同 key：local_summary 相同，取 curated 侧值（不重复累加）
            item.local_summary = existing.local_summary.clone();
        }
        map.insert(key, item);
    }
    let items: Vec<UnifiedModelItem> = map.into_values().collect();
    CatalogPage {
        items,
        has_more: remote.has_more,
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_key_forms() {
        assert_eq!(
            CanonicalModelKey::huggingface("Qwen/Qwen3-4B-GGUF").as_str(),
            "huggingface:qwen/qwen3-4b-gguf"
        );
        // 大小写/空白归一化后同 key（去重）
        assert_eq!(
            CanonicalModelKey::huggingface("qwen/Qwen3-4B-GGUF"),
            CanonicalModelKey::huggingface("  Qwen/Qwen3-4B-GGUF ")
        );
        assert_eq!(
            CanonicalModelKey::builtin("kws-zipformer").as_str(),
            "builtin:kws-zipformer"
        );
        assert_eq!(
            CanonicalModelKey::local("local-abc").as_str(),
            "local:local-abc"
        );
    }

    #[test]
    fn test_compatibility_installable() {
        assert!(CompatibilityLevel::Verified.is_installable());
        assert!(CompatibilityLevel::Compatible.is_installable());
        assert!(!CompatibilityLevel::Possible.is_installable());
        assert!(!CompatibilityLevel::Unsupported.is_installable());
    }

    #[test]
    fn test_catalog_query_default() {
        let q = CatalogQuery::default();
        assert!(q.category.is_none());
        assert_eq!(q.page_size, 20);
        assert_eq!(q.sort, CatalogSort::Recommended);
    }

    #[test]
    fn test_parameter_range_min_max() {
        assert_eq!(ParameterRange::Under1B.min_max(), (None, Some("1B")));
        assert_eq!(ParameterRange::B3to7.min_max(), (Some("3B"), Some("7B")));
        assert_eq!(ParameterRange::Over14.min_max(), (Some("14B"), None));
    }

    #[test]
    fn test_unified_model_serde_roundtrip() {
        let item = UnifiedModelItem {
            canonical_key: "huggingface:qwen/qwen3-4b-gguf".into(),
            model_id: "Qwen/Qwen3-4B-GGUF".into(),
            provider: "huggingface".into(),
            remote: Some(RemoteModelSummary {
                repo_id: "Qwen/Qwen3-4B-GGUF".into(),
                author: "Qwen".into(),
                display_name: "Qwen3-4B-GGUF".into(),
                description: None,
                pipeline_tag: Some("text-generation".into()),
                library_name: Some("gguf".into()),
                tags: vec![],
                downloads: 0,
                likes: 0,
                trending_score: None,
                last_modified: None,
                created_at: None,
                license: None,
                languages: vec![],
                parameter_count: None,
                gated: None,
                private: None,
                sha: None,
            }),
            builtin: None,
            model_type: None,
            compatibility: CompatibilityLevel::Possible,
            compatibility_notes: None,
            recommended_variant: None,
            installs: vec![],
            local_summary: LocalModelSummary::default(),
            confirmed: false,
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: UnifiedModelItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.canonical_key, item.canonical_key);
        assert_eq!(back.compatibility, CompatibilityLevel::Possible);
        assert_eq!(back.remote.as_ref().unwrap().repo_id, "Qwen/Qwen3-4B-GGUF");
    }

    #[test]
    fn test_merge_dedup_verified_plus_hf() {
        // curated 含 unsloth/Qwen3-4B-Instruct-2507-GGUF（Verified），HF 页也返回它 → 只留一个且 Verified
        let mut local = std::collections::HashMap::new();
        let mut s = LocalModelSummary::default();
        s.installed_artifact_count = 1;
        local.insert("unsloth/Qwen3-4B-Instruct-2507-GGUF".to_string(), s);

        let curated = curated_unified(&CatalogQuery::default(), &local);
        assert!(
            curated
                .iter()
                .any(|i| i.model_id == "unsloth/Qwen3-4B-Instruct-2507-GGUF"),
            "curated 应含 HF 映射的内置 LLM"
        );
        assert!(
            curated
                .iter()
                .any(|i| i.model_id == "kws-zipformer-zh-en-3m")
        );

        let remote = CatalogPage {
            items: vec![RemoteModelSummary {
                repo_id: "unsloth/Qwen3-4B-Instruct-2507-GGUF".into(),
                ..Default::default()
            }],
            has_more: false,
        };
        let merged = merge_catalog(remote, curated, &local, None);
        let qwen = merged
            .items
            .iter()
            .find(|i| i.model_id == "unsloth/Qwen3-4B-Instruct-2507-GGUF")
            .unwrap();
        assert_eq!(
            qwen.compatibility,
            CompatibilityLevel::Verified,
            "overlay 增强为 Verified"
        );
        assert_eq!(
            qwen.local_summary.installed_artifact_count, 1,
            "本地状态合并"
        );
        // 同 key 只出现一次
        assert_eq!(
            merged
                .items
                .iter()
                .filter(|i| i.model_id == "unsloth/Qwen3-4B-Instruct-2507-GGUF")
                .count(),
            1
        );
        assert!(!merged.has_more);
    }

    #[test]
    fn test_curated_respects_category_and_search() {
        let local = std::collections::HashMap::new();
        let q = CatalogQuery {
            category: Some(ModelCategory::Llm),
            ..Default::default()
        };
        let curated = curated_unified(&q, &local);
        assert!(
            curated
                .iter()
                .all(|i| i.model_type.as_deref() == Some("llm"))
        );
        assert!(curated.iter().all(|i| i.model_id.contains("unsloth")));
        // 搜 Whisper：只注入匹配的 Whisper 离线模型，不带出无关 KWS/LLM/TTS
        let q2 = CatalogQuery {
            search: Some("whisper".into()),
            ..Default::default()
        };
        let curated2 = curated_unified(&q2, &local);
        assert!(
            curated2.iter().all(|i| i.model_id.contains("whisper")),
            "搜 Whisper 应只注入 whisper 模型，实际: {:?}",
            curated2
                .iter()
                .map(|i| i.model_id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(curated2.len(), 2, "应为 2 个 whisper 模型");
        // 搜 Qwen：注入匹配的 Verified（Qwen 系列 + 描述提及 Qwen 的 llama 条目）与
        // Qwen3-ASR 离线模型，不注入无关 KWS
        let q3 = CatalogQuery {
            search: Some("qwen".into()),
            ..Default::default()
        };
        let curated3 = curated_unified(&q3, &local);
        assert!(
            curated3
                .iter()
                .any(|i| i.model_id == "unsloth/Qwen3-8B-GGUF"),
            "应注入 Qwen 系列 HF repo"
        );
        assert!(
            curated3.iter().any(|i| i.model_id == "asr-qwen3-0.6b"),
            "应注入 Qwen3-ASR 离线模型"
        );
        assert!(
            curated3
                .iter()
                .all(|i| i.model_type.as_deref() == Some("llm")
                    || i.model_id == "asr-qwen3-0.6b"
                    || i.model_id == "tts-omnivoice-q8-audiocpp"
                    || i.model_id == "tts-qwen3-06b-base-q8-audiocpp"
                    || i.model_id == "tts-qwen3-17b-base-q8-audiocpp"),
            "搜索 qwen 只注入 LLM 与 Qwen3-ASR（及描述/名称含 Qwen3 的 OmniVoice、Qwen3-TTS，仅 Metal 平台）"
        );
    }
}
