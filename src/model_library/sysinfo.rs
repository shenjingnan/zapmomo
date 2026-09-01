//! 系统资源检测（内存 / 磁盘 / CPU）与磁盘空间校验。
//!
//! 依赖 `sysinfo`（跨平台）。调用方应在 `spawn_blocking` 中执行，避免阻塞 UI 线程。
//! 空间校验与建议目录挑选的纯逻辑（`check_disk_space` / `pick_suggested_dir`）
//! 以 `DiskInfo` 快照为入参，可在单测中合成注入，不依赖真实磁盘。

use std::path::{Path, PathBuf};

use super::SystemResources;

/// 获取系统资源快照。
///
/// - 内存：总 / 可用。
/// - 磁盘：模型目录所在挂载点的总 / 可用空间。
/// - CPU：两次 refresh（间隔 200ms）取瞬时使用率（首次 refresh 为 0）。
pub fn get_system_resources() -> SystemResources {
    use sysinfo::System;

    let mut sys = System::new_all();
    sys.refresh_memory();

    sys.refresh_cpu_usage();
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_usage();
    let cpu_usage = sys.global_cpu_usage();

    let disks = list_disks();
    let models_dir = crate::config::settings::get_models_dir();
    let (disk_total, disk_available) =
        volume_of(&disks, &models_dir).map_or((0, 0), |v| (v.total, v.available));

    SystemResources {
        total_memory: sys.total_memory(),
        available_memory: sys.available_memory(),
        disk_total,
        disk_available,
        cpu_usage,
    }
}

// ---------------------------------------------------------------------------
// 磁盘快照与查询
// ---------------------------------------------------------------------------

/// 单个磁盘快照（纯数据，供 `volume_of` / `pick_suggested_dir` 等纯函数注入测试）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskInfo {
    pub mount_point: PathBuf,
    pub available: u64,
    pub total: u64,
    pub removable: bool,
    pub read_only: bool,
}

/// 枚举所有磁盘快照（跨平台，无 cfg(windows) 分支）。
pub fn list_disks() -> Vec<DiskInfo> {
    sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .map(|d| DiskInfo {
            mount_point: d.mount_point().to_path_buf(),
            available: d.available_space(),
            total: d.total_space(),
            removable: d.is_removable(),
            read_only: d.is_read_only(),
        })
        .collect()
}

/// `dir` 所在卷（挂载点最长前缀匹配，未命中 → `None`）。
///
/// 匹配前先剥离 dir 的 Windows verbatim 前缀（canonicalize 产物 `\\?\C:\...`
/// 的 `VerbatimDisk(C)` 与挂载点 `C:\` 的 `Disk(C)` 逐组件比较不相等，会导致
/// 永远匹配不到卷、空间误报 0）。
pub fn volume_of<'a>(disks: &'a [DiskInfo], dir: &Path) -> Option<&'a DiskInfo> {
    let dir = &crate::config::settings::strip_verbatim_prefix(dir.to_path_buf());
    let mut best: Option<(usize, &DiskInfo)> = None;
    for d in disks {
        if dir.starts_with(&d.mount_point) {
            let len = d.mount_point.as_os_str().len();
            if best.is_none_or(|(b, _)| len > b) {
                best = Some((len, d));
            }
        }
    }
    best.map(|(_, d)| d)
}

/// `dir` 所在卷 `(total, available)`（未命中 → `(0, 0)`）。
pub fn disk_space(dir: &Path) -> (u64, u64) {
    volume_of(&list_disks(), dir).map_or((0, 0), |v| (v.total, v.available))
}

/// `dir` 所在卷可用字节（未命中 → 0）。
pub fn available_space(dir: &Path) -> u64 {
    disk_space(dir).1
}

// ---------------------------------------------------------------------------
// 空间校验（下载 / 导入前置检查）
// ---------------------------------------------------------------------------

/// 下载所需空间 = 载荷 ×2（下载包 + 解压产物共存 staging）+ 256 MiB 底量。
pub fn required_bytes_for_download(payload: u64) -> u64 {
    payload.saturating_mul(2).saturating_add(256 * 1024 * 1024)
}

/// 导入所需空间 = 载荷 + 64 MiB 余量（逐文件复制，无压缩包膨胀）。
pub fn required_bytes_for_import(payload: u64) -> u64 {
    payload.saturating_add(64 * 1024 * 1024)
}

/// 空间校验：不足返回中文错误（文案引导到「设置 → 存储位置」）。
pub fn check_disk_space(available: u64, required: u64) -> Result<(), String> {
    if available >= required {
        return Ok(());
    }
    Err(format!(
        "磁盘空间不足：约需 {}，当前该卷仅剩 {}。请到「设置 → 存储位置」更改存储目录，或清理磁盘后重试。",
        format_bytes(required),
        format_bytes(available)
    ))
}

/// 字节人性化展示（1 位小数，GB/TB；不足 1GB 按 MB）。
fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

// ---------------------------------------------------------------------------
// 建议存储目录（首次下载引导）
// ---------------------------------------------------------------------------

/// 建议卷自身空间下限。
pub const SUGGEST_MIN_BYTES: u64 = 1024 * 1024 * 1024;
/// 建议卷须比默认（home）卷多出的可用空间边际。
pub const SUGGEST_MARGIN_BYTES: u64 = 1024 * 1024 * 1024;

/// 挑选建议存储目录（纯函数，可注入合成磁盘表单测）。
///
/// 规则：排除可移动 / 只读卷、home 所在卷、挂载点深度 > 1 的卷（网络盘
/// `\\?\UNC\...`、squashfs/snap 等伪挂载）；剩余空间最大者；须 ≥
/// [`SUGGEST_MIN_BYTES`] 且比 home 卷可用空间多 [`SUGGEST_MARGIN_BYTES`] 以上。
/// 返回 `<挂载点>/ZapMomo`；无合格候选（典型：单盘机器）→ `None`。
pub fn pick_suggested_dir(disks: &[DiskInfo], home: &Path) -> Option<PathBuf> {
    let home_volume = volume_of(disks, home);
    let candidate = disks
        .iter()
        .filter(|d| !d.removable && !d.read_only)
        .filter(|d| home_volume.is_none_or(|hv| d.mount_point != hv.mount_point))
        .filter(|d| mount_depth(&d.mount_point) <= 1)
        .filter(|d| d.available >= SUGGEST_MIN_BYTES)
        .filter(|d| {
            home_volume
                .is_none_or(|hv| d.available >= hv.available.saturating_add(SUGGEST_MARGIN_BYTES))
        })
        .max_by(|a, b| {
            // 并列时取挂载点字典序较小者，保证确定性
            a.available
                .cmp(&b.available)
                .then_with(|| b.mount_point.cmp(&a.mount_point))
        })?;
    Some(candidate.mount_point.join("ZapMomo"))
}

/// 挂载点深度：除根/盘符前缀外的路径成分数。`C:\` 与 `/` 均为 0，
/// `/Volumes/X` 为 2（被排除）。
fn mount_depth(mount: &Path) -> usize {
    mount
        .components()
        .filter(|c| {
            !matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
        .count()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn disk(mount: &str, available: u64) -> DiskInfo {
        DiskInfo {
            mount_point: PathBuf::from(mount),
            available,
            total: available * 2,
            removable: false,
            read_only: false,
        }
    }

    // ---- required_bytes ----

    #[test]
    fn test_required_download_floor_and_multiply() {
        assert_eq!(required_bytes_for_download(0), 256 * 1024 * 1024);
        assert_eq!(required_bytes_for_download(GB), 2 * GB + 256 * 1024 * 1024);
    }

    #[test]
    fn test_required_import_margin() {
        assert_eq!(required_bytes_for_import(0), 64 * 1024 * 1024);
        assert_eq!(required_bytes_for_import(GB), GB + 64 * 1024 * 1024);
    }

    #[test]
    fn test_required_saturating_no_panic() {
        let r = required_bytes_for_download(u64::MAX);
        assert_eq!(r, u64::MAX);
        let r = required_bytes_for_import(u64::MAX);
        assert_eq!(r, u64::MAX);
    }

    // ---- check_disk_space ----

    #[test]
    fn test_check_space_sufficient_ok() {
        assert!(check_disk_space(10 * GB, 5 * GB).is_ok());
        assert!(check_disk_space(5 * GB, 5 * GB).is_ok());
    }

    #[test]
    fn test_check_space_insufficient_message() {
        let err = check_disk_space(1 * GB, 5 * GB).unwrap_err();
        assert!(err.contains("磁盘空间不足"), "{err}");
        assert!(err.contains("存储位置"), "{err}");
        assert!(err.contains("GB"), "{err}");
    }

    // ---- volume_of ----

    #[test]
    fn test_volume_of_longest_prefix() {
        let disks = vec![disk("/Users", 1), disk("/Users/local/deep", 2)];
        assert_eq!(
            volume_of(&disks, Path::new("/Users/x")).unwrap().available,
            1
        );
        assert_eq!(
            volume_of(&disks, Path::new("/Users/local/deep/z"))
                .unwrap()
                .available,
            2
        );
        assert!(volume_of(&disks, Path::new("/etc")).is_none());
    }

    #[test]
    fn test_volume_of_empty_when_no_match() {
        let disks = vec![disk("/data", 1)];
        assert!(volume_of(&disks, Path::new("/home/u")).is_none());
    }

    // Windows canonicalize 产物为 verbatim 路径（`\\?\D:\...`），其 Prefix 组件
    // `VerbatimDisk(D)` 与挂载点 `D:\` 的 `Disk(D)` 逐组件比较不相等；volume_of
    // 须先剥离 verbatim 前缀，否则查不到卷、可用空间误报 0。
    #[cfg(windows)]
    #[test]
    fn test_volume_of_matches_verbatim_dir() {
        let disks = vec![disk("D:\\", 100 * GB)];
        let d = volume_of(&disks, Path::new(r"\\?\D:\zapmomo\models")).unwrap();
        assert_eq!(d.available, 100 * GB);
        // 普通路径不受影响；剥前缀后仍不属于该卷 → None
        assert_eq!(
            volume_of(&disks, Path::new(r"D:\other")).unwrap().available,
            100 * GB
        );
        assert!(volume_of(&disks, Path::new(r"\\?\E:\x")).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_volume_of_matches_verbatim_unc_dir() {
        let disks = vec![disk(r"\\server\share", 100 * GB)];
        let d = volume_of(&disks, Path::new(r"\\?\UNC\server\share\zapmomo")).unwrap();
        assert_eq!(d.available, 100 * GB);
    }

    // ---- pick_suggested_dir ----

    #[test]
    fn test_pick_largest_non_home_volume() {
        let disks = vec![disk("/home", 100 * GB), disk("/data", 500 * GB)];
        let picked = pick_suggested_dir(&disks, Path::new("/home/u")).unwrap();
        assert_eq!(picked, PathBuf::from("/data/ZapMomo"));
    }

    #[test]
    fn test_pick_excludes_removable_and_readonly() {
        let mut removable = disk("/data", 500 * GB);
        removable.removable = true;
        let mut readonly = disk("/backup", 500 * GB);
        readonly.read_only = true;
        let disks = vec![disk("/home", 100 * GB), removable, readonly];
        assert!(pick_suggested_dir(&disks, Path::new("/home/u")).is_none());
    }

    #[test]
    fn test_pick_none_when_only_home_volume() {
        let disks = vec![disk("/home", 100 * GB)];
        assert!(pick_suggested_dir(&disks, Path::new("/home/u")).is_none());
    }

    #[test]
    fn test_pick_requires_margin_over_home() {
        // 只比 home 卷多 100MB → 不推荐
        let disks = vec![
            disk("/home", 100 * GB),
            disk("/data", 100 * GB + 100 * 1024 * 1024),
        ];
        assert!(pick_suggested_dir(&disks, Path::new("/home/u")).is_none());
    }

    #[test]
    fn test_pick_requires_min_volume_size() {
        let disks = vec![disk("/home", 100 * GB), disk("/data", 512 * 1024 * 1024)];
        assert!(pick_suggested_dir(&disks, Path::new("/home/u")).is_none());
    }

    #[test]
    fn test_pick_excludes_deep_mounts() {
        // /Volumes/X、/mnt/data 等深度 >1 挂载点视为伪挂载/外置卷，不推荐
        let disks = vec![disk("/home", 100 * GB), disk("/Volumes/Big", 500 * GB)];
        assert!(pick_suggested_dir(&disks, Path::new("/home/u")).is_none());
    }

    #[test]
    fn test_pick_deterministic_on_tie() {
        let disks = vec![
            disk("/home", 100 * GB),
            disk("/b", 500 * GB),
            disk("/a", 500 * GB),
        ];
        let picked = pick_suggested_dir(&disks, Path::new("/home/u")).unwrap();
        assert_eq!(picked, PathBuf::from("/a/ZapMomo"));
    }

    #[test]
    fn test_mount_depth_roots_are_zero() {
        assert_eq!(mount_depth(Path::new("/")), 0);
        assert_eq!(mount_depth(Path::new("/mnt/data")), 2);
    }
}
