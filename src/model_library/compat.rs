//! ZapMomo 兼容性判断（两阶段，四级）+ Artifact Builder。
//!
//! - `ArchitectureDetector`：从真实文件集识别架构（源自 `asr/tts/kws` config 的
//!   required files，**不按 model_type 硬编码**）。
//! - `CompatibilityResolver`：Stage1(summary)→Verified/Possible；Stage2(files)→Compatible/Unsupported。
//! - `ModelArtifact`：唯一下载入口（LLM=1..N gguf 文件；sherpa=文件组）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::catalog::{CompatibilityLevel, ModelCategory, RemoteModelFile, RemoteModelSummary};
use super::gguf;
use super::verified::VerifiedRegistry;

// ---------------------------------------------------------------------------
// ModelArtifact
// ---------------------------------------------------------------------------

/// 一个可下载的安装单元（LLM 可能是单个或 split 的多个 gguf；sherpa 是一组文件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelArtifact {
    /// 稳定 id（repo 内唯一）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// "llama.cpp" | "sherpa-onnx"。
    pub runtime: String,
    /// "GGUF" | "ONNX"。
    pub format: String,
    /// LLM 的量化（如 "Q4_K_M"）；sherpa 为 None。
    #[serde(default)]
    pub variant: Option<String>,
    pub files: Vec<RemoteModelFile>,
    #[serde(default)]
    pub total_size: Option<u64>,
    /// split GGUF 缺 shard 时不可一键安装。
    #[serde(default)]
    pub installable: bool,
}

// ---------------------------------------------------------------------------
// 架构检测（源自真实 Runtime 需求）
// ---------------------------------------------------------------------------

/// 文件匹配规格：contains / equals / ends_with 任一命中即算满足。
struct FileSpec {
    contains: &'static [&'static str],
    equals: &'static [&'static str],
    ends_with: &'static [&'static str],
}

fn spec_matches(spec: &FileSpec, path: &str) -> bool {
    let lower = path.to_lowercase();
    spec.contains
        .iter()
        .any(|c| lower.contains(&c.to_lowercase()))
        || spec.equals.iter().any(|e| lower == e.to_lowercase())
        || spec
            .ends_with
            .iter()
            .any(|e| lower.ends_with(&e.to_lowercase()))
}

/// 一个 runtime 架构的兼容性规格。
struct CompatibilitySpec {
    architecture: &'static str,
    runtime: &'static str,
    format: &'static str,
    model_type: ModelCategory,
    display_name: &'static str,
    /// 全部满足才算 Compatible。
    required: &'static [&'static FileSpec],
    /// 含这些标记文件即整个规格不参与（防其它族目录误判，如 VITS 的 lexicon 污染 SenseVoice）。
    absent: &'static [&'static str],
}

// 参考 src/kws/config.rs KWS_REQUIRED_FILES（encoder/decoder/joiner/tokens + keywords.txt）
// 文件名子串本身足够区分 sherpa 组件，避免 `ends_with .onnx` 误配任意 onnx。
static SHERPA_ENCODER: FileSpec = FileSpec {
    contains: &["encoder"],
    equals: &[],
    ends_with: &[],
};
static SHERPA_DECODER: FileSpec = FileSpec {
    contains: &["decoder"],
    equals: &[],
    ends_with: &[],
};
static SHERPA_JOINER: FileSpec = FileSpec {
    contains: &["joiner"],
    equals: &[],
    ends_with: &[],
};
static SHERPA_TOKENS: FileSpec = FileSpec {
    contains: &[],
    equals: &["tokens.txt"],
    ends_with: &[],
};
static SHERPA_KEYWORDS: FileSpec = FileSpec {
    contains: &["keywords"],
    equals: &[],
    ends_with: &[],
};
// 参考 src/tts/config.rs（vocoder / lexicon）
static SHERPA_VOCODER: FileSpec = FileSpec {
    contains: &["vocoder", "vocos"],
    equals: &[],
    ends_with: &[],
};
static SHERPA_LEXICON: FileSpec = FileSpec {
    contains: &["lexicon"],
    equals: &[],
    ends_with: &[],
};

static KWS_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-kws-streaming-zipformer",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Kws,
    display_name: "唤醒词模型（KWS）",
    required: &[
        &SHERPA_ENCODER,
        &SHERPA_DECODER,
        &SHERPA_JOINER,
        &SHERPA_TOKENS,
        &SHERPA_KEYWORDS,
    ],
    absent: &[],
};
static ASR_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-asr-streaming-zipformer",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Asr,
    display_name: "流式语音识别（ASR）",
    required: &[
        &SHERPA_ENCODER,
        &SHERPA_DECODER,
        &SHERPA_JOINER,
        &SHERPA_TOKENS,
    ],
    absent: &[],
};
static TTS_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-tts-zipvoice",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Tts,
    display_name: "语音合成（TTS）",
    required: &[
        &SHERPA_ENCODER,
        &SHERPA_DECODER,
        &SHERPA_VOCODER,
        &SHERPA_TOKENS,
        &SHERPA_LEXICON,
    ],
    absent: &[],
};
// 离线 ASR：SenseVoice（model.onnx/int8 + tokens，lexicon 排除 VITS 目录）与
// Whisper（<size>-encoder/decoder.onnx + <size>-tokens.txt，无 joiner）
static SHERPA_SENSEVOICE_MODEL: FileSpec = FileSpec {
    contains: &[],
    equals: &["model.onnx", "model.int8.onnx"],
    ends_with: &[],
};
static SHERPA_WHISPER_ENCODER: FileSpec = FileSpec {
    contains: &[],
    equals: &[],
    ends_with: &["-encoder.onnx"],
};
static SHERPA_WHISPER_DECODER: FileSpec = FileSpec {
    contains: &[],
    equals: &[],
    ends_with: &["-decoder.onnx"],
};
static SHERPA_WHISPER_TOKENS: FileSpec = FileSpec {
    contains: &[],
    equals: &[],
    ends_with: &["-tokens.txt"],
};
static SENSEVOICE_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-asr-offline-sensevoice",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Asr,
    display_name: "离线识别（SenseVoice）",
    required: &[&SHERPA_SENSEVOICE_MODEL, &SHERPA_TOKENS],
    absent: &["lexicon"],
};
static WHISPER_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-asr-offline-whisper",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Asr,
    display_name: "离线识别（Whisper）",
    required: &[
        &SHERPA_WHISPER_ENCODER,
        &SHERPA_WHISPER_DECODER,
        &SHERPA_WHISPER_TOKENS,
    ],
    absent: &[],
};
// 流式 Paraformer：裸名 encoder/decoder（int8|fp32）+ tokens，无 joiner。
// `equals` 精确名与 zipformer 的 `contains`+joiner 必需、whisper 的 `-encoder.onnx`
// 连字符后缀互斥；paraformer 目录在 zipformer ASR_SPEC 下只有 3/4（缺 joiner）。
static SHERPA_PARAFORMER_ENCODER: FileSpec = FileSpec {
    contains: &[],
    equals: &["encoder.onnx", "encoder.int8.onnx"],
    ends_with: &[],
};
static SHERPA_PARAFORMER_DECODER: FileSpec = FileSpec {
    contains: &[],
    equals: &["decoder.onnx", "decoder.int8.onnx"],
    ends_with: &[],
};
static PARAFORMER_SPEC: CompatibilitySpec = CompatibilitySpec {
    architecture: "sherpa-asr-streaming-paraformer",
    runtime: "sherpa-onnx",
    format: "ONNX",
    model_type: ModelCategory::Asr,
    display_name: "流式识别（Paraformer）",
    required: &[
        &SHERPA_PARAFORMER_ENCODER,
        &SHERPA_PARAFORMER_DECODER,
        &SHERPA_TOKENS,
    ],
    absent: &[],
};

const SHERPA_SPECS: [&CompatibilitySpec; 6] = [
    &KWS_SPEC,
    &ASR_SPEC,
    &TTS_SPEC,
    &SENSEVOICE_SPEC,
    &WHISPER_SPEC,
    &PARAFORMER_SPEC,
];

// ---------------------------------------------------------------------------
// Compatibility 结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub level: CompatibilityLevel,
    pub reason: String,
    #[serde(default)]
    pub model_type: Option<ModelCategory>,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ModelArtifact>,
    /// 推荐 variant（overlay 或业务层计算；无法可靠判断为 None）。
    #[serde(default)]
    pub recommended_variant: Option<String>,
}

// ---------------------------------------------------------------------------
// Artifact Builder
// ---------------------------------------------------------------------------

/// 按 (base_stem, quantization) 分组 gguf 文件并校验 split 完整性。
fn build_gguf_artifacts(files: &[RemoteModelFile]) -> Vec<ModelArtifact> {
    let gguf_files: Vec<&RemoteModelFile> = files
        .iter()
        .filter(|f| f.file_type == "file" && gguf::is_gguf_filename(&f.path))
        .collect();
    let mut groups: BTreeMap<(String, Option<String>), Vec<&RemoteModelFile>> = BTreeMap::new();
    for f in &gguf_files {
        if let Some(id) = gguf::parse_gguf_identity(&f.path) {
            groups
                .entry((id.base_stem.clone(), id.quantization.clone()))
                .or_default()
                .push(f);
        }
    }
    groups
        .into_iter()
        .map(|((base, quant), files)| {
            let ids: Vec<gguf::GgufFileIdentity> = files
                .iter()
                .filter_map(|f| gguf::parse_gguf_identity(&f.path))
                .collect();
            // split 完整性：任一文件有 shard_total 时，必须 1..=total 齐全
            let shard_total = ids.iter().filter_map(|i| i.shard_total).max();
            let complete = match shard_total {
                Some(total) => {
                    let present: BTreeSet<usize> =
                        ids.iter().filter_map(|i| i.shard_index).collect();
                    (1..=total).all(|i| present.contains(&i))
                }
                None => true,
            };
            let total_size = files.iter().filter_map(|f| f.size).sum::<u64>();
            let variant = quant.clone();
            let id = match &quant {
                Some(q) => format!("{base}-{q}"),
                None => format!("{base}-model"),
            };
            ModelArtifact {
                id,
                name: base.clone(),
                runtime: "llama.cpp".into(),
                format: "GGUF".into(),
                variant,
                files: files.into_iter().cloned().collect(),
                total_size: (total_size > 0).then_some(total_size),
                installable: complete,
            }
        })
        .collect()
}

/// 构建 sherpa 文件组 artifact（required 全部满足才调用）。
fn build_sherpa_artifact(spec: &CompatibilitySpec, files: &[RemoteModelFile]) -> ModelArtifact {
    let required: Vec<RemoteModelFile> = files
        .iter()
        .filter(|f| {
            spec.required
                .iter()
                .copied()
                .any(|s| spec_matches(s, &f.path))
        })
        .cloned()
        .collect();
    let total_size = required.iter().filter_map(|f| f.size).sum::<u64>();
    ModelArtifact {
        id: spec.architecture.to_string(),
        name: spec.display_name.to_string(),
        runtime: spec.runtime.to_string(),
        format: spec.format.to_string(),
        variant: None,
        files: required,
        total_size: (total_size > 0).then_some(total_size),
        installable: true,
    }
}

// ---------------------------------------------------------------------------
// Stage 1 预判（列表阶段，仅凭 summary，不查文件）
// ---------------------------------------------------------------------------

/// 列表阶段可运行预判（避免 N+1）。
///
/// - overlay 命中 repo → `Verified`
/// - LLM（text-generation）：tags 含 `"gguf"`（HF 对 GGUF 仓库自动打 tag）→ `Compatible`；否则 → `Unsupported`
/// - ASR/TTS/KWS（speech pipeline）：tags 含 `"sherpa"` → `Possible`（需查文件确认文件组）；否则 → `Unsupported`
/// - 其他 pipeline（图像/embedding 等）→ `Unsupported`
pub fn assess_summary(
    summary: &RemoteModelSummary,
    category: Option<ModelCategory>,
) -> CompatibilityLevel {
    let verified = VerifiedRegistry::builtin();
    if verified.is_verified_repo(&summary.repo_id) {
        return CompatibilityLevel::Verified;
    }
    let effective = category.or_else(|| infer_category_from_pipeline(&summary.pipeline_tag));
    match effective {
        Some(ModelCategory::Llm) => {
            if has_tag(&summary.tags, "gguf") {
                CompatibilityLevel::Compatible
            } else {
                CompatibilityLevel::Unsupported
            }
        }
        Some(ModelCategory::Asr | ModelCategory::Tts | ModelCategory::Kws) => {
            if has_tag(&summary.tags, "sherpa") {
                CompatibilityLevel::Possible
            } else {
                CompatibilityLevel::Unsupported
            }
        }
        None => CompatibilityLevel::Unsupported,
    }
}

/// pipeline_tag → 有效分类（"全部"Tab 时 category 为 None 用此推断）。
fn infer_category_from_pipeline(pipeline: &Option<String>) -> Option<ModelCategory> {
    match pipeline.as_deref() {
        Some("text-generation") => Some(ModelCategory::Llm),
        Some("automatic-speech-recognition") => Some(ModelCategory::Asr),
        Some("text-to-speech") => Some(ModelCategory::Tts),
        _ => None,
    }
}

fn has_tag(tags: &[String], needle: &str) -> bool {
    let needle = needle.to_lowercase();
    tags.iter().any(|t| t.to_lowercase().contains(&needle))
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

/// 两阶段兼容性解析器。
pub struct CompatibilityResolver {
    verified: &'static VerifiedRegistry,
}

impl CompatibilityResolver {
    pub fn new() -> Self {
        Self {
            verified: VerifiedRegistry::builtin(),
        }
    }

    /// Stage 2：files 加载后完整判定。
    pub fn from_files(&self, repo_id: &str, files: &[RemoteModelFile]) -> Compatibility {
        let verified_entry = self.verified.entry_for_repo(repo_id);
        if let Some(entry) = verified_entry {
            if let Some(mut c) = self.assess(files) {
                // 已验证模型：级别提升为 Verified，并注入 overlay 的推荐 variant。
                c.level = CompatibilityLevel::Verified;
                if c.recommended_variant.is_none() {
                    c.recommended_variant = entry.recommended_variant.clone();
                }
                c.reason = format!("ZapMomo 已验证。{}", c.reason);
                return c;
            }
            // 已验证但文件未匹配：保持 Verified（安装走内置 registry），不给伪造 artifacts。
            return Compatibility {
                level: CompatibilityLevel::Verified,
                reason: "ZapMomo 已验证（安装走内置 registry），当前 repo 未检出可安装文件集"
                    .into(),
                model_type: entry
                    .model_type
                    .as_deref()
                    .and_then(ModelCategory::from_str_value),
                architecture: entry.architecture.clone(),
                artifacts: Vec::new(),
                recommended_variant: entry.recommended_variant.clone(),
            };
        }
        self.assess(files).unwrap_or_else(|| Compatibility {
            level: CompatibilityLevel::Unsupported,
            reason: "无法匹配 ZapMomo 支持的任何模型架构（需要 GGUF，或 sherpa-onnx 所需文件组）"
                .into(),
            model_type: None,
            architecture: None,
            artifacts: Vec::new(),
            recommended_variant: None,
        })
    }

    /// 文件集评估：GGUF → LLM Compatible；sherpa 规格全满足 → Compatible；部分 → Possible；否则 None。
    fn assess(&self, files: &[RemoteModelFile]) -> Option<Compatibility> {
        let has_gguf = files
            .iter()
            .any(|f| f.file_type == "file" && gguf::is_gguf_filename(&f.path));
        if has_gguf {
            let artifacts = build_gguf_artifacts(files);
            if artifacts.is_empty() {
                return None;
            }
            let recommended = {
                let quants: Vec<String> = artifacts
                    .iter()
                    .filter(|a| a.installable)
                    .filter_map(|a| a.variant.clone())
                    .collect();
                gguf::recommend_quantization(&quants).map(|i| quants[i].clone())
            };
            return Some(Compatibility {
                level: CompatibilityLevel::Compatible,
                reason: format!("检测到 {} 个 GGUF 版本（llama.cpp）", artifacts.len()),
                model_type: Some(ModelCategory::Llm),
                architecture: Some("llama-cpp-gguf".into()),
                artifacts,
                recommended_variant: recommended,
            });
        }

        // 先找完整匹配（KWS 优先，避免 ASR 误判）；无完整匹配再取最多匹配的 spec → Possible
        let mut best_partial: Option<(usize, &CompatibilitySpec, Vec<&str>)> = None;
        for spec in SHERPA_SPECS {
            // 含该规格禁止的标记文件（如 VITS 的 lexicon）→ 整个规格不参与，防误判
            let absent_hit = spec
                .absent
                .iter()
                .any(|a| files.iter().any(|f| f.path.to_lowercase().contains(a)));
            if absent_hit {
                continue;
            }
            let matched_count = spec
                .required
                .iter()
                .filter(|s| files.iter().any(|f| spec_matches(s, &f.path)))
                .count();
            if matched_count == spec.required.len() {
                let artifact = build_sherpa_artifact(spec, files);
                return Some(Compatibility {
                    level: CompatibilityLevel::Compatible,
                    reason: format!("sherpa-onnx {}：必需文件齐全", spec.architecture),
                    model_type: Some(spec.model_type),
                    architecture: Some(spec.architecture.into()),
                    artifacts: vec![artifact],
                    recommended_variant: None,
                });
            }
            if matched_count > 0 {
                let missing: Vec<&str> = spec
                    .required
                    .iter()
                    .copied()
                    .filter(|s| !files.iter().any(|f| spec_matches(s, &f.path)))
                    .map(|s| match s.equals.first() {
                        Some(name) => *name,
                        None => s.contains.first().copied().unwrap_or("?"),
                    })
                    .collect();
                if best_partial
                    .as_ref()
                    .is_none_or(|(c, _, _)| matched_count > *c)
                {
                    best_partial = Some((matched_count, spec, missing));
                }
            }
        }
        if let Some((_, spec, missing)) = best_partial {
            return Some(Compatibility {
                level: CompatibilityLevel::Possible,
                reason: format!(
                    "检测到 {} 架构的部分文件，缺少：{}",
                    spec.display_name,
                    missing.join(", ")
                ),
                model_type: Some(spec.model_type),
                architecture: Some(spec.architecture.into()),
                artifacts: Vec::new(),
                recommended_variant: None,
            });
        }
        None
    }
}

impl Default for CompatibilityResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> RemoteModelFile {
        RemoteModelFile {
            path: path.into(),
            size: Some(size),
            file_type: "file".into(),
            lfs: None,
            sha256: None,
        }
    }

    fn files_of(paths: &[&str]) -> Vec<RemoteModelFile> {
        paths.iter().map(|p| file(p, 1024)).collect()
    }

    #[test]
    fn test_summary_llm_gguf_signal() {
        // text-generation + gguf tag → Compatible
        let s = RemoteModelSummary {
            repo_id: "Some/Qwen3-4B-GGUF".into(),
            pipeline_tag: Some("text-generation".into()),
            tags: vec!["gguf".into(), "qwen3".into()],
            ..Default::default()
        };
        assert_eq!(
            assess_summary(&s, Some(ModelCategory::Llm)),
            CompatibilityLevel::Compatible
        );
        // 纯 transformers（无 gguf tag）→ Unsupported
        let t = RemoteModelSummary {
            repo_id: "Some/Qwen3-4B".into(),
            pipeline_tag: Some("text-generation".into()),
            tags: vec!["transformers".into()],
            ..Default::default()
        };
        assert_eq!(
            assess_summary(&t, Some(ModelCategory::Llm)),
            CompatibilityLevel::Unsupported
        );
    }

    #[test]
    fn test_summary_speech_signal() {
        // whisper（automatic-speech-recognition，无 sherpa）→ Unsupported
        let w = RemoteModelSummary {
            repo_id: "openai/whisper-large-v3".into(),
            pipeline_tag: Some("automatic-speech-recognition".into()),
            tags: vec!["whisper".into(), "transformers".into()],
            ..Default::default()
        };
        assert_eq!(
            assess_summary(&w, Some(ModelCategory::Asr)),
            CompatibilityLevel::Unsupported
        );
        // sherpa tag → Possible（需查文件确认文件组）
        let sp = RemoteModelSummary {
            repo_id: "Some/sherpa-onnx-model".into(),
            pipeline_tag: Some("automatic-speech-recognition".into()),
            tags: vec!["sherpa-onnx".into()],
            ..Default::default()
        };
        assert_eq!(
            assess_summary(&sp, Some(ModelCategory::Asr)),
            CompatibilityLevel::Possible
        );
    }

    #[test]
    fn test_summary_other_pipeline_unsupported() {
        // 图像模型 → Unsupported
        let img = RemoteModelSummary {
            repo_id: "Some/Flux".into(),
            pipeline_tag: Some("text-to-image".into()),
            tags: vec![],
            ..Default::default()
        };
        assert_eq!(assess_summary(&img, None), CompatibilityLevel::Unsupported);
        // 无 category 且 pipeline 不可识别 → Unsupported
        assert_eq!(
            assess_summary(&img, Some(ModelCategory::Llm)),
            CompatibilityLevel::Unsupported
        );
    }

    #[test]
    fn test_llm_compatible_from_gguf_files() {
        let r = CompatibilityResolver::new();
        let files = files_of(&[
            "Qwen3-4B-Q4_K_M.gguf",
            "Qwen3-4B-Q5_K_M.gguf",
            "README.md",
            "config.json",
        ]);
        let c = r.from_files("Some/Qwen3-4B-GGUF", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Llm));
        assert_eq!(c.artifacts.len(), 2, "同 base 不同 quant → 两个 artifact");
        assert_eq!(c.recommended_variant.as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn test_llm_same_quant_diff_base_two_artifacts() {
        let r = CompatibilityResolver::new();
        let files = files_of(&["foo-Q4_K_M.gguf", "bar-Q4_K_M.gguf"]);
        let c = r.from_files("x/y", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.artifacts.len(), 2, "不同 base 同 quant 必须两个 artifact");
    }

    #[test]
    fn test_llm_split_complete_aggregates() {
        let r = CompatibilityResolver::new();
        let files = files_of(&[
            "model-Q4_K_M-00001-of-00002.gguf",
            "model-Q4_K_M-00002-of-00002.gguf",
        ]);
        let c = r.from_files("x/y", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.artifacts.len(), 1, "split 聚合为一个 artifact");
        assert!(c.artifacts[0].installable);
        assert_eq!(c.artifacts[0].files.len(), 2);
        assert_eq!(c.artifacts[0].variant.as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn test_llm_split_missing_not_installable() {
        let r = CompatibilityResolver::new();
        let files = files_of(&["model-Q4_K_M-00001-of-00002.gguf"]);
        let c = r.from_files("x/y", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.artifacts.len(), 1);
        assert!(!c.artifacts[0].installable, "缺 shard 禁止一键安装");
    }

    #[test]
    fn test_asr_compatible_file_group() {
        let r = CompatibilityResolver::new();
        let files = files_of(&[
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            "tokens.txt",
        ]);
        let c = r.from_files("x/asr", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Asr));
        assert_eq!(c.artifacts.len(), 1);
        assert!(c.artifacts[0].installable);
        assert_eq!(c.artifacts[0].runtime, "sherpa-onnx");
    }

    #[test]
    fn test_kws_requires_keywords() {
        let r = CompatibilityResolver::new();
        // 有 encoder/decoder/joiner/tokens 但无 keywords → 走 ASR 而非 KWS
        let files = files_of(&["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"]);
        let c = r.from_files("x/y", &files);
        assert_eq!(c.model_type, Some(ModelCategory::Asr));
        // 加上 test_wavs/keywords.txt → KWS
        let files2 = files_of(&[
            "encoder.onnx",
            "decoder.onnx",
            "joiner.onnx",
            "tokens.txt",
            "test_wavs/keywords.txt",
        ]);
        let c2 = r.from_files("x/y", &files2);
        assert_eq!(c2.model_type, Some(ModelCategory::Kws));
    }

    #[test]
    fn test_tts_compatible_file_group() {
        let r = CompatibilityResolver::new();
        let files = files_of(&[
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "vocos_24khz.onnx",
            "tokens.txt",
            "lexicon.txt",
        ]);
        let c = r.from_files("x/tts", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Tts));
    }

    #[test]
    fn test_sensevoice_compatible_file_group() {
        let r = CompatibilityResolver::new();
        let files = files_of(&["model.int8.onnx", "tokens.txt"]);
        let c = r.from_files("x/sensevoice", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Asr));
        assert_eq!(
            c.architecture.as_deref(),
            Some("sherpa-asr-offline-sensevoice")
        );
    }

    #[test]
    fn test_whisper_compatible_file_group() {
        let r = CompatibilityResolver::new();
        let files = files_of(&["tiny-encoder.onnx", "tiny-decoder.onnx", "tiny-tokens.txt"]);
        let c = r.from_files("x/whisper", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Asr));
        assert_eq!(
            c.architecture.as_deref(),
            Some("sherpa-asr-offline-whisper")
        );
    }

    #[test]
    fn test_paraformer_compatible_file_group() {
        let r = CompatibilityResolver::new();
        // 官方包布局：裸名 int8 + fp32 双份 + tokens（无 joiner）
        let files = files_of(&[
            "encoder.int8.onnx",
            "encoder.onnx",
            "decoder.int8.onnx",
            "decoder.onnx",
            "tokens.txt",
            "test_wavs/0.wav",
        ]);
        let c = r.from_files("x/paraformer", &files);
        assert_eq!(c.level, CompatibilityLevel::Compatible);
        assert_eq!(c.model_type, Some(ModelCategory::Asr));
        assert_eq!(
            c.architecture.as_deref(),
            Some("sherpa-asr-streaming-paraformer")
        );
        assert_eq!(c.artifacts.len(), 1);
        assert!(c.artifacts[0].installable);

        // fp32-only 目录同样识别
        let files_fp32 = files_of(&["encoder.onnx", "decoder.onnx", "tokens.txt"]);
        let c2 = r.from_files("x/paraformer-fp32", &files_fp32);
        assert_eq!(c2.level, CompatibilityLevel::Compatible);
        assert_eq!(
            c2.architecture.as_deref(),
            Some("sherpa-asr-streaming-paraformer")
        );
    }

    #[test]
    fn test_paraformer_not_misclassified_as_zipformer() {
        // paraformer 三件在 zipformer ASR_SPEC 下只有 3/4（缺 joiner）；
        // PARAFORMER_SPEC 精确名全满足 → 必须归 streaming-paraformer
        let r = CompatibilityResolver::new();
        let files = files_of(&["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"]);
        let c = r.from_files("x/paraformer", &files);
        assert_ne!(
            c.architecture.as_deref(),
            Some("sherpa-asr-streaming-zipformer")
        );
        // 反向：zipformer 前缀命名不落 paraformer（equals 精确名不命中）
        let zip_files = files_of(&[
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
            "tokens.txt",
        ]);
        let cz = r.from_files("x/zipformer", &zip_files);
        assert_ne!(
            cz.architecture.as_deref(),
            Some("sherpa-asr-streaming-paraformer")
        );
    }

    #[test]
    fn test_vits_not_misclassified_as_sensevoice() {
        // VITS 目录（model.onnx + lexicon + tokens）不得被 SENSEVOICE_SPEC 判为 ASR
        let r = CompatibilityResolver::new();
        let files = files_of(&["model.onnx", "lexicon.txt", "tokens.txt"]);
        let c = r.from_files("x/vits", &files);
        assert_ne!(
            c.architecture.as_deref(),
            Some("sherpa-asr-offline-sensevoice"),
            "含 lexicon 的目录不应被 SenseVoice 规格捕获"
        );
        assert_ne!(
            c.level,
            CompatibilityLevel::Compatible,
            "VITS 目录不应因 model.onnx 被判 Compatible ASR"
        );
    }

    #[test]
    fn test_partial_sherpa_is_possible() {
        let r = CompatibilityResolver::new();
        // 裸名 encoder/decoder + tokens 现在是完整的流式 Paraformer 文件组（Compatible），
        // 「部分」场景改用缺 tokens 的集合：paraformer 缺 tokens、zipformer 缺 joiner+tokens
        let complete = files_of(&["encoder.onnx", "decoder.onnx", "tokens.txt"]);
        let c = r.from_files("x/y", &complete);
        assert_eq!(
            c.level,
            CompatibilityLevel::Compatible,
            "裸名三件套应判完整 Paraformer"
        );

        let files = files_of(&["encoder.onnx", "decoder.onnx"]);
        let c = r.from_files("x/y", &files);
        assert_eq!(
            c.level,
            CompatibilityLevel::Possible,
            "缺 tokens → Possible 而非 Unsupported"
        );
    }

    #[test]
    fn test_unrelated_files_unsupported() {
        let r = CompatibilityResolver::new();
        let files = files_of(&["model.safetensors", "config.json", "README.md"]);
        let c = r.from_files("x/y", &files);
        assert_eq!(c.level, CompatibilityLevel::Unsupported);
        assert!(c.artifacts.is_empty());
    }
}
