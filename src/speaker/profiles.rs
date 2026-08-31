//! 声纹档案（Speaker Profile）持久化。
//!
//! 每个说话人一个 JSON 文件：`~/.zapmomo/speaker_profiles/<speaker_id>.json`，
//! 保存多段**原始 embedding**（对应 sherpa `SpeakerEmbeddingManager::add_list`
//! 语义：检索时对同一说话人取最大相似度，比存平均向量对多场次/多麦克风更鲁棒）。
//! 本文件只管数据与磁盘，不接触 sherpa 类型。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::settings::get_settings_dir;

/// 声纹档案目录名（`~/.zapmomo/` 下，与 `voices/` 等用户数据同层）。
const PROFILES_DIR_NAME: &str = "speaker_profiles";

/// 档案格式版本（字段演进时递增；读取时只接受当前版本）。
pub const PROFILE_VERSION: u32 = 1;

/// speaker_id 最大长度（同时约束文件名长度）。
pub const SPEAKER_ID_MAX_LEN: usize = 64;

/// 单条注册样本（一段语音的 embedding 及其元数据）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProfileSample {
    pub embedding: Vec<f32>,
    pub duration_ms: f64,
    /// 注册时间（RFC3339）
    pub enrolled_at: String,
}

/// 单个说话人的声纹档案。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerProfile {
    pub version: u32,
    pub speaker_id: String,
    /// 生成 embedding 的模型文件名（加载时做跨模型守卫）
    pub model: String,
    /// embedding 维度
    pub dim: usize,
    pub samples: Vec<ProfileSample>,
    /// 最近更新时间（RFC3339）
    pub updated_at: String,
}

/// speaker_id 合法性校验：仅允许 `[A-Za-z0-9_-]`、不以 `.` 开头、非空、长度受限。
///
/// 同时防住路径穿越（`../x`）、隐藏文件（`.owner`）与文件系统保留字符；
/// 中文 id 一律拒绝并提示改用合法字符（保持跨平台文件名安全）。
pub fn validate_speaker_id(speaker_id: &str) -> Result<(), String> {
    if speaker_id.is_empty() {
        return Err("speaker_id 不能为空".to_string());
    }
    if speaker_id.len() > SPEAKER_ID_MAX_LEN {
        return Err(format!("speaker_id 过长（最多 {SPEAKER_ID_MAX_LEN} 字符）"));
    }
    if speaker_id.starts_with('.') {
        return Err("speaker_id 不能以 . 开头".to_string());
    }
    if !speaker_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("speaker_id 只能包含英文字母、数字、下划线（_）和连字符（-）".to_string());
    }
    Ok(())
}

/// 声纹档案目录：`~/.zapmomo/speaker_profiles`。
pub fn profiles_dir() -> PathBuf {
    get_settings_dir().join(PROFILES_DIR_NAME)
}

/// 某个说话人的档案文件路径。
pub fn profile_path(speaker_id: &str) -> PathBuf {
    profiles_dir().join(format!("{speaker_id}.json"))
}

/// 保存档案（覆盖写；临时文件 + rename 原子替换，与 `save_settings` 同模式）。
pub fn save(profile: &SpeakerProfile) -> Result<(), String> {
    validate_speaker_id(&profile.speaker_id)?;
    let path = profile_path(&profile.speaker_id);
    let dir = profiles_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建声纹档案目录失败: {e}"))?;
    let content =
        serde_json::to_string_pretty(profile).map_err(|e| format!("序列化声纹档案失败: {e}"))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        profile.speaker_id,
        std::process::id()
    ));
    std::fs::write(&tmp, content).map_err(|e| format!("写入临时声纹档案失败: {e}"))?;
    let renamed = match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows：目标存在时 rename 可能失败，先移除再重试
            if path.exists() {
                std::fs::remove_file(&path).map_err(|e| format!("移除旧声纹档案失败: {e}"))?;
            }
            std::fs::rename(&tmp, &path).map_err(|e| format!("替换声纹档案失败: {e}"))
        }
    };
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
}

/// 读取档案；文件不存在返回 `Ok(None)`，损坏返回 `Err`。
pub fn load(speaker_id: &str) -> Result<Option<SpeakerProfile>, String> {
    let path = profile_path(speaker_id);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取声纹档案 {} 失败: {e}", path.display())),
    };
    let profile: SpeakerProfile = serde_json::from_str(&content)
        .map_err(|e| format!("声纹档案 {} 已损坏: {e}", path.display()))?;
    Ok(Some(profile))
}

/// 列出全部档案（按 speaker_id 排序；损坏文件跳过并 warn）。
pub fn list() -> Result<Vec<SpeakerProfile>, String> {
    let dir = profiles_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("扫描声纹档案目录失败: {e}"))?;
    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|c| serde_json::from_str::<SpeakerProfile>(&c).map_err(|e| e.to_string()))
        {
            Ok(profile) => profiles.push(profile),
            Err(e) => {
                tracing::warn!("跳过无法解析的声纹档案 {}: {e}", path.display());
            }
        }
    }
    profiles.sort_by(|a, b| a.speaker_id.cmp(&b.speaker_id));
    Ok(profiles)
}

/// 删除档案；返回是否确实删除了文件。
pub fn delete(speaker_id: &str) -> Result<bool, String> {
    validate_speaker_id(speaker_id)?;
    let path = profile_path(speaker_id);
    if !path.is_file() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| format!("删除声纹档案失败: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    fn sample_profile(speaker_id: &str) -> SpeakerProfile {
        SpeakerProfile {
            version: PROFILE_VERSION,
            speaker_id: speaker_id.to_string(),
            model: "3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx".to_string(),
            dim: 192,
            samples: vec![ProfileSample {
                embedding: vec![0.1_f32; 192],
                duration_ms: 3200.0,
                enrolled_at: "2026-08-29T00:00:00+08:00".to_string(),
            }],
            updated_at: "2026-08-29T00:00:00+08:00".to_string(),
        }
    }

    // ---- validate_speaker_id ----

    #[test]
    fn test_validate_speaker_id_accepts_safe_names() {
        for id in ["owner", "user_1", "A-b_9", "a"] {
            assert!(validate_speaker_id(id).is_ok(), "应接受 {id}");
        }
    }

    #[test]
    fn test_validate_speaker_id_rejects_unsafe_names() {
        for id in [
            "", "../evil", "own er", "张三", ".hidden", "a/b", "a\\b", "a:b",
        ] {
            assert!(validate_speaker_id(id).is_err(), "应拒绝 {id}");
        }
    }

    #[test]
    fn test_validate_speaker_id_rejects_too_long() {
        let long = "a".repeat(SPEAKER_ID_MAX_LEN + 1);
        assert!(validate_speaker_id(&long).is_err());
        let ok = "a".repeat(SPEAKER_ID_MAX_LEN);
        assert!(validate_speaker_id(&ok).is_ok());
    }

    // ---- save / load / list / delete ----

    #[test]
    fn test_save_and_load_roundtrip() {
        run_with_temp_home(|home| {
            let profile = sample_profile("owner");
            save(&profile).unwrap();
            assert_eq!(
                profile_path("owner"),
                home.join(".zapmomo/speaker_profiles/owner.json")
            );
            let loaded = load("owner").unwrap().unwrap();
            assert_eq!(loaded, profile);
        });
    }

    #[test]
    fn test_load_missing_returns_none() {
        run_with_temp_home(|_home| {
            assert!(load("nobody").unwrap().is_none());
        });
    }

    #[test]
    fn test_load_corrupted_returns_err() {
        run_with_temp_home(|_home| {
            std::fs::create_dir_all(profiles_dir()).unwrap();
            std::fs::write(profile_path("broken"), "not json").unwrap();
            assert!(load("broken").is_err());
        });
    }

    #[test]
    fn test_list_sorted_and_skips_corrupted() {
        run_with_temp_home(|_home| {
            std::fs::create_dir_all(profiles_dir()).unwrap();
            save(&sample_profile("user_2")).unwrap();
            save(&sample_profile("owner")).unwrap();
            std::fs::write(profile_path("broken"), "not json").unwrap();
            // 非 .json 文件不参与
            std::fs::write(profiles_dir().join("notes.txt"), "x").unwrap();
            let listed = list().unwrap();
            let ids: Vec<&str> = listed.iter().map(|p| p.speaker_id.as_str()).collect();
            assert_eq!(ids, vec!["owner", "user_2"]);
        });
    }

    #[test]
    fn test_delete_removes_file_once() {
        run_with_temp_home(|_home| {
            save(&sample_profile("owner")).unwrap();
            assert!(delete("owner").unwrap());
            assert!(!delete("owner").unwrap());
            assert!(load("owner").unwrap().is_none());
        });
    }

    #[test]
    fn test_save_overwrites_previous() {
        run_with_temp_home(|_home| {
            let mut profile = sample_profile("owner");
            save(&profile).unwrap();
            profile.samples.push(ProfileSample {
                embedding: vec![0.2_f32; 192],
                duration_ms: 2800.0,
                enrolled_at: "2026-08-29T01:00:00+08:00".to_string(),
            });
            save(&profile).unwrap();
            let loaded = load("owner").unwrap().unwrap();
            assert_eq!(loaded.samples.len(), 2);
        });
    }

    #[test]
    fn test_save_rejects_unsafe_id() {
        run_with_temp_home(|_home| {
            assert!(save(&sample_profile("../evil")).is_err());
            // 未创建任何越界文件
            assert!(!std::fs::exists(profiles_dir().join("..")).unwrap());
        });
    }
}
