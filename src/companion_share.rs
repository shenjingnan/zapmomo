//! 角色包导出/导入 .zip（分享闭环）。
//!
//! 导出：白名单收集源资产（character.md/png、sprites/ 一层、voice/reference.*，
//! 结构性排除 cover.png / voice/welcome.* 等应用派生文件）+ 合成 `character.json`
//! （当前生效的唤醒词/欢迎语预设随包流通）→ 原子写 .zip。
//!
//! 导入：魔数预检 → **确定性解压路径**（temp_dir 下按 zip 路径 hash 命名，保证
//! 同一 zip 的 companion id 稳定、重复导入走 `already_imported`）→ zip-slip /
//! 炸弹防护 + 白名单过滤解压 → 完整复用 `import_character_from_dir` 既有链。
//!
//! zip 根目录为扁平结构（character.md 在根），可直接喂导入判据；解压端宽容
//! 「恰好一层 wrapper 目录」（救 Windows 右键压缩等带外层目录的分享包）。

use crate::companion::{self, CompanionManifest, CompanionModel};
use crate::companion_sprites::{SPRITE_EXTS, SPRITES_DIR};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// 导出结果：目的地路径与打包文件数（character.json 计入）。
pub struct ExportedPack {
    pub dest: PathBuf,
    pub files: u32,
}

/// 解压资源上限（zip 炸弹防护）：条目数 / 解压后总字节数。
const MAX_ENTRIES: usize = 4096;
const MAX_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;

/// 同进程解压串行化：解压目录「清空重建」必须互斥。
/// 跨进程由应用 single-instance 保证；不同 zip 路径的目录本就不同，串行只为防同一 zip 并发。
static PACK_EXTRACT_LOCK: Mutex<()> = Mutex::new(());

// ===========================================================================
// 导出
// ===========================================================================

/// 把角色包伙伴导出为可分享的 .zip。
///
/// 同步阻塞（秒级），调用方（Tauri command）必须 spawn_blocking。
/// 打包内容为白名单源资产；`character.json` 为导出时合成（当前生效预设），
/// 不直接复制托管原件。
pub fn export_pack(id: &str, dest: &Path) -> Result<ExportedPack, String> {
    let model = companion::character_pack_model(id)?;
    let model_dir = PathBuf::from(&model.model_dir);
    reject_managed_dest(dest)?;
    // 预检提前失败（如 voice 不成对），而不是打包完才在导入端炸。
    companion::validate_character_pack(&model_dir)?;

    let manifest_json = build_pack_manifest(&model, &model_dir)?;
    let mut entries = collect_export_entries(&model_dir)?;
    // 按 rel 排序：zip 字节级可复现（利于 roundtrip 测试与未来校验和）。
    entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    let files = write_zip(dest, &entries, &manifest_json)?;
    Ok(ExportedPack {
        dest: dest.to_path_buf(),
        files,
    })
}

/// 打包条目：`rel` 为 zip 内相对路径（`/` 分隔，由组件显式拼接，防 Windows
/// 反斜杠混入 zip），`src` 为本地源文件。
struct PackEntry {
    rel: String,
    src: PathBuf,
}

/// 白名单收集源资产（不含 `character.json`——它由导出端合成）。
fn collect_export_entries(model_dir: &Path) -> Result<Vec<PackEntry>, String> {
    let mut entries = Vec::new();
    // 必需资产。
    for name in [companion::CHARACTER_MD, companion::CHARACTER_PNG] {
        let path = model_dir.join(name);
        if !path.is_file() {
            return Err(format!("角色包缺少 {name}"));
        }
        entries.push(PackEntry {
            rel: name.to_string(),
            src: path,
        });
    }
    // sprites/ 一层图片（对齐运行时枚举规则：扩展名白名单 + 跳过隐藏文件；
    // 不做同 stem 去重——全部带走，去重留给消费端 list_sprites 的优先级逻辑）。
    let sprites_dir = model_dir.join(SPRITES_DIR);
    if sprites_dir.is_dir() {
        let mut names: Vec<String> = std::fs::read_dir(&sprites_dir)
            .map_err(|e| format!("读取 sprites 目录失败: {e}"))?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|file_name| {
                let Some(ext) = Path::new(file_name)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                else {
                    return false;
                };
                SPRITE_EXTS.contains(&ext.as_str())
            })
            .filter(|file_name| {
                Path::new(file_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| !s.is_empty() && !s.starts_with('.'))
            })
            .collect();
        names.sort();
        for file_name in names {
            entries.push(PackEntry {
                rel: format!("{SPRITES_DIR}/{file_name}"),
                src: sprites_dir.join(&file_name),
            });
        }
    }
    // 音色克隆参考（可选，成对性由 validate_character_pack 已校验）。
    for name in [companion::REFERENCE_WAV, companion::REFERENCE_TXT] {
        let path = model_dir.join(companion::VOICE_DIR).join(name);
        if path.is_file() {
            entries.push(PackEntry {
                rel: format!("{}/{name}", companion::VOICE_DIR),
                src: path,
            });
        }
    }
    Ok(entries)
}

/// 导出端合成的 `character.json`（独立于 [`CompanionManifest`]：避免 Serialize
/// 带出 null 字段——预设缺席时应整体省略字段）。
#[derive(Serialize)]
struct PackManifestOut {
    version: u32,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    wake_word: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    welcome_text: Option<String>,
}

/// 合成导出用 `character.json`。
///
/// 唤醒词/欢迎语取「用户自定义（library.json）> 托管原件作者预设」，两者皆无则
/// **整体省略字段**——刻意不写 `effective_*` 的推导兜底值（跟随角色名/默认模板
/// 是导入端行为，写死会焊死「跟随改名」语义）。托管原件损坏在此报错（严格，
/// 与导入端一致：坏预设不该被静默丢弃或静默传播）。
fn build_pack_manifest(model: &CompanionModel, model_dir: &Path) -> Result<Vec<u8>, String> {
    let managed = CompanionManifest::read(model_dir)?;
    let merged = |custom: &Option<String>, preset: &Option<String>| {
        CompanionManifest::preset(custom).or_else(|| CompanionManifest::preset(preset))
    };
    let out = PackManifestOut {
        version: 1,
        name: model.name.clone(),
        wake_word: merged(&model.wake_word, &managed.wake_word),
        welcome_text: merged(&model.welcome_text, &managed.welcome_text),
    };
    serde_json::to_vec_pretty(&out)
        .map_err(|e| format!("序列化 {0} 失败: {e}", companion::CHARACTER_JSON))
}

/// 目的地防线：拒绝落在伙伴托管目录内、或已是目录的路径。
fn reject_managed_dest(dest: &Path) -> Result<(), String> {
    if dest.is_dir() {
        return Err(format!("导出路径已是目录: {}", dest.display()));
    }
    // dest 通常尚不存在，canonicalize 会失败——用「父目录规范值 + 文件名」拼绝对路径。
    let abs = match (dest.parent(), dest.file_name()) {
        (Some(parent), Some(name)) => parent
            .canonicalize()
            .map(|p| p.join(name))
            .unwrap_or_else(|_| dest.to_path_buf()),
        _ => dest.to_path_buf(),
    };
    for root in companion::companion_store_roots() {
        let root_abs = root.canonicalize().unwrap_or(root);
        if abs.starts_with(&root_abs) {
            return Err("导出路径不能位于伙伴托管目录内".to_string());
        }
    }
    Ok(())
}

/// 写 zip：先落 `*.partial` 临时文件，`finish()` 成功后 rename 为正式名；
/// Windows 上 rename 无法覆盖已存在目标，先移除再重试（对齐
/// `convert_reference_to_mono` 既有先例）。任何失败路径删除 partial。
fn write_zip(dest: &Path, entries: &[PackEntry], manifest_json: &[u8]) -> Result<u32, String> {
    let partial = dest.with_extension("zip.partial");
    let count = (|| -> Result<u32, String> {
        let file = File::create(&partial).map_err(|e| format!("创建临时 zip 失败: {e}"))?;
        let mut writer = ZipWriter::new(file);
        // 显式 Deflated：zip 8 的「默认压缩方法」随启用特性回退，必须钉死。
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut count = 0u32;
        for entry in entries {
            writer
                .start_file(&entry.rel, opts)
                .map_err(|e| format!("写入 zip 条目 {} 失败: {e}", entry.rel))?;
            let mut src = File::open(&entry.src)
                .map_err(|e| format!("读取 {} 失败: {e}", entry.src.display()))?;
            io::copy(&mut src, &mut writer)
                .map_err(|e| format!("写入 zip 条目 {} 失败: {e}", entry.rel))?;
            count += 1;
        }
        writer
            .start_file(companion::CHARACTER_JSON, opts)
            .map_err(|e| format!("写入 zip 条目 {} 失败: {e}", companion::CHARACTER_JSON))?;
        io::Write::write_all(&mut writer, manifest_json)
            .map_err(|e| format!("写入 zip 条目 {} 失败: {e}", companion::CHARACTER_JSON))?;
        count += 1;
        writer
            .finish()
            .map_err(|e| format!("完成 zip 写入失败: {e}"))?;
        Ok(count)
    })();
    if let Err(e) = count {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    if dest.exists()
        && let Err(e) = std::fs::remove_file(dest)
    {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("移除已有导出文件失败: {e}"));
    }
    std::fs::rename(&partial, dest).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("提交导出文件失败: {e}")
    })?;
    Ok(count.unwrap_or_default())
}

// ===========================================================================
// 导入
// ===========================================================================

/// 从 .zip 导入角色包（同步阻塞，调用方必须 spawn_blocking）。
///
/// 同一 zip 文件重复导入 → `already_imported = true`（确定性解压路径 ⇒
/// `derive_id` 稳定），不会产生重复伙伴。
pub fn import_zip(zip_path: &Path) -> Result<(CompanionModel, bool), String> {
    let zip_abs = zip_path
        .canonicalize()
        .map_err(|e| format!("无法访问压缩包: {e}"))?;
    if !zip_abs.is_file() {
        return Err(format!("源路径不是文件: {}", zip_abs.display()));
    }
    // 魔数预检：比「未找到 character.md」友好得多的第一道报错。
    let mut magic = [0u8; 4];
    File::open(&zip_abs)
        .and_then(|mut f| io::Read::read_exact(&mut f, &mut magic))
        .map_err(|e| format!("读取压缩包失败: {e}"))?;
    if &magic != b"PK\x03\x04" {
        return Err("不是合法的 zip 压缩包".to_string());
    }

    let target = extract_root_for_zip(&zip_abs);
    let _g = PACK_EXTRACT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let result = (|| -> Result<(CompanionModel, bool), String> {
        // 清空重建：确定性路径的复用语义（上次导入的解压残留不影响本次）。
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir_all(&target).map_err(|e| format!("创建解压目录失败: {e}"))?;
        let file = File::open(&zip_abs).map_err(|e| format!("打开压缩包失败: {e}"))?;
        let mut archive = ZipArchive::new(file).map_err(|e| format!("无法读取 zip: {e}"))?;
        extract_pack(&mut archive, &target)?;
        let root = resolve_pack_root(&target)?;
        companion::import_character_from_dir(&root)
    })();
    match result {
        Ok((model, already)) => Ok((model, already)),
        Err(e) => {
            // 不留半成品解压目录（下次同 zip 导入也会先清空重建，此处只是卫生）。
            let _ = std::fs::remove_dir_all(&target);
            Err(e)
        }
    }
}

/// zip → 确定性解压目录：`temp_dir/zapmomo-pack-{sha256(zip 绝对路径)[..12]}`。
///
/// `derive_id` 对 canonical 源路径做 sha256，因此同一 zip ⇒ 同一解压目录 ⇒
/// 同一 companion id（防随机 temp 路径导致的重复导入）。放系统 temp 而非托管
/// store：不污染用户可见的伙伴资产目录，也不与 `.tmp-` 清理扫描交互。
fn extract_root_for_zip(zip_abs: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(zip_abs.to_string_lossy().as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::env::temp_dir().join(format!("zapmomo-pack-{}", &hex[..12]))
}

/// 解压白名单：归一路径命中源资产才落盘（返回 Some(rel)）。
///
/// 白名单之外（`__MACOSX/`、`.DS_Store`、别人的 cover.png / welcome.wav、
/// 一切未知文件）直接跳过——`copy_dir_recursive` 不过滤文件，解压端白名单是
/// 托管目录不被污染的结构性保证。
fn classify_entry(norm: &str) -> Option<String> {
    match norm {
        companion::CHARACTER_MD | companion::CHARACTER_PNG | companion::CHARACTER_JSON => {
            Some(norm.to_string())
        }
        _ => {
            let (head, rest) = norm.split_once('/')?;
            match head {
                companion::VOICE_DIR => match rest {
                    companion::REFERENCE_WAV | companion::REFERENCE_TXT => Some(norm.to_string()),
                    _ => None,
                },
                SPRITES_DIR => {
                    // 一层：文件名内不得再含分隔符。
                    if rest.contains('/') || rest.is_empty() {
                        return None;
                    }
                    let stem = Path::new(rest).file_stem()?.to_str()?;
                    if stem.is_empty() || stem.starts_with('.') {
                        return None;
                    }
                    let ext = Path::new(rest).extension()?.to_str()?.to_ascii_lowercase();
                    SPRITE_EXTS
                        .contains(&ext.as_str())
                        .then(|| norm.to_string())
                }
                _ => None,
            }
        }
    }
}

/// 检测 wrapper 目录前缀：根直含 character.md → None；否则「恰好一个」顶层
/// 子目录含 character.md → `Some("子目录/")`（0/多个 → None，交给
/// `resolve_pack_root` 报错）。白名单匹配的是剥离前缀后的内层路径。
fn detect_wrapper_prefix<R: io::Read + io::Seek>(archive: &mut ZipArchive<R>) -> Option<String> {
    let mut wrapper: Option<String> = None;
    for i in 0..archive.len() {
        let name = archive.by_index(i).ok()?.name().replace('\\', "/");
        if name == companion::CHARACTER_MD {
            return None;
        }
        if let Some((head, rest)) = name.split_once('/')
            && rest == companion::CHARACTER_MD
        {
            if wrapper.is_some() {
                return None;
            }
            wrapper = Some(format!("{head}/"));
        }
    }
    wrapper
}

/// 解压到 `dest`（zip-slip / 炸弹防护收拢在此）。
fn extract_pack<R: io::Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    dest: &Path,
) -> Result<(), String> {
    if archive.len() > MAX_ENTRIES {
        return Err(format!("压缩包条目过多（上限 {MAX_ENTRIES}）"));
    }
    let prefix = detect_wrapper_prefix(archive);
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目失败: {e}"))?;
        if file.is_dir() {
            continue;
        }
        // 先归一分隔符再匹配白名单：Windows 工具打出的 zip 条目用 `\`。
        // wrapper 前缀剥离后再匹配——包内资产以扁平 rel 落盘。
        let norm = file.name().replace('\\', "/");
        let inner = prefix
            .as_deref()
            .and_then(|p| norm.strip_prefix(p))
            .unwrap_or(&norm);
        let Some(rel) = classify_entry(inner) else {
            continue;
        };
        // zip-slip：enclosed_name 拒绝越界/绝对路径；目标路径由白名单 rel 自拼，
        // 绝不使用 mangled_name。
        if file.enclosed_name().is_none() {
            return Err(format!("压缩包内含不安全路径: {}", file.name()));
        }
        total += file.size();
        if total > MAX_UNCOMPRESSED_BYTES {
            return Err("压缩包解压后过大".to_string());
        }
        let target = dest.join(&rel);
        if let Some(parent) = target.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(format!("创建解压子目录失败: {e}"));
        }
        let mut out = File::create(&target).map_err(|e| format!("写入解压文件失败: {e}"))?;
        io::copy(&mut file, &mut out).map_err(|e| format!("解压 {} 失败: {e}", rel))?;
    }
    Ok(())
}

/// 定位包内角色包根：根目录直含 character.md 优先；否则取「恰好一个」含
/// character.md 的一层子目录（宽容 wrapper 目录），0 个 / 多个各自报错。
fn resolve_pack_root(dir: &Path) -> Result<PathBuf, String> {
    if dir.join(companion::CHARACTER_MD).is_file() {
        return Ok(dir.to_path_buf());
    }
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(dir)
        .map_err(|e| format!("读取解压目录失败: {e}"))?
        .flatten()
    {
        let sub = entry.path();
        if sub.is_dir() && sub.join(companion::CHARACTER_MD).is_file() {
            candidates.push(sub);
        }
    }
    match candidates.len() {
        1 => Ok(candidates.pop().unwrap_or_default()),
        0 => Err("压缩包中未找到 character.md（不是 ZapMomo 角色包）".to_string()),
        _ => Err("压缩包含多个角色包，请解压后分别导入".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;
    use std::io::Write;

    /// 构造最小合法角色包源目录（含 character.md/png 与可选 voice/sprites/json）。
    fn make_pack(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(companion::CHARACTER_MD), format!("# {name}\n")).unwrap();
        std::fs::write(dir.join(companion::CHARACTER_PNG), b"\x89PNG\r\n\x1a\n png").unwrap();
    }

    /// 构造最小合法 wav（RIFF 头 + 少量样本），供 voice/ 成对资产用。
    fn make_wav(path: &Path) {
        crate::audio::write_wav_f32(path, 16_000, &[0.1; 160]).unwrap();
    }

    /// 测试内构造任意条目的 zip（含白名单外/恶意条目）。
    fn write_test_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }

    /// 读 zip 内全部条目名集合。
    fn zip_names(path: &Path) -> Vec<String> {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn test_export_whitelist_excludes_derived() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "芙宁娜");
            std::fs::create_dir_all(src.join("sprites")).unwrap();
            std::fs::write(src.join("sprites/happy.png"), b"png").unwrap();
            std::fs::write(src.join("sprites/.hidden.png"), b"x").unwrap();
            std::fs::write(src.join("sprites/sad.psd"), b"x").unwrap();
            std::fs::create_dir_all(src.join("voice")).unwrap();
            make_wav(&src.join("voice/reference.wav"));
            std::fs::write(src.join("voice/reference.txt"), "哼~").unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();

            let model = companion::active_model_fast().unwrap();
            let dir = PathBuf::from(&model.model_dir);
            // 塞入应用派生文件与垃圾 → 导出必须结构性排除。
            std::fs::write(dir.join("cover.png"), b"derived").unwrap();
            std::fs::write(dir.join("voice/welcome.wav"), b"derived").unwrap();
            std::fs::write(dir.join("voice/welcome.json"), b"{}").unwrap();
            std::fs::write(dir.join(".DS_Store"), b"junk").unwrap();
            std::fs::write(dir.join("notes.txt"), b"junk").unwrap();

            let dest = home.join("furina.zip");
            let exported = export_pack(&model.id, &dest).unwrap();
            // zip_names 已排序：字节序 j < m < p（character.json < character.md < character.png）。
            assert_eq!(
                zip_names(&dest),
                vec![
                    "character.json".to_string(),
                    "character.md".to_string(),
                    "character.png".to_string(),
                    "sprites/happy.png".to_string(),
                    "voice/reference.txt".to_string(),
                    "voice/reference.wav".to_string(),
                ]
            );
            assert_eq!(exported.files, 6);
        });
    }

    #[test]
    fn test_export_manifest_merge_priority() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "错误的H1");
            // 托管原件带作者预设。
            std::fs::write(
                src.join(companion::CHARACTER_JSON),
                r#"{"version":1,"name":"芙宁娜","wake_word":"水神","welcome_text":"哼~"}"#,
            )
            .unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();
            let dest = home.join("out.zip");

            // ① 用户未自定义 → 保留预设。
            export_pack(&model.id, &dest).unwrap();
            let json = read_zip_entry(&dest, companion::CHARACTER_JSON);
            assert!(json.contains("水神"));
            assert!(json.contains("哼~"));
            assert!(json.contains("\"name\": \"芙宁娜\""));

            // ② 用户自定义 → 覆盖预设。
            companion::set_wake_word(&model.id, Some("芙芙")).unwrap();
            export_pack(&model.id, &dest).unwrap();
            let json = read_zip_entry(&dest, companion::CHARACTER_JSON);
            assert!(json.contains("芙芙"));
            assert!(!json.contains("水神"));

            // ③ name 恒为当前显示名（rename 后跟随）。
            companion::rename(&model.id, "芙宁娜·改").unwrap();
            export_pack(&model.id, &dest).unwrap();
            let json = read_zip_entry(&dest, companion::CHARACTER_JSON);
            assert!(json.contains("芙宁娜·改"));
        });
    }

    #[test]
    fn test_export_manifest_absent_fields_when_no_preset() {
        run_with_temp_home(|home| {
            let src = home.join("plain");
            make_pack(&src, "素包");
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();
            let dest = home.join("out.zip");
            export_pack(&model.id, &dest).unwrap();
            let json = read_zip_entry(&dest, companion::CHARACTER_JSON);
            assert!(json.contains("\"version\": 1"));
            assert!(!json.contains("wake_word"), "无预设时字段必须整体缺席");
            assert!(!json.contains("welcome_text"));
        });
    }

    #[test]
    fn test_export_manifest_invalid_json_fails() {
        run_with_temp_home(|home| {
            let src = home.join("bad");
            make_pack(&src, "坏声明");
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();
            // 导入后把托管副本的声明改坏 → 导出报错（严格，不静默）。
            std::fs::write(
                Path::new(&model.model_dir).join(companion::CHARACTER_JSON),
                "{not json",
            )
            .unwrap();
            assert!(export_pack(&model.id, &home.join("out.zip")).is_err());
        });
    }

    #[test]
    fn test_export_rejects_non_character_and_bad_dest() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "芙宁娜");
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();

            // dest 落在托管目录内 → 拒绝。
            let inside = Path::new(&model.model_dir).join("out.zip");
            assert!(export_pack(&model.id, &inside).is_err());
            // dest 已是目录 → 拒绝。
            let dir_dest = home.join("adir");
            std::fs::create_dir_all(&dir_dest).unwrap();
            assert!(export_pack(&model.id, &dir_dest).is_err());
        });
    }

    #[test]
    fn test_zip_roundtrip_deterministic_and_already_imported() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "芙宁娜");
            std::fs::write(
                src.join(companion::CHARACTER_JSON),
                r#"{"version":1,"wake_word":"水神","welcome_text":"哼~"}"#,
            )
            .unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();
            companion::set_wake_word(&model.id, Some("芙芙")).unwrap();

            let dest = home.join("share.zip");
            export_pack(&model.id, &dest).unwrap();

            // 第一次 zip 导入（源目录 id 与 zip 导入 id 天然不同——源不同语义不变）。
            let (first, already) = import_zip(&dest).unwrap();
            assert!(!already);
            let lib = companion::load_library_fast().unwrap();
            assert_eq!(lib.models.len(), 2, "目录导入 + zip 导入各一");

            // 删除 zip 导入的伙伴 → 同一 zip 再导入 → 同 id（确定性解压路径）。
            companion::remove(&first.id).unwrap();
            let (again, already2) = import_zip(&dest).unwrap();
            assert_eq!(again.id, first.id, "同 zip 必须 id 稳定");
            assert!(!already2);

            // 不删除第三次导入 → already_imported，列表不增。
            let (third, already3) = import_zip(&dest).unwrap();
            assert_eq!(third.id, first.id);
            assert!(already3);
            let lib = companion::load_library_fast().unwrap();
            assert_eq!(lib.models.len(), 2);
        });
    }

    #[test]
    fn test_zip_import_preserves_presets_in_new_home() {
        // 用「另一个 home」模拟导入者：预设必须随包还原（A 层流通验证）。
        let zip_holder = std::env::temp_dir().join(format!(
            "zapmomo-share-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&zip_holder).unwrap();
        let zip_path = zip_holder.join("share.zip");

        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "芙宁娜");
            std::fs::write(
                src.join(companion::CHARACTER_JSON),
                r#"{"version":1,"wake_word":"水神","welcome_text":"哼~没错，就是我。"}"#,
            )
            .unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();
            export_pack(&model.id, &zip_path).unwrap();
        });

        run_with_temp_home(|_home| {
            let (model, already) = import_zip(&zip_path).unwrap();
            assert!(!already);
            assert_eq!(model.name, "芙宁娜");
            assert_eq!(model.wake_word.as_deref(), Some("水神"));
            assert_eq!(model.welcome_text.as_deref(), Some("哼~没错，就是我。"));
        });
        let _ = std::fs::remove_dir_all(&zip_holder);
    }

    #[test]
    fn test_import_zip_rejects_slip_and_junk_only() {
        run_with_temp_home(|_home| {
            let zip_path = std::env::temp_dir().join("zapmomo-slip-test.zip");
            // zip-slip 条目 → 拒绝。
            write_test_zip(
                &zip_path,
                &[
                    (companion::CHARACTER_MD, b"# x\n".as_slice()),
                    ("../evil.txt", b"evil".as_slice()),
                ],
            );
            assert!(import_zip(&zip_path).is_err());

            // 只有杂物的包 → 明确报「未找到 character.md」。
            write_test_zip(
                &zip_path,
                &[
                    ("__MACOSX/x", b"".as_slice()),
                    (".DS_Store", b"".as_slice()),
                    ("cover.png", b"".as_slice()),
                ],
            );
            let err = import_zip(&zip_path).unwrap_err();
            assert!(err.contains("character.md"), "实际错误: {err}");

            let _ = std::fs::remove_file(&zip_path);
        });
    }

    #[test]
    fn test_import_zip_accepts_wrapper_dir_and_filters_junk() {
        run_with_temp_home(|home| {
            let zip_path = home.join("wrapped.zip");
            // wrapper 目录 + 混入杂物：白名单过滤后应成功导入干净内容。
            write_test_zip(
                &zip_path,
                &[
                    ("__MACOSX/junk", b"".as_slice()),
                    ("MyChar/.DS_Store", b"".as_slice()),
                    ("MyChar/cover.png", b"junk".as_slice()),
                    ("MyChar/character.md", "# 包装角色\n".as_bytes()),
                    ("MyChar/character.png", b"\x89PNG\r\n\x1a\n png".as_slice()),
                    (
                        "MyChar/character.json",
                        r#"{"version":1,"name":"包装角色"}"#.as_bytes(),
                    ),
                ],
            );
            let (model, already) = import_zip(&zip_path).unwrap();
            assert!(!already);
            assert_eq!(model.name, "包装角色");
            // 杂物不得进入托管目录。
            let dir = PathBuf::from(&model.model_dir);
            assert!(!dir.join("cover.png").exists());
            assert!(!dir.join(".DS_Store").exists());
            // 反斜杠分隔的条目名（Windows 工具风格）归一后命中白名单。
            let bs_zip = home.join("backslash.zip");
            write_test_zip(
                &bs_zip,
                &[
                    ("character.md", "# 反斜杠\n".as_bytes()),
                    ("character.png", b"\x89PNG\r\n\x1a\n png".as_slice()),
                ],
            );
            let (m2, _) = import_zip(&bs_zip).unwrap();
            assert_eq!(m2.name, "反斜杠");
        });
    }

    #[test]
    fn test_import_zip_rejects_non_zip() {
        run_with_temp_home(|home| {
            let fake = home.join("fake.zip");
            std::fs::write(&fake, b"this is not a zip file at all").unwrap();
            let err = import_zip(&fake).unwrap_err();
            assert!(err.contains("zip"), "实际错误: {err}");
        });
    }

    #[test]
    fn test_export_excludes_original_backup_and_carries_current_voice() {
        // zip 放系统 temp：run_with_temp_home 有全局锁，**不可嵌套**，两个 home 顺序执行。
        let zip_path = std::env::temp_dir().join(format!(
            "share-voice-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));

        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_pack(&src, "芙宁娜");
            std::fs::create_dir_all(src.join("voice")).unwrap();
            make_wav(&src.join("voice/reference.wav"));
            std::fs::write(src.join("voice/reference.txt"), "作者转写").unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();
            let model = companion::active_model_fast().unwrap();

            // 用户上传自定义音色 → 导出应带「当前生效」版本，备份不进包。
            let up = home.join("custom.wav");
            make_wav(&up);
            crate::companion::upload_companion_voice(&model.id, &up, "自定义转写").unwrap();
            let original_txt = Path::new(&model.model_dir)
                .join(companion::VOICE_DIR)
                .join(companion::REFERENCE_ORIGINAL_TXT);
            std::fs::write(original_txt, "作者转写（备份）").unwrap();

            export_pack(&model.id, &zip_path).unwrap();
            let names = zip_names(&zip_path);
            assert!(names.contains(&"voice/reference.wav".to_string()));
            assert!(names.contains(&"voice/reference.txt".to_string()));
            assert!(
                !names.iter().any(|n| n.contains("original")),
                "备份不得进分享包: {names:?}"
            );
        });

        run_with_temp_home(|_home2| {
            let (imported, _) = import_zip(&zip_path).unwrap();
            assert_eq!(imported.name, "芙宁娜");
            let (voice, _) = companion::companion_voice_in(&imported).unwrap();
            assert_eq!(voice.text, "自定义转写", "接收者拿到分享者调好的音色");
            assert!(!companion::has_original_voice(&imported), "备份不随包");
        });
        let _ = std::fs::remove_file(zip_path);
    }

    #[test]
    fn test_classify_entry_rejects_original_names() {
        // 白名单精确名匹配：备份命名永远不得进解压白名单（与常量绑定防漂移）。
        assert_eq!(
            classify_entry(&format!("voice/{}", companion::REFERENCE_ORIGINAL_WAV)),
            None
        );
        assert_eq!(
            classify_entry(&format!("voice/{}", companion::REFERENCE_ORIGINAL_TXT)),
            None
        );
        // 正名仍命中。
        assert_eq!(
            classify_entry(&format!("voice/{}", companion::REFERENCE_WAV)),
            Some(format!("voice/{}", companion::REFERENCE_WAV))
        );
    }

    /// 读 zip 内指定条目的文本（manifest 断言用）。
    fn read_zip_entry(zip_path: &Path, name: &str) -> String {
        let file = File::open(zip_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut out = String::new();
        io::Read::read_to_string(&mut entry, &mut out).unwrap();
        out
    }
}
