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
    /// 参考音频克隆（omnivoice/voxcpm2）：`Reference` → `voice_ref`+`reference_text`；
    /// `Named` → 透传 `voice`；`Sid`/缺省 → 省略 voice 字段（server auto voice）。
    ReferenceClone,
    /// 强制参考音频克隆（qwen3_tts Base）：与 [`VoiceSemantics::ReferenceClone`]
    /// 同款 `voice_ref`+`reference_text` 映射，但 `Sid`/缺省**必须拦截**--
    /// 上游 Base 版无 auto voice（实测报错 "requires voice clone reference
    /// audio"），ZapMomo 侧提前报错给中文文案。
    ReferenceCloneRequired,
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
    /// 音色语义。
    pub voice_semantics: VoiceSemantics,
    /// 是否透传 `Named` 具名音色（ReferenceClone 族的差异项）：omnivoice 支持
    /// （server 端 preset/voice_dir 通道）；voxcpm2/qwen3_tts 上游仅接受 speaker
    /// reference，具名请求会被 server 拒绝——client 据此提前拦截并给中文文案。
    pub allows_named_voice: bool,
    /// 是否支持 SSE 伪流式（server config `mode` 与请求体 `stream_format` 的依据）。
    /// 流式矩阵（audio.cpp release-0.6.1 实测/README）：omnivoice ✅、voxcpm2 ✅、
    /// qwen3_tts ❌（上游 modes 仅 offline）、sherpa 全族 ❌
    /// （`OfflineTts` 整段合成，无 sidecar 语义）。
    /// offline-mode server 会拒绝 SSE 请求（实测 HTTP 500），故该标记同时决定
    /// server config 的 `mode:"streaming"` 翻转——两者必须同源。
    pub supports_streaming: bool,
    /// preflight 缺文件时的安装提示命令。
    pub registry_hint: &'static str,
}

impl AudiocppFamilyDesc {
    /// server config `load_options`（当前收录族均自动推导，恒空对象；
    /// 保留方法作为新增模型族的族差异扩展点）。
    pub fn load_options(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    /// 请求体 `options` 的族差异项（整段与流式两路径都携带）。
    ///
    /// voxcpm2 必须 `"retry_badcase": false`：上游约束（重试已完成 bad case 是
    /// offline-only 行为），且阶段 1 实测 streaming-mode server 下**非流式请求
    /// 同样必须携带**（缺省 500）——因此收敛在本方法而非流式专用路径。
    pub fn request_options(&self) -> serde_json::Value {
        match self.family {
            "voxcpm2" => serde_json::json!({ "retry_badcase": false }),
            _ => serde_json::json!({}),
        }
    }
}

/// OmniVoice q8_0（Qwen3-0.6B 基座，600+ 语种零样本克隆 + 声音设计）。
///
/// 单文件 GGUF（generator 与 audio_tokenizer 双权重内嵌，无 embeddings 副件）。
pub const OMNIVOICE: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "omnivoice",
    family: "omnivoice",
    gguf_file: "omnivoice-q8_0.gguf",
    required_files: &["omnivoice-q8_0.gguf"],
    sample_rate: 24_000,
    voice_semantics: VoiceSemantics::ReferenceClone,
    allows_named_voice: true,
    supports_streaming: true,
    registry_hint: "zapmomo tts install-model --registry-id tts-omnivoice-q8-audiocpp",
};

/// VoxCPM2 q8_0（OpenBMB MiniCPM-4 2B 基座，48kHz 录音室级 + 30 语种克隆）。
///
/// 单文件 GGUF（权重与 AudioVAE V2 内嵌）。流式为**音频帧级**（实测 0.16s/块连续
/// 吐出、首块 0.36s），与 omnivoice 的文本块伪流式不同；`options.retry_badcase=false`
/// 为硬约束（见 [`AudiocppFamilyDesc::request_options`]）。
pub const VOXCPM2: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "voxcpm2",
    family: "voxcpm2",
    gguf_file: "voxcpm2-q8_0.gguf",
    required_files: &["voxcpm2-q8_0.gguf"],
    sample_rate: 48_000,
    voice_semantics: VoiceSemantics::ReferenceClone,
    allows_named_voice: false,
    supports_streaming: true,
    registry_hint: "zapmomo tts install-model --registry-id tts-voxcpm2-q8-audiocpp",
};

/// Qwen3-TTS 0.6B Base q8_0（10 语种 3 秒音色克隆，24kHz）。
///
/// 单文件 GGUF（权重 + speech tokenizer + 全部 sidecar 内嵌，实测
/// `audiocpp.embedded_files` 含 11 个文件）。**Base 版必须参考音频**（无
/// auto voice 兜底，见 `VoiceSemantics::ReferenceCloneRequired`）；CustomVoice/
/// VoiceDesign 变体不在本期接入范围。GGUF 文件名无 `_v2` 后缀。
pub const QWEN3_TTS_06B: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "qwen3-tts-0.6b",
    family: "qwen3_tts",
    gguf_file: "qwen3-tts-12hz-0.6b-base-q8_0.gguf",
    required_files: &["qwen3-tts-12hz-0.6b-base-q8_0.gguf"],
    sample_rate: 24_000,
    voice_semantics: VoiceSemantics::ReferenceCloneRequired,
    allows_named_voice: false,
    supports_streaming: false,
    registry_hint: "zapmomo tts install-model --registry-id tts-qwen3-06b-base-q8-audiocpp",
};

/// Qwen3-TTS 1.7B Base q8_0（质量优先变体；GGUF 为上游 `_v2` 重打包版，文件名带 `_v2`）。
///
/// 同 0.6B 语义；1.7B RTF 预计 ~1.0+，句级流水线可能句间间隙，定位质量优先。
pub const QWEN3_TTS_17B: AudiocppFamilyDesc = AudiocppFamilyDesc {
    model_id: "qwen3-tts-1.7b",
    family: "qwen3_tts",
    gguf_file: "qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
    required_files: &["qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf"],
    sample_rate: 24_000,
    voice_semantics: VoiceSemantics::ReferenceCloneRequired,
    allows_named_voice: false,
    supports_streaming: false,
    registry_hint: "zapmomo tts install-model --registry-id tts-qwen3-17b-base-q8-audiocpp",
};

/// 按模型类型查表；sherpa-only kind 返回 None（audiocpp 后端不支持该组合）。
pub fn family_desc(kind: TtsModelKind) -> Option<&'static AudiocppFamilyDesc> {
    match kind {
        TtsModelKind::Omnivoice => Some(&OMNIVOICE),
        TtsModelKind::Voxcpm2 => Some(&VOXCPM2),
        TtsModelKind::Qwen3Tts06 => Some(&QWEN3_TTS_06B),
        TtsModelKind::Qwen3Tts17 => Some(&QWEN3_TTS_17B),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表覆盖锚点：omnivoice/voxcpm2/qwen3 可查，sherpa 族与 Kitten/Supertonic 不可查。
    #[test]
    fn test_family_desc_coverage() {
        assert_eq!(
            family_desc(TtsModelKind::Omnivoice).unwrap().family,
            "omnivoice"
        );
        assert_eq!(
            family_desc(TtsModelKind::Voxcpm2).unwrap().family,
            "voxcpm2"
        );
        for kind in [
            TtsModelKind::Zipvoice,
            TtsModelKind::Kitten,
            TtsModelKind::Supertonic,
        ] {
            assert!(family_desc(kind).is_none(), "{kind:?} 不应有 audiocpp 描述");
        }
    }

    /// omnivoice 单文件清单 / 克隆语义。
    #[test]
    fn test_family_records_shape() {
        let omni = family_desc(TtsModelKind::Omnivoice).unwrap();
        assert_eq!(omni.required_files, &["omnivoice-q8_0.gguf"]);
        assert_eq!(omni.voice_semantics, VoiceSemantics::ReferenceClone);
        assert!(omni.supports_streaming, "omnivoice 支持 SSE 伪流式");
        assert_eq!(omni.load_options(), serde_json::json!({}));

        // model_id 与 registry id 提示一一对应（preflight 提示语可执行）
        assert!(omni.registry_hint.contains("tts-omnivoice-q8-audiocpp"));

        // voxcpm2：48kHz / 帧级流式 / Named 不透传 / retry_badcase 硬约束
        let vox = family_desc(TtsModelKind::Voxcpm2).unwrap();
        assert_eq!(vox.required_files, &["voxcpm2-q8_0.gguf"]);
        assert_eq!(vox.sample_rate, 48_000, "VoxCPM2 输出 48kHz");
        assert!(vox.supports_streaming);
        assert!(!vox.allows_named_voice, "上游仅接受 speaker reference");
        assert_eq!(
            vox.request_options(),
            serde_json::json!({ "retry_badcase": false })
        );
        assert_eq!(omni.request_options(), serde_json::json!({}));

        // qwen3_tts 两尺寸：24kHz / 强制克隆 / 无流式 / 单文件清单
        let q06 = family_desc(TtsModelKind::Qwen3Tts06).unwrap();
        assert_eq!(q06.model_id, "qwen3-tts-0.6b");
        assert_eq!(q06.family, "qwen3_tts");
        assert_eq!(q06.required_files, &["qwen3-tts-12hz-0.6b-base-q8_0.gguf"]);
        assert_eq!(q06.sample_rate, 24_000);
        assert_eq!(q06.voice_semantics, VoiceSemantics::ReferenceCloneRequired);
        assert!(!q06.allows_named_voice, "Base 版仅接受 speaker reference");
        assert!(!q06.supports_streaming, "上游 modes 仅 offline");
        assert!(q06.registry_hint.contains("tts-qwen3-06b-base-q8-audiocpp"));
        assert_eq!(q06.load_options(), serde_json::json!({}));

        let q17 = family_desc(TtsModelKind::Qwen3Tts17).unwrap();
        assert_eq!(q17.model_id, "qwen3-tts-1.7b");
        assert_eq!(
            q17.required_files,
            &["qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf"]
        );
        assert_eq!(q17.sample_rate, 24_000);
        assert_eq!(q17.voice_semantics, VoiceSemantics::ReferenceCloneRequired);
    }
}
