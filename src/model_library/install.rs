//! 本地安装存储（ModelStorage）与安装事实（LocalModelInstall）。
//!
//! Source of Truth：**installed inventory 只来源于模型目录 + `.zapmomo-lib.json` 扫描**，
//! Settings 不保存 installed inventory。`is_current` 不持久化，由 Settings current path 派生。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::settings::get_models_dir;

use super::catalog::ModelCategory;
use super::download::ArtifactSource;

/// 元数据 schema 版本（v2：新增 source/model_id/artifact_id/variant 等）。
pub const META_SCHEMA_VERSION: u32 = 2;

/// `.zapmomo-lib.json` 内容。
///
/// **不保存**：is_current / download state / 临时进度。
/// **Legacy 兼容**：schema_version=1 只有 `registry_id/version/installed_at/managed`，均 `#[serde(default)]`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMeta {
    pub schema_version: u32,
    /// 稳定 install_id（`hash(source|model_id|artifact_id|variant)`；revision 不参与）。
    #[serde(default)]
    pub install_id: String,
    /// "hf" | "registry" | "local"。
    #[serde(default)]
    pub source: String,
    /// 稳定 model_id（HF repo_id / registry id / local id）。
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub repo_id: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    /// llm / asr / tts / kws。
    #[serde(default)]
    pub model_type: String,
    #[serde(default)]
    pub artifact_id: String,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub installed_at: String,
    // ---- legacy（schema_version=1）----
    #[serde(default)]
    pub registry_id: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub managed: Option<bool>,
}

impl InstallMeta {
    pub fn legacy(registry_id: &str, installed_at: &str) -> Self {
        Self {
            schema_version: 1,
            install_id: String::new(),
            source: "registry".into(),
            model_id: registry_id.into(),
            repo_id: None,
            revision: None,
            model_type: String::new(),
            artifact_id: String::new(),
            variant: None,
            architecture: None,
            installed_at: installed_at.into(),
            registry_id: Some(registry_id.into()),
            version: None,
            managed: Some(true),
        }
    }

    /// artifact_source 字符串 ↔ 枚举。
    pub fn source_str(source: &ArtifactSource) -> &'static str {
        match source {
            ArtifactSource::RegistryManifest => "registry",
            ArtifactSource::HuggingFace => "hf",
            ArtifactSource::LocalImport => "local",
        }
    }
}

/// 本地安装事实（**安装信息**；运行状态由派生，不持久化）。
#[derive(Debug, Clone)]
pub struct LocalModelInstall {
    pub install_id: String,
    pub model_id: String,
    pub artifact_id: String,
    pub variant: Option<String>,
    pub model_type: ModelCategory,
    pub artifact_source: ArtifactSource,
    /// 安装目录（文件集合所在目录）。
    pub install_dir: PathBuf,
    /// runtime 实际入口（LLM=gguf 文件 / sherpa=目录）；由 `resolve_runtime_path` 提供。
    pub runtime_path: PathBuf,
    pub repo_id: Option<String>,
    pub revision: Option<String>,
    pub installed_at: String,
    pub architecture: Option<String>,
}

// ---------------------------------------------------------------------------
// ModelStorage
// ---------------------------------------------------------------------------

/// 本地模型存储：路径统一抽象（避免散落字符串 path）。
pub struct ModelStorage;

impl ModelStorage {
    /// 根目录：`~/.zapmomo/models`。
    pub fn root() -> PathBuf {
        get_models_dir()
    }

    /// 分类子目录：`~/.zapmomo/models/<llm|asr|tts|kws>`。
    pub fn category_dir(cat: ModelCategory) -> PathBuf {
        ModelStorage::root().join(cat.as_str())
    }

    /// storageKey：`<provider>--<sanitized model_id>`（安全、稳定、跨平台）。
    pub fn storage_key(provider: &str, model_id: &str) -> String {
        let body = model_id.replace('/', "__");
        let mut sanitized: String = String::with_capacity(body.len());
        let mut last_dash = false;
        for c in body.chars() {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                sanitized.push(c);
                last_dash = false;
            } else if !last_dash {
                sanitized.push('-');
                last_dash = true;
            }
        }
        let sanitized = sanitized.trim_matches('-').to_string();
        let key = format!("{provider}--{sanitized}");
        if key.len() > 120 {
            let h = crate::model_library::install::short_hash(model_id);
            format!("{}-{h}", &key[..60])
        } else {
            key
        }
    }

    /// artifact 安装目录：`~/.zapmomo/models/<category>/<storageKey>/<artifact_id>`。
    pub fn install_dir(
        provider: &str,
        model_id: &str,
        cat: ModelCategory,
        artifact_id: &str,
    ) -> PathBuf {
        ModelStorage::category_dir(cat)
            .join(ModelStorage::storage_key(provider, model_id))
            .join(safe_artifact_id(artifact_id))
    }

    pub fn meta_path(dir: &Path) -> PathBuf {
        dir.join(".zapmomo-lib.json")
    }

    /// 读取安装元数据（缺失/损坏返回 None）。
    pub fn read_meta(dir: &Path) -> Option<InstallMeta> {
        let content = std::fs::read_to_string(ModelStorage::meta_path(dir)).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// 写安装元数据（幂等）。
    pub fn write_meta(dir: &Path, meta: &InstallMeta) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建安装目录失败：{e}"))?;
        let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        let tmp = ModelStorage::meta_path(dir).with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写入元数据失败：{e}"))?;
        std::fs::rename(&tmp, ModelStorage::meta_path(dir)).map_err(|e| e.to_string())
    }

    /// 扫描根集合：主根（安装目标，永远新根）+ 旧默认根（自定义 `data_dir` 后的存量位置）。
    ///
    /// 顺序即优先级：去重时保留先出现者。
    pub fn roots() -> Vec<PathBuf> {
        let mut roots = vec![ModelStorage::root()];
        if let Some(legacy) = crate::config::settings::legacy_models_dir()
            && !roots.contains(&legacy)
        {
            roots.push(legacy);
        }
        roots
    }

    /// 扫描所有已安装模型（`.zapmomo-lib.json`）。
    /// 返回 (install_dir, meta)。这是 installed inventory 的唯一事实来源。
    ///
    /// 支持两种布局：
    /// - HF 布局：`<category>/<storageKey>/<artifact_id>/.zapmomo-lib.json`
    /// - legacy 布局：`<reg.name>/.zapmomo-lib.json`（内置模型，无分类子目录）
    ///
    /// 自定义 `data_dir` 后扫描双根，按 install_id 去重（新根优先）；
    /// v1 legacy meta 的 install_id 为空时用规范化目录路径作 key。
    pub fn scan_installs() -> Vec<(PathBuf, InstallMeta)> {
        let mut out = Vec::new();
        fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, InstallMeta)>) {
            if depth > 3 {
                return;
            }
            if let Some(meta) = ModelStorage::read_meta(dir) {
                out.push((dir.to_path_buf(), meta));
                return; // 有 meta 的目录不再下钻
            }
            if !dir.is_dir() {
                return;
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == ".install" {
                    continue; // staging 目录不视为已安装
                }
                if p.is_dir() {
                    walk(&p, depth + 1, out);
                }
            }
        }
        for root in ModelStorage::roots() {
            walk(&root, 0, &mut out);
        }
        // 双根去重：同 install_id 只保留先出现者（roots 顺序 = 新根优先）
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        out.retain(|(dir, meta)| seen.insert(install_scan_key(dir, meta)));
        out
    }
}

/// 扫描去重 key：优先 install_id；v1 legacy meta（install_id 为空）退化用目录路径。
///
/// Windows 路径大小写不敏感，统一小写比较，避免同一路径两种大小写被当作两份。
fn install_scan_key(dir: &Path, meta: &InstallMeta) -> String {
    if !meta.install_id.is_empty() {
        return meta.install_id.clone();
    }
    let s = dir.to_string_lossy();
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

/// artifact_id 用于目录名时清洗（只保留安全字符）。
fn safe_artifact_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "model".to_string()
    } else {
        out
    }
}

/// 稳定短 hash（install_id / storage_key 超长兜底）。
pub(crate) fn short_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(input.as_bytes());
    hex::encode(d)[..12].to_string()
}

/// 派生稳定 install_id（revision 不参与）。
pub fn derive_install_id(
    source: &ArtifactSource,
    model_id: &str,
    artifact_id: &str,
    variant: Option<&str>,
) -> String {
    let key = format!(
        "{}|{model_id}|{artifact_id}|{}",
        InstallMeta::source_str(source),
        variant.unwrap_or("")
    );
    format!("install-{}", short_hash(&key))
}

/// 从文件集推导 runtime_path：
/// - LLM 单文件 → 该 gguf 文件；split → 第一 shard（llama-cpp-2 真实加载行为 Phase 4 复核）。
/// - sherpa 文件组 → 安装目录本身。
pub fn resolve_runtime_path(
    install_dir: &Path,
    model_type: ModelCategory,
    files: &[String],
) -> PathBuf {
    if model_type == ModelCategory::Llm
        && let Some(first) = files.first()
    {
        return install_dir.join(first);
    }
    install_dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    #[test]
    fn test_storage_key_sanitized() {
        assert_eq!(
            ModelStorage::storage_key("hf", "Qwen/Qwen3-4B-GGUF"),
            "hf--Qwen__Qwen3-4B-GGUF"
        );
        assert_eq!(ModelStorage::storage_key("hf", "a:b c"), "hf--a-b-c");
        assert!(ModelStorage::storage_key("hf", "x").starts_with("hf--x"));
    }

    #[test]
    fn test_install_dir_layout() {
        run_with_temp_home(|_| {
            let dir = ModelStorage::install_dir(
                "hf",
                "Qwen/Qwen3-4B-GGUF",
                ModelCategory::Llm,
                "Qwen3-4B-Q4_K_M",
            );
            assert!(dir.ends_with("models/llm/hf--Qwen__Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M"));
        });
    }

    #[test]
    fn test_meta_roundtrip_and_scan() {
        run_with_temp_home(|_| {
            let dir = ModelStorage::install_dir(
                "hf",
                "Qwen/Qwen3-4B-GGUF",
                ModelCategory::Llm,
                "Qwen3-4B-Q4_K_M",
            );
            let meta = InstallMeta {
                schema_version: META_SCHEMA_VERSION,
                install_id: "install-abcdef".into(),
                source: "hf".into(),
                model_id: "Qwen/Qwen3-4B-GGUF".into(),
                repo_id: Some("Qwen/Qwen3-4B-GGUF".into()),
                revision: Some("main".into()),
                model_type: "llm".into(),
                artifact_id: "Qwen3-4B-Q4_K_M".into(),
                variant: Some("Q4_K_M".into()),
                architecture: Some("llama-cpp-gguf".into()),
                installed_at: "2026-08-17T00:00:00Z".into(),
                registry_id: None,
                version: None,
                managed: None,
            };
            ModelStorage::write_meta(&dir, &meta).unwrap();
            let scanned = ModelStorage::scan_installs();
            assert_eq!(scanned.len(), 1);
            assert_eq!(scanned[0].0, dir);
            assert_eq!(scanned[0].1.model_id, "Qwen/Qwen3-4B-GGUF");
            assert_eq!(scanned[0].1.variant.as_deref(), Some("Q4_K_M"));
        });
    }

    #[test]
    fn test_legacy_meta_reads() {
        run_with_temp_home(|_| {
            let dir = ModelStorage::install_dir(
                "registry",
                "qwen3-1.7b-q4-k-m",
                ModelCategory::Llm,
                "qwen3-1.7b-q4-k-m",
            );
            let meta = InstallMeta::legacy("qwen3-1.7b-q4-k-m", "2026-01-01T00:00:00Z");
            ModelStorage::write_meta(&dir, &meta).unwrap();
            let m = ModelStorage::read_meta(&dir).unwrap();
            assert_eq!(m.schema_version, 1);
            assert_eq!(m.registry_id.as_deref(), Some("qwen3-1.7b-q4-k-m"));
        });
    }

    #[test]
    fn test_derive_install_id_stable_and_revision_free() {
        let source = ArtifactSource::HuggingFace;
        let a = derive_install_id(&source, "Qwen/Qwen3-4B-GGUF", "a1", Some("Q4_K_M"));
        let b = derive_install_id(&source, "Qwen/Qwen3-4B-GGUF", "a1", Some("Q4_K_M"));
        assert_eq!(a, b);
        // revision 不参与 install_id
        let c = derive_install_id(&source, "Qwen/Qwen3-4B-GGUF", "a1", Some("Q4_K_M"));
        assert_eq!(a, c);
        // 不同 variant 不同 install_id
        let d = derive_install_id(&source, "Qwen/Qwen3-4B-GGUF", "a1", Some("Q5_K_M"));
        assert_ne!(a, d);
        assert!(a.starts_with("install-"));
    }

    #[test]
    fn test_runtime_path_llm_file_vs_sherpa_dir() {
        let dir = Path::new("/x/models/llm/m");
        assert_eq!(
            resolve_runtime_path(dir, ModelCategory::Llm, &["model-Q4_K_M.gguf".into()]),
            dir.join("model-Q4_K_M.gguf")
        );
        assert_eq!(resolve_runtime_path(dir, ModelCategory::Asr, &[]), dir);
    }
}
