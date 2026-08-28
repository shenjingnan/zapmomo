//! audiocpp ASR 模型族静态描述表（与 TTS 的 [`super::families`] 平行的单一事实源）。
//!
//! TTS 描述表含 VoiceSemantics / sample_rate / supports_streaming 等 TTS 语义字段，
//! 强行抽公共基类收益低、churn 大，故 ASR 族单独成表（技术方案 §3.2 决策 4）。
//! 新增 ASR 模型族 = 本表加一条记录 + registry/manifest 各一个条目 + 前端 preset 一条。

use crate::asr::config::AsrModelKind;
use std::path::Path;

/// 单个 audiocpp ASR 模型族的静态描述。
#[derive(Debug)]
pub struct AudiocppAsrFamilyDesc {
    /// server config `models[].id` 与 `/v1/audio/transcriptions` 请求体 `model`（两侧同源）。
    pub model_id: &'static str,
    /// audio.cpp `model_specs` 的 family 标识。
    pub family: &'static str,
    /// 主 GGUF 文件名（相对模型目录，与 manifest asset 一致）。
    pub gguf_file: &'static str,
    /// preflight / registry 完整性共用清单（相对模型目录）。
    pub required_files: &'static [&'static str],
    /// 缺省推理后端（server `backend`）；用户显式配置 `[asr].provider` 优先。
    pub default_provider: &'static str,
    /// preflight 缺文件时的安装提示命令。
    pub registry_hint: &'static str,
}

/// Qwen3-ASR 0.6B q8_0（29 语言自动识别，LLM 自回归解码；Metal 必需——sherpa
/// ONNX int8 版 CPU 解码慢是接入本族的动机）。
///
/// 单文件 GGUF（tokenizer/config sidecar 内嵌，上游 converter 默认嵌入）。
/// 无 hotwords 能力（上游 spec 无此选项，sherpa 版独有）；量化降低自动语种识别
/// 可靠性（上游文档明示），`language` 显式指定可兜底。
pub const QWEN3_ASR_06B: AudiocppAsrFamilyDesc = AudiocppAsrFamilyDesc {
    model_id: "qwen3-asr-0.6b",
    family: "qwen3_asr",
    gguf_file: "qwen3-asr-0.6b-q8_0.gguf",
    required_files: &["qwen3-asr-0.6b-q8_0.gguf"],
    default_provider: "metal",
    registry_hint: "zapmomo asr install-model --registry-id asr-qwen3-0.6b-audiocpp",
};

/// 按模型类型查表；sherpa-only kind 返回 None（audiocpp 后端不支持该组合）。
pub fn asr_family_desc(kind: AsrModelKind) -> Option<&'static AudiocppAsrFamilyDesc> {
    match kind {
        AsrModelKind::Qwen3Asr => Some(&QWEN3_ASR_06B),
        _ => None,
    }
}

/// 目录内按 GGUF 主文件名探测 audiocpp ASR 族（外部导入/手工放置目录的兜底）。
///
/// 与 sherpa 的 `detect_kind_from_dir`（ONNX 探针）正交：本函数只在 backend
/// 显式 audiocpp 或 `set_selected_model` 外部导入分支介入，不污染 sherpa 探测。
pub fn detect_gguf_in_dir(dir: &Path) -> Option<&'static AudiocppAsrFamilyDesc> {
    if dir.join(QWEN3_ASR_06B.gguf_file).is_file() {
        return Some(&QWEN3_ASR_06B);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 表覆盖锚点：qwen3_asr 可查，sherpa 族不可查。
    #[test]
    fn test_asr_family_desc_coverage() {
        assert_eq!(
            asr_family_desc(AsrModelKind::Qwen3Asr).unwrap().family,
            "qwen3_asr"
        );
        for kind in [
            AsrModelKind::Zipformer,
            AsrModelKind::Paraformer,
            AsrModelKind::SenseVoice,
            AsrModelKind::Whisper,
        ] {
            assert!(
                asr_family_desc(kind).is_none(),
                "{kind:?} 不应有 audiocpp 描述"
            );
        }
    }

    /// qwen3_asr 记录形状：单文件清单 / metal 默认后端 / model_id 与 registry hint 对应。
    #[test]
    fn test_asr_family_record_shape() {
        let q = asr_family_desc(AsrModelKind::Qwen3Asr).unwrap();
        assert_eq!(q.model_id, "qwen3-asr-0.6b");
        assert_eq!(q.required_files, &["qwen3-asr-0.6b-q8_0.gguf"]);
        assert_eq!(q.gguf_file, q.required_files[0], "清单即单 GGUF");
        assert_eq!(q.default_provider, "metal");
        assert!(q.registry_hint.contains("asr-qwen3-0.6b-audiocpp"));
    }

    /// GGUF 探测：命中 / 空目录 / 不存在目录。
    #[test]
    fn test_detect_gguf_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_gguf_in_dir(dir.path()).is_none());
        std::fs::write(dir.path().join(QWEN3_ASR_06B.gguf_file), b"x").unwrap();
        assert_eq!(
            detect_gguf_in_dir(dir.path()).unwrap().model_id,
            "qwen3-asr-0.6b"
        );
        assert!(detect_gguf_in_dir(Path::new("/nonexistent-asr-gguf")).is_none());
    }
}
