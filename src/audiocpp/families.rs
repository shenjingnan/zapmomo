//! audiocpp 模型族静态描述表（单一事实源）。
//!
//! 每个接入 audio.cpp sidecar 的 TTS 模型族一条 [`AudiocppFamilyDesc`] 记录，
//! 取代此前散落在 `mod.rs` 常量 / `tts::config::preflight` / `server_config` /
//! `client` 各处的 pocket 单模型硬编码。新增模型族 = 本表加一条记录 +
//! registry/manifest 各一个条目 + 前端 preset 一条（技术方案 §4.3）。

use crate::tts::config::TtsModelKind;

/// 音色语义（决定 [`crate::tts::TtsVoiceParams`] 到请求体字段的映射，见 client）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSemantics {
    /// 固定具名音色（pocket）：`Named`/缺省 → 该音色；`Sid` → 默认音色；
    /// `Reference` → 不支持（报错）。
    FixedNamed(&'static str),
    /// 参考音频克隆（omnivoice）：`Reference` → `voice_ref`+`reference_text`；
    /// `Named` → 透传 `voice`；`Sid`/缺省 → 省略 voice 字段（server auto voice）。
    ReferenceClone,
}

/// 单个 audiocpp 模型族的静态描述。
#[derive(Debug)]
pub struct AudiocppFamilyDesc {
    /// server config `models[].id` 与 `/v1/audio/speech` 请求体 `model`（两侧同源）。
    pub model_id: &'static str,
    /// audio.cpp `model_specs` 的 family 标识。
    pub family: &'static str,
    /// 主 GGUF 文件名（相对模型目录，与 manifest asset 一致）。
    pub gguf_file: &'static str,
    /// preflight / registry 完整性共用清单（相对模型目录）。
    pub required_files: &'static [&'static str],
    /// 输出采样率初值（Hz；client 首响应按 wav 头校准）。
    pub sample_rate: i32,
    /// 缺省推理后端（server `backend`）；用户显式配置 `[tts].provider` 优先。
    /// pocket 100M 实测 CPU 快于 Metal；omnivoice 0.6B 实测 CPU RTF 6.6 不可用、
    /// Metal RTF 0.41 达标（技术方案阶段 1 实测，2026-08-23）。
    pub default_provider: &'static str,
    /// 音色语义。
    pub voice_semantics: VoiceSemantics,
    /// preflight 缺文件时的安装提示命令。
    pub registry_hint: &'static str,
}

impl AudiocppFamilyDesc {
    /// server config `load_options`（pocket 需显式 language；omnivoice 自动检测）。
    pub fn load_options(&self) -> serde_json::Value {
        match self.family {
            "pocket_tts" => serde_json::json!({ "language": "english" }),
            _ => serde_json::json!({}),
        }
    }
}

/// PocketTTS English q8_0（固定音色 alba，CPU 友好）。
///
/// `POCKET_EMBEDDINGS_FILE` 单独导出供 registry role 映射引用（切片索引
/// 不能常量提升），与 `POCKET.required_files[1]` 的一致性由测试锚定。
pub const POCKET_EMBEDDINGS_FILE: &str = "embeddings/alba.safetensors";
pub const POCKET: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "pocket-tts-english",
    family: "pocket_tts",
    gguf_file: "pocket-tts-english-q8_0.gguf",
    required_files: &["pocket-tts-english-q8_0.gguf", POCKET_EMBEDDINGS_FILE],
    sample_rate: 24_000,
    default_provider: "cpu",
    voice_semantics: VoiceSemantics::FixedNamed("alba"),
    registry_hint: "zapmomo tts install-model --registry-id tts-pocket-english-audiocpp",
};

/// OmniVoice q8_0（Qwen3-0.6B 基座，600+ 语种零样本克隆 + 声音设计；Metal 必需）。
///
/// 单文件 GGUF（generator 与 audio_tokenizer 双权重内嵌，无 embeddings 副件）。
pub const OMNIVOICE: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "omnivoice",
    family: "omnivoice",
    gguf_file: "omnivoice-q8_0.gguf",
    required_files: &["omnivoice-q8_0.gguf"],
    sample_rate: 24_000,
    default_provider: "metal",
    voice_semantics: VoiceSemantics::ReferenceClone,
    registry_hint: "zapmomo tts install-model --registry-id tts-omnivoice-q8-audiocpp",
};

/// 按模型类型查表；sherpa-only kind 返回 None（audiocpp 后端不支持该组合）。
pub fn family_desc(kind: TtsModelKind) -> Option<&'static AudiocppFamilyDesc> {
    match kind {
        TtsModelKind::Pocket => Some(&POCKET),
        TtsModelKind::Omnivoice => Some(&OMNIVOICE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表覆盖锚点：pocket/omnivoice 可查，sherpa 族与 Kitten/Supertonic 不可查。
    #[test]
    fn test_family_desc_coverage() {
        assert_eq!(
            family_desc(TtsModelKind::Pocket).unwrap().family,
            "pocket_tts"
        );
        assert_eq!(
            family_desc(TtsModelKind::Omnivoice).unwrap().family,
            "omnivoice"
        );
        for kind in [
            TtsModelKind::Zipvoice,
            TtsModelKind::Vits,
            TtsModelKind::Matcha,
            TtsModelKind::Kokoro,
            TtsModelKind::Kitten,
            TtsModelKind::Supertonic,
        ] {
            assert!(family_desc(kind).is_none(), "{kind:?} 不应有 audiocpp 描述");
        }
    }

    /// omnivoice 单文件清单 / metal 默认后端 / 克隆语义；pocket 双文件 + cpu。
    #[test]
    fn test_family_records_shape() {
        let omni = family_desc(TtsModelKind::Omnivoice).unwrap();
        assert_eq!(omni.required_files, &["omnivoice-q8_0.gguf"]);
        assert_eq!(omni.default_provider, "metal");
        assert_eq!(omni.voice_semantics, VoiceSemantics::ReferenceClone);
        assert_eq!(omni.load_options(), serde_json::json!({}));

        let pocket = family_desc(TtsModelKind::Pocket).unwrap();
        assert_eq!(pocket.required_files.len(), 2);
        // 单独导出的 embeddings 常量与清单第 2 项一致（registry 引用它）
        assert_eq!(pocket.required_files[1], POCKET_EMBEDDINGS_FILE);
        assert_eq!(pocket.default_provider, "cpu");
        assert_eq!(pocket.voice_semantics, VoiceSemantics::FixedNamed("alba"));
        assert_eq!(
            pocket.load_options(),
            serde_json::json!({ "language": "english" })
        );
        // model_id 与 registry id 提示一一对应（preflight 提示语可执行）
        assert!(pocket.registry_hint.contains("tts-pocket-english-audiocpp"));
        assert!(omni.registry_hint.contains("tts-omnivoice-q8-audiocpp"));
    }
}
