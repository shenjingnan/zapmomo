//! 模型库 Registry：编译期嵌入 `models/model_registry.json` 的目录解析。
//!
//! 一个 RegistryModel = 一个实际可加载的模型版本/变体（如 `qwen3-1.7b-q4-k-m`）。
//! 下载源（URL/sha256/size）不在此重复维护，而是通过 `download.manifest_role`
//! 引用 `models/manifest.json`（单一数据源）。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::asr::config::AsrModelKind;
use crate::kws::model::{ModelAsset, asset_by_role};
use crate::tts::config::TtsModelKind;

/// 能力类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelType {
    Kws,
    Asr,
    Llm,
    Tts,
}

impl ModelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelType::Kws => "kws",
            ModelType::Asr => "asr",
            ModelType::Llm => "llm",
            ModelType::Tts => "tts",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "kws" => Some(ModelType::Kws),
            "asr" => Some(ModelType::Asr),
            "llm" => Some(ModelType::Llm),
            "tts" => Some(ModelType::Tts),
            _ => None,
        }
    }
}

/// 顶层目录。
#[derive(Debug, Clone, Deserialize)]
pub struct ModelRegistry {
    #[serde(rename = "schema_version")]
    pub schema_version: u32,
    pub models: Vec<RegistryModel>,
}

/// 单个目录条目。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryModel {
    pub id: String,
    /// 目录基名（sherpa 模型目录名 / LLM 期望目录名）
    pub name: String,
    pub display_name: String,
    #[serde(rename = "model_type")]
    pub model_type: ModelType,
    /// TTS 子类型（zipvoice/omnivoice/...；仅 `model_type == Tts` 有意义，其余为 None）
    #[serde(default)]
    pub tts_kind: Option<TtsModelKind>,
    /// ASR 子类型（zipformer/sensevoice/whisper；仅 `model_type == Asr` 有意义，其余为 None）
    #[serde(default)]
    pub asr_kind: Option<AsrModelKind>,
    pub runtime: String,
    pub format: String,
    pub description: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub parameter_count: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    /// LLM 条目：具体 GGUF 文件名
    #[serde(default)]
    pub file_name: Option<String>,
    pub version: String,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// 安装所需资产 role 列表（安装与完整性共用同一份定义）
    #[serde(default)]
    pub required_assets: Vec<String>,
    /// 可选增强资产 role 列表（如 ASR 的 punctuation，缺失不影响可用性）
    #[serde(default)]
    pub optional_assets: Vec<String>,
    /// 可用平台约束（`None` = 全平台；取值对齐 target triple 简写，如
    /// "darwin-aarch64"）。平台不符的条目在模型库中隐藏——如 omnivoice
    /// 依赖 Metal 加速，仅 macOS arm64 的 sidecar 构建编入 Metal 后端，
    /// 其余平台纯 CPU 实测 RTF 不可用（技术方案 R1 预案）。
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    /// `None` = 无内置下载源（需导入本地文件；当前 LLM 预设均已有 manifest 下载源）
    pub download: Option<RegistryDownload>,
}

/// 当前平台的 triple 简写（与 registry `platforms` 字段取值对齐）。
fn current_platform_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "darwin-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "darwin-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        "other"
    }
}

impl RegistryModel {
    pub fn is_llm(&self) -> bool {
        self.model_type == ModelType::Llm
    }
}

/// 下载引用：只存 manifest role，真实 URL/hash/size 由 manifest 单源解析。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RegistryDownload {
    pub manifest_role: String,
    #[serde(default)]
    pub extra_roles: Vec<String>,
    #[serde(default)]
    pub kind: String,
}

const REGISTRY_JSON: &str = include_str!("../../models/model_registry.json");

/// 解析一次并缓存。
fn registry() -> &'static ModelRegistry {
    static CACHE: OnceLock<ModelRegistry> = OnceLock::new();
    CACHE.get_or_init(|| serde_json::from_str(REGISTRY_JSON).expect("内嵌模型目录无效"))
}

/// 所有目录条目（保持 JSON 顺序，即推荐顺序）。
pub fn all_models() -> &'static [RegistryModel] {
    &registry().models
}

/// 按 id 查找目录条目。
pub fn model_by_id(id: &str) -> Option<&'static RegistryModel> {
    registry().models.iter().find(|m| m.id == id)
}

/// 当前平台可用的目录条目（`platforms` 为 None 的条目恒可用）。
///
/// 模型库列表（`list_models`）与解析入口都应以此过滤，保证平台受限条目
/// （如仅 Metal 平台的 omnivoice）在其余平台不可见、不可下载。
pub fn models_for_current_platform() -> Vec<&'static RegistryModel> {
    all_models()
        .iter()
        .filter(|m| {
            m.platforms
                .as_ref()
                .is_none_or(|list| list.iter().any(|p| p == current_platform_triple()))
        })
        .collect()
}

/// 按下载引用解析 manifest 资产。
pub fn asset_for(model: &RegistryModel) -> Option<&'static ModelAsset> {
    let role = model.download.as_ref()?.manifest_role.as_str();
    asset_by_role(role)
}

/// manifest role 对应的必需文件清单。
///
/// 安装（`install_asset_to` 的幂等/校验）与完整性判断使用**同一份**定义，
/// 避免出现「安装要求 A+B、完整性只查 A」的不一致。
pub fn required_files_for_role(role: &str) -> &'static [&'static str] {
    match role {
        "wake-word" => &crate::kws::model::KWS_REQUIRED_FILES,
        // 离线 ASR：精确 role 必须在 asr-* 通配之前，否则被通配吞掉返回错误 4 件套
        "asr-sensevoice" => &crate::asr::config::SENSEVOICE_REQUIRED_FILES,
        "asr-whisper-tiny" => &crate::asr::config::WHISPER_TINY_REQUIRED_FILES,
        "asr-whisper-base" => &crate::asr::config::WHISPER_BASE_REQUIRED_FILES,
        // 流式 Paraformer：裸名三件套（int8），同样先于 asr-* 通配
        "asr-paraformer-bilingual-zh-en" | "asr-paraformer-trilingual-zh-cantonese-en" => {
            &crate::asr::config::PARAFORMER_REQUIRED_FILES
        }
        // 离线 Qwen3-ASR：conv_frontend + 裸名 int8 二件 + tokenizer 三文件
        // （has_required_files 是 is_file 语义，tokenizer 目录不能作条目），先于通配
        "asr-qwen3" => &crate::asr::config::QWEN3_REQUIRED_FILES,
        // audiocpp Qwen3-ASR：单文件 GGUF，先于 asr-* 通配
        "asr-audiocpp-qwen3-06b" => &[crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file],
        // 所有 streaming zipformer ASR（含每个 ASR 的唯一 role）共用同一组 4 文件
        r if r == "asr" || r.starts_with("asr-") => &crate::asr::config::REQUIRED_FILES,
        "punctuation" => &crate::asr::config::PUNCT_REQUIRED_FILES,
        "tts" => &crate::tts::config::REQUIRED_FILES,
        "tts-vocoder" => &[crate::tts::config::DEFAULT_VOCODER],
        "tts-audiocpp-omnivoice" => &[crate::audiocpp::families::OMNIVOICE.gguf_file],
        "tts-audiocpp-voxcpm2" => &[crate::audiocpp::families::VOXCPM2.gguf_file],
        // Qwen3-TTS 两尺寸（音色克隆）：钉死各自 gguf 主文件名
        "tts-audiocpp-qwen3-06b" => &[crate::audiocpp::families::QWEN3_TTS_06B.gguf_file],
        "tts-audiocpp-qwen3-17b" => &[crate::audiocpp::families::QWEN3_TTS_17B.gguf_file],
        // LLM：必需文件由 `RegistryModel.file_name` 推导（见 install_managed_model），这里不维护静态表
        _ => &[],
    }
}

/// 按 registry id 查 TTS 子类型（非 TTS 或无 `tts_kind` 时返回 None）。
pub fn registry_tts_kind(id: &str) -> Option<TtsModelKind> {
    model_by_id(id).and_then(|m| m.tts_kind)
}

/// 按 registry id 查 ASR 子类型（非 ASR 或无 `asr_kind` 时返回 None）。
pub fn registry_asr_kind(id: &str) -> Option<AsrModelKind> {
    model_by_id(id).and_then(|m| m.asr_kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_parses() {
        let models = all_models();
        assert_eq!(
            models.len(),
            19,
            "应为 7 个首批（含 1 KWS）+ 5 个 ASR + 2 个新 TTS + 3 个新 ASR + 2 个流式 Paraformer + 1 个 Qwen3-ASR + 1 个 audiocpp OmniVoice + 1 个 audiocpp VoxCPM2 + 2 个 Qwen3-TTS + 1 个 audiocpp Qwen3-ASR（LLM 条目已随本地推理移除；vits/matcha/kokoro/pocket TTS 已移除）"
        );
        assert!(
            models
                .iter()
                .all(|m| !m.id.is_empty() && !m.display_name.is_empty())
        );
        // id 唯一
        let mut ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), models.len(), "Registry id 必须唯一");
        // 本地 LLM 推理已移除：registry 不应再有 LLM 条目
        assert!(
            !models.iter().any(|m| m.is_llm()),
            "LLM 已改为远程连接，registry 不应包含 LLM 条目"
        );
    }

    #[test]
    fn test_registry_manifest_roles_exist() {
        // 所有 download.manifest_role / required_assets / optional_assets 都必须在 manifest 中存在
        for m in all_models() {
            if let Some(d) = &m.download {
                assert!(
                    asset_by_role(&d.manifest_role).is_some(),
                    "manifest_role '{}' 在 manifest 中不存在 (model {})",
                    d.manifest_role,
                    m.id
                );
                for extra in &d.extra_roles {
                    assert!(
                        asset_by_role(extra).is_some(),
                        "extra_roles '{}' 在 manifest 中不存在 (model {})",
                        extra,
                        m.id
                    );
                }
            }
            for role in m.required_assets.iter().chain(m.optional_assets.iter()) {
                assert!(
                    asset_by_role(role).is_some(),
                    "asset role '{}' 在 manifest 中不存在 (model {})",
                    role,
                    m.id
                );
            }
        }
    }

    #[test]
    fn test_model_by_id_and_order() {
        let m = model_by_id("asr-sensevoice-zh-en-ja-ko-yue").expect("按 id 查找");
        assert_eq!(m.model_type, ModelType::Asr);
        // 推荐顺序 = registry 原始顺序（首个是 KWS）
        assert_eq!(all_models()[0].model_type, ModelType::Kws);
    }

    #[test]
    fn test_required_files_for_role() {
        assert_eq!(required_files_for_role("asr").len(), 4);
        assert_eq!(required_files_for_role("punctuation").len(), 1);
        assert_eq!(required_files_for_role("tts").len(), 5); // 含 vocoder
        assert_eq!(required_files_for_role("tts-vocoder").len(), 1);
        assert_eq!(required_files_for_role("wake-word").len(), 5);
        // 新离线 ASR：精确 role 优先于 asr-* 通配
        assert_eq!(required_files_for_role("asr-sensevoice").len(), 2);
        assert!(required_files_for_role("asr-sensevoice").contains(&"model.int8.onnx"));
        assert_eq!(required_files_for_role("asr-whisper-tiny").len(), 3);
        assert!(required_files_for_role("asr-whisper-tiny").contains(&"tiny-tokens.txt"));
        assert_eq!(required_files_for_role("asr-whisper-base").len(), 3);
        assert!(required_files_for_role("asr-whisper-base").contains(&"base-encoder.onnx"));
        // 流式 Paraformer：裸名三件套（int8），先于 asr-* 通配
        assert_eq!(
            required_files_for_role("asr-paraformer-bilingual-zh-en").len(),
            3
        );
        assert!(
            required_files_for_role("asr-paraformer-bilingual-zh-en")
                .contains(&"encoder.int8.onnx")
        );
        assert_eq!(
            required_files_for_role("asr-paraformer-trilingual-zh-cantonese-en").len(),
            3
        );
        // 离线 Qwen3-ASR：6 件（含 tokenizer 目录内三文件，目录不能作 is_file 条目）
        let q3 = required_files_for_role("asr-qwen3");
        assert_eq!(q3.len(), 6, "不应被 asr-* 通配吞成 4 件套");
        assert!(q3.contains(&"conv_frontend.onnx"));
        assert!(q3.contains(&"tokenizer/vocab.json"));
        assert!(!q3.contains(&"tokenizer"), "目录不能作完整性条目");
        // 回归：既有 streaming zipformer role 仍为 4 件套（不被新精确 arm 吞掉）
        assert_eq!(required_files_for_role("asr-zh-14m").len(), 4);
        // audiocpp Qwen3-ASR：单文件 GGUF（families 常量单源），先于 asr-* 通配
        assert_eq!(
            required_files_for_role("asr-audiocpp-qwen3-06b"),
            &[crate::audiocpp::asr_families::QWEN3_ASR_06B.gguf_file]
        );
        // Qwen3-TTS 两尺寸：钉死各自 gguf 主文件名（families 常量单源）
        assert_eq!(
            required_files_for_role("tts-audiocpp-qwen3-06b"),
            &[crate::audiocpp::families::QWEN3_TTS_06B.gguf_file]
        );
        assert_eq!(
            required_files_for_role("tts-audiocpp-qwen3-17b"),
            &[crate::audiocpp::families::QWEN3_TTS_17B.gguf_file]
        );
        assert!(required_files_for_role("unknown").is_empty());
    }

    #[test]
    fn test_registry_tts_kind() {
        assert_eq!(
            registry_tts_kind("tts-zipvoice-distill-int8"),
            Some(TtsModelKind::Zipvoice)
        );
        assert_eq!(
            registry_tts_kind("tts-omnivoice-q8-audiocpp"),
            Some(TtsModelKind::Omnivoice)
        );
        assert_eq!(
            registry_tts_kind("tts-qwen3-06b-base-q8-audiocpp"),
            Some(TtsModelKind::Qwen3Tts06)
        );
        assert_eq!(
            registry_tts_kind("tts-qwen3-17b-base-q8-audiocpp"),
            Some(TtsModelKind::Qwen3Tts17)
        );
        // 已移除的 vits/matcha/kokoro/pocket 条目不再收录
        for id in [
            "tts-vits-melo-zh-en",
            "tts-matcha-zh-baker",
            "tts-kokoro-int8-multi-lang-v1-1",
            "tts-kokoro-multi-lang-v1-1",
            "tts-pocket-english-audiocpp",
        ] {
            assert!(model_by_id(id).is_none(), "{id} 应已从 registry 移除");
        }
        // 非 TTS 或无 tts_kind → None
        assert_eq!(registry_tts_kind("kws-zipformer-zh-en-3m"), None);
        assert_eq!(registry_tts_kind("不存在"), None);
    }

    /// 平台过滤：omnivoice 仅darwin-aarch64；无 platforms 的条目全平台可见。
    /// 本机为 darwin-aarch64 时 omnivoice 在列；其它平台的 CI 通过「显式三元组
    /// 判定函数」覆盖，不依赖宿主平台。
    #[test]
    fn test_platforms_filter() {
        let omni = model_by_id("tts-omnivoice-q8-audiocpp").unwrap();
        assert_eq!(
            omni.platforms.as_deref(),
            Some(&["darwin-aarch64".to_string()][..])
        );
        // 显式判定（不依赖宿主平台）
        let visible = |triple: &str| {
            omni.platforms
                .as_ref()
                .is_none_or(|list| list.iter().any(|p| p == triple))
        };
        assert!(visible("darwin-aarch64"));
        assert!(!visible("darwin-x86_64"));
        assert!(!visible("linux-x86_64"));
        assert!(!visible("windows-x86_64"));
        // 无 platforms 的条目恒可见
        let zipvoice = model_by_id("tts-zipvoice-distill-int8").unwrap();
        assert!(zipvoice.platforms.is_none());
        // 全量条目在当前平台的过滤数 ≤ 总数，且 darwin-aarch64 下含 omnivoice
        let filtered = models_for_current_platform();
        assert!(filtered.len() <= all_models().len());
        if current_platform_triple() == "darwin-aarch64" {
            assert!(filtered.iter().any(|m| m.id == "tts-omnivoice-q8-audiocpp"));
        }
    }

    #[test]
    fn test_registry_asr_kind() {
        use crate::asr::config::AsrModelKind;
        assert_eq!(
            registry_asr_kind("asr-sensevoice-zh-en-ja-ko-yue"),
            Some(AsrModelKind::SenseVoice)
        );
        assert_eq!(
            registry_asr_kind("asr-whisper-tiny"),
            Some(AsrModelKind::Whisper)
        );
        assert_eq!(
            registry_asr_kind("asr-whisper-base"),
            Some(AsrModelKind::Whisper)
        );
        assert_eq!(
            registry_asr_kind("asr-paraformer-bilingual-zh-en"),
            Some(AsrModelKind::Paraformer)
        );
        assert_eq!(
            registry_asr_kind("asr-paraformer-trilingual-zh-cantonese-en"),
            Some(AsrModelKind::Paraformer)
        );
        assert_eq!(
            registry_asr_kind("asr-qwen3-0.6b"),
            Some(AsrModelKind::Qwen3Asr)
        );
        // 既有 streaming zipformer：asr_kind 缺省 → None（老行为）
        assert_eq!(registry_asr_kind("asr-streaming-bilingual-zh-en"), None);
        // 非 ASR 或不存在 → None
        assert_eq!(registry_asr_kind("tts-zipvoice-distill-int8"), None);
        assert_eq!(registry_asr_kind("不存在"), None);
    }

    #[test]
    fn test_default_asset_stays_zh_en() {
        // manifest 中第一个 role=="wake-word" 资产必须保持 zh-en（default_asset 语义）。
        let d = crate::kws::model::default_asset();
        assert_eq!(d.role, "wake-word");
        assert_eq!(d.name, "sherpa-onnx-kws-zipformer-zh-en-3M-2025-12-20");
    }
}
