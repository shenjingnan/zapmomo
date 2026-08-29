//! 存储信息与迁移引擎（自定义数据目录）。
//!
//! 纯 core（不依赖 Tauri）：提供存储信息查询、目标目录校验、迁移规划与执行。
//! 迁移粒度 = 旧根下顶层条目（models 每个非 `.install` 目录 / companions 每个 `companion-*` 目录），
//! 每条目原子提交（同卷 rename / 跨卷 staging copy），引用改写随条目提交，
//! 崩溃恢复 = 重跑即续（无 journal）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::config::settings;
use crate::model_library;

// ---------------------------------------------------------------------------
// 视图结构（序列化给前端）
// ---------------------------------------------------------------------------

/// 存储信息视图（`get_storage_info` 返回）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfoView {
    /// 已解析的 data_dir 展示值（`None` = 使用默认 `~/.zapmomo`）。
    pub data_dir: Option<String>,
    /// 模型根目录（当前生效）。
    pub models_dir: String,
    /// 伙伴载荷根目录（当前生效）。
    pub companions_dir: String,
    /// 旧默认模型根（data_dir 设置后存在存量时返回，否则 `None`）。
    pub legacy_models_dir: Option<String>,
    /// 旧默认伙伴载荷根。
    pub legacy_companions_dir: Option<String>,
    /// 旧根模型占用字节。
    pub legacy_models_bytes: u64,
    /// 旧根伙伴占用字节。
    pub legacy_companions_bytes: u64,
    /// 是否有可迁移的存量。
    pub migration_available: bool,
    /// 迁移是否进行中（由命令层填充）。
    pub migrating: bool,
    /// 新旧根是否同卷（同卷迁移走瞬时 rename）。
    pub same_volume: bool,
    /// 新根所在卷总空间 / 可用空间。
    pub disk_total: u64,
    pub disk_available: u64,
}

/// 迁移进度事件载荷（`storage-migrate-progress`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrateProgress {
    /// scanning | moving | finishing | done | cancelled | failed
    pub state: String,
    /// 当前条目名（moving 阶段）。
    pub current_item: Option<String>,
    /// 已处理条目数 / 总条目数。
    pub items_done: usize,
    pub items_total: usize,
    /// 已复制字节 / 总字节（跨卷 copy 时递增；同卷 rename 快速推进）。
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
    /// 单条目失败的汇总（不中断整体）。
    pub failed_items: Vec<MigrateFailedItem>,
}

/// 单条目失败信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateFailedItem {
    pub name: String,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// 迁移内部结构
// ---------------------------------------------------------------------------

/// 迁移条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateKind {
    Model,
    Companion,
}

/// 单条迁移条目。
#[derive(Debug, Clone)]
pub struct MigrateItem {
    pub kind: MigrateKind,
    pub name: String,
    pub source: PathBuf,
    pub dest: PathBuf,
    pub bytes: u64,
}

/// 迁移结果。
#[derive(Debug, Clone, Default)]
pub struct MigrateOutcome {
    pub moved: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// 信息收集 / 校验
// ---------------------------------------------------------------------------

/// 目录递归大小（字节）。
pub fn dir_size(p: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        let Ok(meta) = std::fs::symlink_metadata(p) else {
            return 0;
        };
        if meta.is_file() {
            return meta.len();
        }
        if !meta.is_dir() {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(p) else {
            return 0;
        };
        entries.flatten().map(|e| walk(&e.path())).sum()
    }
    walk(p)
}

/// 收集存储信息（应在 `spawn_blocking` 中调用——旧根可能很大）。
pub fn collect_storage_info() -> Result<StorageInfoView, String> {
    let data_dir = settings::get_data_dir().map(|p| p.display().to_string());
    let models_dir = settings::get_models_dir();
    let companions_dir = settings::get_companions_store_dir();
    let legacy_models = settings::legacy_models_dir();
    let legacy_companions = settings::legacy_companions_dir();

    let legacy_models_bytes = legacy_models.as_deref().map(dir_size).unwrap_or(0);
    let legacy_companions_bytes = legacy_companions.as_deref().map(dir_size).unwrap_or(0);

    let same_volume = legacy_models
        .as_deref()
        .is_some_and(|l| same_volume(l, &models_dir));

    let (disk_total, disk_available) = disk_space(&models_dir);

    Ok(StorageInfoView {
        data_dir,
        models_dir: models_dir.display().to_string(),
        companions_dir: companions_dir.display().to_string(),
        legacy_models_dir: legacy_models.map(|p| p.display().to_string()),
        legacy_companions_dir: legacy_companions.map(|p| p.display().to_string()),
        legacy_models_bytes,
        legacy_companions_bytes,
        migration_available: legacy_models_bytes > 0 || legacy_companions_bytes > 0,
        migrating: false,
        same_volume,
        disk_total,
        disk_available,
    })
}

/// 目标卷总/可用空间（复用 sysinfo 的挂载点匹配逻辑）。
fn disk_space(dir: &Path) -> (u64, u64) {
    let mut best_len: Option<u64> = None;
    let mut total = 0u64;
    let mut available = 0u64;
    for d in sysinfo::Disks::new_with_refreshed_list().iter() {
        let mount = d.mount_point();
        if dir.starts_with(mount) {
            let len = mount.as_os_str().to_string_lossy().len() as u64;
            if best_len.is_none_or(|b| len > b) {
                best_len = Some(len);
                total = d.total_space();
                available = d.available_space();
            }
        }
    }
    (total, available)
}

/// 两个路径是否同卷（Windows 比较根前缀/盘符；其它平台视为同卷）。
fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::path::Component;
        let a_root = a
            .components()
            .find_map(|c| match c {
                Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let b_root = b
            .components()
            .find_map(|c| match c {
                Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
                _ => None,
            })
            .unwrap_or_default();
        a_root.to_lowercase() == b_root.to_lowercase()
    }
    #[cfg(not(windows))]
    {
        let _ = (a, b);
        true
    }
}

/// 校验目标 data_dir 是否合法（绝对路径、可创建、可写、不与模型根互含）。
///
/// 返回规范化后的绝对路径。`path` 为 `None`/空串时语义为「重置默认」，由调用方短路，
/// 本函数只处理 `Some`。
pub fn validate_data_dir(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("数据目录需为绝对路径：{}", path.display()));
    }
    std::fs::create_dir_all(path).map_err(|e| format!("无法创建数据目录：{e}"))?;
    // 写删探针验证可写
    let probe = path.join(".zapmomo-probe");
    std::fs::write(&probe, b"ok").map_err(|e| format!("数据目录不可写：{e}"))?;
    let _ = std::fs::remove_file(&probe);

    let canon = path
        .canonicalize()
        .map_err(|e| format!("无法访问数据目录：{e}"))?;

    // 新 models 根 = data_dir/models；旧根 = ~/.zapmomo/models
    let new_models_root = canon.join("models");
    if let Some(legacy) = settings::legacy_models_dir()
        && let Ok(legacy_canon) = legacy.canonicalize()
        && (new_models_root.starts_with(&legacy_canon)
            || legacy_canon.starts_with(&new_models_root))
    {
        return Err("数据目录不能嵌套在现有模型根目录内，请选择独立目录".to_string());
    }
    Ok(canon)
}

// ---------------------------------------------------------------------------
// 迁移规划 / 执行
// ---------------------------------------------------------------------------

/// 规划迁移条目（旧根下顶层可迁移目录）。
pub fn plan_migration() -> Result<Vec<MigrateItem>, String> {
    let mut items = Vec::new();
    if let Some(legacy_models) = settings::legacy_models_dir() {
        let dest_root = settings::get_models_dir();
        if let Ok(entries) = std::fs::read_dir(&legacy_models) {
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name == ".install" || name.starts_with('.') {
                    continue; // staging / 隐藏目录不迁移
                }
                if !p.is_dir() {
                    continue;
                }
                let bytes = dir_size(&p);
                items.push(MigrateItem {
                    kind: MigrateKind::Model,
                    name: name.clone(),
                    source: p,
                    dest: dest_root.join(&name),
                    bytes,
                });
            }
        }
    }
    if let Some(legacy_comp) = settings::legacy_companions_dir() {
        let dest_root = settings::get_companions_store_dir();
        if let Ok(entries) = std::fs::read_dir(&legacy_comp) {
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name == "library.json" || name.starts_with(".tmp-") || !p.is_dir() {
                    continue; // 清单/临时目录不迁移
                }
                let bytes = dir_size(&p);
                items.push(MigrateItem {
                    kind: MigrateKind::Companion,
                    name: name.clone(),
                    source: p,
                    dest: dest_root.join(&name),
                    bytes,
                });
            }
        }
    }
    Ok(items)
}

/// 执行迁移。
///
/// - `force_copy`: 强制走 staging copy 路径（测试用；正常同卷走 rename）。
/// - `on_progress`: 进度回调。
/// - `cancel`: 取消标志（条目间与拷贝块间检查）。
pub fn run_migration(
    force_copy: bool,
    on_progress: &mut dyn FnMut(StorageMigrateProgress),
    cancel: Option<&AtomicBool>,
) -> Result<MigrateOutcome, String> {
    let mut outcome = MigrateOutcome::default();
    let items = plan_migration()?;
    let total = items.len();
    let bytes_total: u64 = items.iter().map(|i| i.bytes).sum();

    let mut done = 0usize;
    let mut bytes_done = 0u64;

    emit_progress(
        on_progress,
        "scanning",
        None,
        done,
        total,
        bytes_done,
        bytes_total,
        "正在规划迁移",
        &outcome.failed,
    );

    let legacy_models = settings::legacy_models_dir();
    let legacy_comp = settings::legacy_companions_dir();

    for item in &items {
        emit_progress(
            on_progress,
            "moving",
            Some(&item.name),
            done,
            total,
            bytes_done,
            bytes_total,
            &format!("正在迁移 {}", item.name),
            &outcome.failed,
        );
        // 条目间取消：emit 之后检查（回调可能在本轮置位 cancel，命中即停）
        if cancelled(cancel) {
            outcome.cancelled = true;
            break;
        }

        // 状态判定：dest 有 & source 无 → 已完成，跳过
        if item.dest.exists() && !item.source.exists() {
            outcome.skipped.push(item.name.clone());
            done += 1;
            continue;
        }
        // 双方都有 → 校验 dest 完整性
        if item.dest.exists() && item.source.exists() {
            if dest_complete(item) {
                // dest 完整有效 → 删 source 收尾
                match std::fs::remove_dir_all(&item.source) {
                    Ok(()) => {
                        outcome.skipped.push(item.name.clone());
                        commit_refs(item, legacy_models.as_deref(), legacy_comp.as_deref());
                    }
                    Err(e) => outcome
                        .failed
                        .push((item.name.clone(), format!("清理旧目录失败：{e}"))),
                }
                done += 1;
                continue;
            }
            // dest 不完整 → 删除重做
            let _ = std::fs::remove_dir_all(&item.dest);
        }

        // 正常迁移：同卷 rename 或跨卷 staging copy
        let result = if !force_copy && same_volume(&item.source, &item.dest) {
            if let Some(parent) = item.dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建目标父目录失败：{e}"))?;
            }
            std::fs::rename(&item.source, &item.dest).map_err(|e| format!("移动失败：{e}"))
        } else {
            copy_item(item, &mut bytes_done, on_progress, cancel).and_then(|_| {
                std::fs::remove_dir_all(&item.source).map_err(|e| format!("清理源目录失败：{e}"))
            })
        };

        match result {
            Ok(()) => {
                outcome.moved.push(item.name.clone());
                commit_refs(item, legacy_models.as_deref(), legacy_comp.as_deref());
            }
            Err(e) => outcome.failed.push((item.name.clone(), e)),
        }
        done += 1;
    }

    emit_progress(
        on_progress,
        if outcome.cancelled {
            "cancelled"
        } else if !outcome.failed.is_empty() {
            "done"
        } else {
            "finishing"
        },
        None,
        done,
        total,
        bytes_done,
        bytes_total,
        if outcome.cancelled {
            "已取消迁移"
        } else if !outcome.failed.is_empty() {
            "迁移完成（部分条目失败）"
        } else {
            "正在完成迁移"
        },
        &outcome.failed,
    );
    Ok(outcome)
}

/// 拷贝单个条目到 staging 再 rename 提交（跨卷路径）。
fn copy_item(
    item: &MigrateItem,
    bytes_done: &mut u64,
    on_progress: &mut dyn FnMut(StorageMigrateProgress),
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    let staging_parent = item.dest.parent().ok_or("目标目录无父目录")?;
    std::fs::create_dir_all(staging_parent).map_err(|e| format!("创建目标父目录失败：{e}"))?;
    let staging = staging_parent.join(format!(
        ".migrate-{}-{}-{}",
        item.name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    copy_dir_with_progress(
        &item.source,
        &staging,
        item.bytes,
        bytes_done,
        on_progress,
        cancel,
    )?;
    // staging → 最终名（同卷原子 rename）
    std::fs::rename(&staging, &item.dest).map_err(|e| format!("提交目标目录失败：{e}"))
}

/// 递归拷贝目录，逐文件 1MB 块推进字节进度。
fn copy_dir_with_progress(
    src: &Path,
    dst: &Path,
    _total_bytes: u64,
    bytes_done: &mut u64,
    _on_progress: &mut dyn FnMut(StorageMigrateProgress),
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建暂存目录失败：{e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("读取源目录失败：{e}"))? {
        let entry = entry.map_err(|e| format!("读取目录项失败：{e}"))?;
        if cancelled(cancel) {
            return Err("已取消".to_string());
        }
        let target = dst.join(entry.file_name());
        let ft = entry
            .file_type()
            .map_err(|e| format!("读取文件类型失败：{e}"))?;
        if ft.is_dir() {
            copy_dir_with_progress(
                &entry.path(),
                &target,
                _total_bytes,
                bytes_done,
                _on_progress,
                cancel,
            )?;
        } else if ft.is_file() {
            let mut reader = std::io::BufReader::new(
                std::fs::File::open(entry.path()).map_err(|e| format!("打开源文件失败：{e}"))?,
            );
            let mut writer = std::io::BufWriter::new(
                std::fs::File::create(&target).map_err(|e| format!("创建目标文件失败：{e}"))?,
            );
            use std::io::{Read, Write};
            let mut buf = vec![0u8; 1024 * 1024];
            loop {
                if cancelled(cancel) {
                    return Err("已取消".to_string());
                }
                let n = reader
                    .read(&mut buf)
                    .map_err(|e| format!("读取失败：{e}"))?;
                if n == 0 {
                    break;
                }
                writer
                    .write_all(&buf[..n])
                    .map_err(|e| format!("写入失败：{e}"))?;
                *bytes_done += n as u64;
            }
            writer.flush().map_err(|e| format!("刷新失败：{e}"))?;
        }
    }
    Ok(())
}

/// dest 完整性判定（models 看 `.zapmomo-lib.json` 或必需文件；companion 看清单）。
fn dest_complete(item: &MigrateItem) -> bool {
    match item.kind {
        MigrateKind::Model => item.dest.join(".zapmomo-lib.json").is_file(),
        MigrateKind::Companion => crate::live2d::config::find_model_file(&item.dest).is_some(),
    }
}

/// 条目迁移成功后改写引用（settings 绝对路径字段 / 伙伴 library.json）。
fn commit_refs(item: &MigrateItem, legacy_models: Option<&Path>, legacy_comp: Option<&Path>) {
    match item.kind {
        MigrateKind::Model => {
            if let Some(old_prefix) = legacy_models {
                let _ = rewrite_settings_paths(old_prefix, &settings::get_models_dir());
            }
        }
        MigrateKind::Companion => {
            if let Some(_old_prefix) = legacy_comp {
                // 把该伙伴条目改写为新 store 根
                let _ = crate::companion::relocate_payload(
                    &item.name,
                    &settings::get_companions_store_dir(),
                );
            }
        }
    }
}

/// 改写 settings 中指向旧根前缀的绝对路径字段为新根。
///
/// 持 SETTINGS_LOCK（经 `update_settings`）。只改恰好位于旧前缀下的绝对路径；
/// 相对路径 / 外部路径不动。
fn rewrite_settings_paths(old_prefix: &Path, new_prefix: &Path) -> Result<(), String> {
    model_library::update_settings(|cfg| {
        // kws/asr/tts model_dir、asr punctuation_model（LLM 已改远程连接，无本地路径）
        if let Some(k) = cfg.kws.as_mut()
            && let Some(v) = k.model_dir.as_mut()
        {
            *v = relocate_in(v, old_prefix, new_prefix);
        }
        if let Some(a) = cfg.asr.as_mut() {
            if let Some(v) = a.model_dir.as_mut() {
                *v = relocate_in(v, old_prefix, new_prefix);
            }
            if let Some(v) = a.punctuation_model.as_mut() {
                *v = relocate_in(v, old_prefix, new_prefix);
            }
        }
        if let Some(t) = cfg.tts.as_mut()
            && let Some(v) = t.model_dir.as_mut()
        {
            *v = relocate_in(v, old_prefix, new_prefix);
        }
        if let Some(l) = cfg.live2d.as_mut()
            && let Some(v) = l.model_dir.as_mut()
        {
            *v = relocate_in(v, old_prefix, new_prefix);
        }
        // model_library.local_models[].path（external 注册，仅当在旧根下）
        if let Some(ml) = cfg.model_library.as_mut() {
            for lm in &mut ml.local_models {
                lm.path = relocate_in(&lm.path, old_prefix, new_prefix);
            }
        }
    })
}

/// 相对 old_prefix 的路径改写为 new_prefix（不在 old_prefix 下则原样返回）。
fn relocate_in(value: &str, old_prefix: &Path, new_prefix: &Path) -> String {
    let p = Path::new(value);
    match settings::strip_prefix_ci(p, old_prefix) {
        Some(rest) => new_prefix.join(rest).display().to_string(),
        None => value.to_string(),
    }
}

fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    on_progress: &mut dyn FnMut(StorageMigrateProgress),
    state: &str,
    current_item: Option<&str>,
    items_done: usize,
    items_total: usize,
    bytes_done: u64,
    bytes_total: u64,
    message: &str,
    failed: &[(String, String)],
) {
    on_progress(StorageMigrateProgress {
        state: state.to_string(),
        current_item: current_item.map(|s| s.to_string()),
        items_done,
        items_total,
        bytes_done,
        bytes_total,
        message: message.to_string(),
        failed_items: failed
            .iter()
            .map(|(name, reason)| MigrateFailedItem {
                name: name.clone(),
                reason: reason.clone(),
            })
            .collect(),
    });
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppConfig;
    use crate::test_util::{run_with_temp_home, set_custom_data_dir};
    use std::sync::atomic::AtomicBool;

    fn make_model_dir(dir: &Path, file: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), b"x").unwrap();
    }

    fn make_installed_model(dir: &Path) {
        make_model_dir(dir, "f.onnx");
        std::fs::write(dir.join(".zapmomo-lib.json"), "{}").unwrap();
    }

    #[test]
    fn test_validate_rejects_relative() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let err = validate_data_dir(Path::new("relative/dir")).unwrap_err();
            assert!(err.contains("绝对"), "{err}");
        });
    }

    #[test]
    fn test_validate_rejects_nested_in_legacy_root() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let nested = home.join(".zapmomo/models/somewhere");
            let err = validate_data_dir(&nested).unwrap_err();
            assert!(err.contains("嵌套"), "{err}");
        });
    }

    #[test]
    fn test_validate_accepts_writable_abs() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let target = home.join("newdata");
            let canon = validate_data_dir(&target).unwrap();
            assert!(canon.exists());
        });
    }

    #[test]
    fn test_plan_skips_install_and_library_json() {
        run_with_temp_home(|home| {
            set_custom_data_dir(home);
            let legacy_models = home.join(".zapmomo/models");
            make_model_dir(&legacy_models.join("model-a"), "f.onnx");
            make_model_dir(&legacy_models.join(".install/tmp-x"), "f");
            let legacy_comp = home.join(".zapmomo/companions");
            make_model_dir(&legacy_comp.join("companion-abc"), "m.model3.json");
            std::fs::write(legacy_comp.join("library.json"), "{}").unwrap();

            let items = plan_migration().unwrap();
            let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
            assert!(names.contains(&"model-a"));
            assert!(names.contains(&"companion-abc"));
            assert!(!names.iter().any(|n| n.contains(".install")));
            assert!(!names.iter().any(|n| n.contains("library")));
        });
    }

    #[test]
    fn test_migrate_same_volume_moves_and_rewrites() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            let legacy_models = home.join(".zapmomo/models");
            make_installed_model(&legacy_models.join("model-a"));

            // settings 里指向旧根的绝对路径字段（保留 data_dir）
            crate::config::settings::save_settings(&AppConfig {
                data_dir: Some(data.display().to_string()),
                kws: Some(crate::config::settings::KwsSettings {
                    model_dir: Some(legacy_models.join("model-a").display().to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap();

            let outcome = run_migration(false, &mut |_| {}, None).unwrap();
            assert!(outcome.moved.iter().any(|n| n == "model-a"));
            assert!(!legacy_models.join("model-a").exists());
            assert!(data.join("models/model-a").is_dir());

            let cfg = crate::config::settings::load_settings().unwrap().unwrap();
            let md = cfg.kws.unwrap().model_dir.unwrap();
            let expected = data.join("models").join("model-a").display().to_string();
            assert_eq!(md, expected);
        });
    }

    #[test]
    fn test_migrate_force_copy_path() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            make_model_dir(&home.join(".zapmomo/models/model-copy"), "f.onnx");
            let mut bytes_seen = 0u64;
            let outcome = run_migration(
                true,
                &mut |p| {
                    bytes_seen = bytes_seen.max(p.bytes_done);
                },
                None,
            )
            .unwrap();
            assert!(outcome.moved.iter().any(|n| n == "model-copy"));
            assert!(data.join("models/model-copy").is_dir());
            assert!(bytes_seen > 0);
        });
    }

    #[test]
    fn test_migrate_idempotent_rerun() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            make_model_dir(&home.join(".zapmomo/models/m"), "f");
            run_migration(false, &mut |_| {}, None).unwrap();
            // 重跑即续：旧根已清空 → 无条目可迁移，无 moved 无 skipped
            let second = run_migration(false, &mut |_| {}, None).unwrap();
            assert!(second.moved.is_empty());
            assert!(second.skipped.is_empty());
            assert!(data.join("models/m").is_dir());
        });
    }

    #[test]
    fn test_migrate_cancel_midway_consistent() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            let cancel = AtomicBool::new(false);
            make_model_dir(&home.join(".zapmomo/models/m1"), "f");
            make_model_dir(&home.join(".zapmomo/models/m2"), "f");
            let mut seen = 0u32;
            let outcome = run_migration(
                false,
                &mut |p| {
                    if p.items_done >= 1 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    seen = seen.max(p.items_done as u32);
                },
                Some(&cancel),
            )
            .unwrap();
            assert!(outcome.cancelled);
            // 至少 m1 已迁移（一致），m2 可能留在旧根
            assert!(data.join("models/m1").is_dir());
            assert!(seen >= 1);
        });
    }

    #[test]
    fn test_migrate_dest_conflict_keeps_valid_and_frees_source() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            // dest 已有完整安装（带 meta），source 也有 → 保留 dest、删 source
            make_installed_model(&data.join("models/dup"));
            make_model_dir(&home.join(".zapmomo/models/dup"), "old.onnx");
            let outcome = run_migration(false, &mut |_| {}, None).unwrap();
            assert!(data.join("models/dup/.zapmomo-lib.json").is_file());
            assert!(!home.join(".zapmomo/models/dup").exists());
            assert!(
                outcome.skipped.iter().any(|n| n == "dup")
                    || outcome.moved.iter().any(|n| n == "dup")
            );
        });
    }

    #[test]
    fn test_migrate_companion_rewrites_library() {
        run_with_temp_home(|home| {
            let data = set_custom_data_dir(home);
            let comp_dir = home.join(".zapmomo/companions");
            let id = "companion-abc123";
            make_model_dir(&comp_dir.join(id), "cat.model3.json");
            std::fs::write(comp_dir.join(id).join("cat.model3.json"), "{}").unwrap();
            let lib = crate::companion::CompanionLibrary {
                schema_version: crate::companion::SCHEMA_VERSION,
                models: vec![crate::companion::CompanionModel {
                    id: id.to_string(),
                    name: "cat".into(),
                    source_path: None,
                    model_dir: comp_dir.join(id).display().to_string(),
                    model_file: comp_dir
                        .join(id)
                        .join("cat.model3.json")
                        .display()
                        .to_string(),
                    format: "Cubism3".into(),
                    imported_at: "t".into(),
                    voice_id: None,
                    layout: None,
                }],
                active_model_id: Some(id.to_string()),
                completed_migrations: Vec::new(),
            };
            std::fs::create_dir_all(&comp_dir).unwrap();
            std::fs::write(
                comp_dir.join("library.json"),
                serde_json::to_string_pretty(&lib).unwrap(),
            )
            .unwrap();

            let outcome = run_migration(false, &mut |_| {}, None).unwrap();
            assert!(outcome.moved.iter().any(|n| n == id));

            // library.json 条目已改写指向新 store
            let reloaded = crate::companion::load_library_fast().unwrap();
            let m = reloaded.models.iter().find(|m| m.id == id).unwrap();
            let expected = data.join("companions").join(id).display().to_string();
            assert_eq!(m.model_dir, expected);
        });
    }
}
