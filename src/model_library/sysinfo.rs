//! 系统资源检测（内存 / 磁盘 / CPU），供模型库「系统资源」卡片。
//!
//! 依赖 `sysinfo`（跨平台）。调用方应在 `spawn_blocking` 中执行，避免阻塞 UI 线程。

use super::SystemResources;

/// 获取系统资源快照。
///
/// - 内存：总 / 可用。
/// - 磁盘：模型目录所在挂载点的总 / 可用空间。
/// - CPU：两次 refresh（间隔 200ms）取瞬时使用率（首次 refresh 为 0）。
pub fn get_system_resources() -> SystemResources {
    use sysinfo::{Disks, System};

    let mut sys = System::new_all();
    sys.refresh_memory();

    sys.refresh_cpu_usage();
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_usage();
    let cpu_usage = sys.global_cpu_usage();

    let disks = Disks::new_with_refreshed_list();
    let models_dir = crate::config::settings::get_models_dir();
    let mut disk_total: u64 = 0;
    let mut disk_available: u64 = 0;
    let mut best_len: Option<u64> = None;
    for d in &disks {
        let mount = d.mount_point();
        if models_dir.starts_with(mount) {
            let len = mount.as_os_str().to_string_lossy().len() as u64;
            if best_len.is_none_or(|b| len > b) {
                best_len = Some(len);
                disk_total = d.total_space();
                disk_available = d.available_space();
            }
        }
    }

    SystemResources {
        total_memory: sys.total_memory(),
        available_memory: sys.available_memory(),
        disk_total,
        disk_available,
        cpu_usage,
    }
}
