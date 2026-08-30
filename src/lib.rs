/// 通用工具模块
pub mod asr;
pub mod audio;
pub mod audiocpp;
pub mod cli;
pub mod companion;
pub mod companion_bubble_link;
pub mod companion_click_through;
pub mod companion_sprites;
pub mod config;
pub mod datetime;
pub mod dsh;
pub mod kws;
pub mod live2d;
pub mod llm;
pub mod logging;
pub mod model_library;
pub mod tts;
pub mod voice;

#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::{Mutex, OnceLock};

    /// 全局 HOME 锁，串行化所有修改 HOME 的测试
    static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// 获取 HOME 锁守卫
    pub(crate) fn acquire_home_lock() -> std::sync::MutexGuard<'static, ()> {
        HOME_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 写入自定义 data_dir 设置（双根/迁移测试共用），返回数据目录路径。
    pub(crate) fn set_custom_data_dir(home: &std::path::Path) -> std::path::PathBuf {
        let data = home.join("zapdata");
        let mut config = crate::config::settings::AppConfig::default();
        config.data_dir = Some(data.display().to_string());
        crate::config::settings::save_settings(&config).unwrap();
        data
    }

    /// 在临时 HOME 目录下执行测试函数
    /// 使用全局锁确保 HOME 环境变量不会被并行测试竞态覆盖
    pub fn run_with_temp_home(f: impl FnOnce(&std::path::Path)) {
        let _guard = acquire_home_lock();
        crate::config::settings::reset_data_dir_cache_for_test();
        let dir = tempfile::tempdir().unwrap();
        let orig_home = std::env::var("HOME").ok();
        // SAFETY: HOME_LOCK 确保无竞态
        unsafe {
            std::env::set_var("HOME", dir.path());
        }
        f(dir.path());
        match orig_home {
            Some(h) => unsafe {
                std::env::set_var("HOME", h);
            },
            None => unsafe {
                std::env::remove_var("HOME");
            },
        }
    }
}
