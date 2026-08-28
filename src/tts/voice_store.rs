/// 自定义音色库：`~/.zapmomo/voices/` 目录 + `manifest.json`。
///
/// 每个自定义音色 = `<id>.wav`（从上传/录音源拷贝）+ 清单条目
/// `{ id, name, wav, reference_text }`。清单用 JSON（`serde_json` 已是依赖），
/// 与 `models/manifest.json` 的解析方式一致。
///
/// 合成时前端直接把 `wav_path` / `reference_text` 经 `synthesize_tts` 的
/// `reference_wav` / `reference_text` 传入，`resolve_reference` 自定义分支原样使用，
/// 因此本模块只管「存储 + 枚举 + 增删」，不涉及克隆逻辑。
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tts::voice::TtsVoice;

/// 用户音色目录名（相对 `~/.zapmomo/`）。
const VOICES_DIR_NAME: &str = "voices";
/// 音色清单文件名。
const MANIFEST_NAME: &str = "manifest.json";

/// 清单单条：`wav` 为相对音色目录的文件名。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceEntry {
    pub id: String,
    pub name: String,
    pub wav: String,
    pub reference_text: String,
}

/// 音色目录：`~/.zapmomo/voices/`。
pub fn voices_dir() -> std::path::PathBuf {
    crate::config::settings::get_settings_dir().join(VOICES_DIR_NAME)
}

fn manifest_path() -> std::path::PathBuf {
    voices_dir().join(MANIFEST_NAME)
}

/// 读取清单；文件缺失或解析失败返回空（不报错，视为尚无自定义音色）。
fn load_manifest() -> Vec<VoiceEntry> {
    let Ok(content) = std::fs::read_to_string(manifest_path()) else {
        return Vec::new();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

/// 原子写清单：先写临时文件再 rename，避免中断留下半截清单。
fn save_manifest(entries: &[VoiceEntry]) -> Result<(), String> {
    std::fs::create_dir_all(voices_dir()).map_err(|e| format!("创建音色目录失败: {e}"))?;
    let tmp = manifest_path().with_extension("json.tmp");
    let content =
        serde_json::to_string_pretty(entries).map_err(|e| format!("序列化音色清单失败: {e}"))?;
    std::fs::write(&tmp, content).map_err(|e| format!("写入音色清单失败: {e}"))?;
    std::fs::rename(&tmp, manifest_path()).map_err(|e| format!("更新音色清单失败: {e}"))?;
    Ok(())
}

/// 生成唯一音色 id（时间戳 + 进程内自增，保证同毫秒内不冲突）。
fn new_voice_id() -> String {
    static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER
        .get_or_init(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);
    format!("custom-{millis}-{n}")
}

/// 列出全部自定义音色（`custom: true`）。
pub fn list_custom_voices() -> Vec<TtsVoice> {
    let dir = voices_dir();
    load_manifest()
        .into_iter()
        .map(|e| TtsVoice {
            id: e.id,
            name: e.name,
            wav_path: dir.join(&e.wav),
            reference_text: e.reference_text,
            custom: true,
        })
        .collect()
}

/// 保存一个自定义音色：把源 wav 拷贝到音色目录并写入清单。
///
/// 校验：名称非空、转写文本非空、源 wav 存在且是合法 RIFF/WAVE（前 4 字节 `RIFF`）。
pub fn save_voice(
    name: &str,
    source_wav: &std::path::Path,
    reference_text: &str,
) -> Result<TtsVoice, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("音色名称不能为空".to_string());
    }
    let reference_text = reference_text.trim();
    if reference_text.is_empty() {
        return Err("请提供参考音频的逐字转写文本".to_string());
    }
    if !source_wav.is_file() {
        return Err(format!("源音频不存在: {}", source_wav.display()));
    }
    if !is_wav_file(source_wav) {
        return Err(format!("不是有效的 wav 文件: {}", source_wav.display()));
    }

    let id = new_voice_id();
    let dir = voices_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建音色目录失败: {e}"))?;
    let wav_name = format!("{id}.wav");
    let dest = dir.join(&wav_name);
    std::fs::copy(source_wav, &dest).map_err(|e| format!("复制参考音频失败: {e}"))?;

    let mut entries = load_manifest();
    entries.push(VoiceEntry {
        id: id.clone(),
        name: name.to_string(),
        wav: wav_name,
        reference_text: reference_text.to_string(),
    });
    // 清单写入失败时回滚已拷贝的 wav，避免留下孤儿文件
    if let Err(e) = save_manifest(&entries) {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }

    Ok(TtsVoice {
        id,
        name: name.to_string(),
        wav_path: dest,
        reference_text: reference_text.to_string(),
        custom: true,
    })
}

/// 删除自定义音色：移除清单条目 + 删除 wav 文件。
pub fn delete_voice(id: &str) -> Result<(), String> {
    let mut entries = load_manifest();
    let Some(idx) = entries.iter().position(|e| e.id == id) else {
        return Err(format!("未找到音色: {id}"));
    };
    let entry = entries.remove(idx);
    save_manifest(&entries)?;
    let _ = std::fs::remove_file(voices_dir().join(&entry.wav));
    Ok(())
}

/// 校验文件是否为 RIFF/WAVE（读取前 4 字节）。
fn is_wav_file(path: &std::path::Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut head = [0u8; 4];
    f.read_exact(&mut head)
        .map(|_| &head == b"RIFF")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    /// 生成一个合法最小 wav 字节（44 字节头 + 少量样本）。
    fn sample_wav_bytes() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&44u32.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&16000u32.to_le_bytes());
        buf.extend_from_slice(&32000u32.to_le_bytes());
        buf.extend_from_slice(&2u16.to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&8u32.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf.extend_from_slice(&0i16.to_le_bytes());
        buf
    }

    #[test]
    fn test_save_and_list_voice() {
        run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = save_voice("我的声音", &src, "你好世界").unwrap();
            assert!(v.custom);
            assert_eq!(v.name, "我的声音");
            assert_eq!(v.reference_text, "你好世界");
            assert!(v.wav_path.is_file(), "wav 应已拷贝到音色目录");

            let voices = list_custom_voices();
            assert_eq!(voices.len(), 1);
            assert_eq!(voices[0].id, v.id);
            assert_eq!(voices[0].wav_path, v.wav_path);
        });
    }

    #[test]
    fn test_save_voice_validates_input() {
        run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            // 空名称
            assert!(save_voice("  ", &src, "文本").is_err());
            // 空转写文本
            assert!(save_voice("名字", &src, "  ").is_err());
            // 源不存在
            assert!(save_voice("名字", &home.join("nope.wav"), "文本").is_err());
            // 非 wav（缺 RIFF 头）
            std::fs::write(home.join("bad.txt"), b"not a wav").unwrap();
            assert!(save_voice("名字", &home.join("bad.txt"), "文本").is_err());
        });
    }

    #[test]
    fn test_delete_voice() {
        run_with_temp_home(|home| {
            let src = home.join("src.wav");
            std::fs::write(&src, sample_wav_bytes()).unwrap();
            let v = save_voice("删除我", &src, "文本").unwrap();
            assert!(v.wav_path.is_file());

            delete_voice(&v.id).unwrap();
            assert!(list_custom_voices().is_empty());
            assert!(!v.wav_path.is_file(), "wav 文件应被删除");

            // 删除不存在的 id 报错
            assert!(delete_voice("custom-nope").is_err());
        });
    }
}
