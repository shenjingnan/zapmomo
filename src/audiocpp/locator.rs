use std::path::PathBuf;
use std::sync::OnceLock;

use super::AudiocppError;

/// externalBin 落盘的引擎文件名：`audiocpp_server-<target-triple>`（Windows 加 `.exe`）。
///
/// triple 与 release.yml matrix 的 tauri target 一致，编译期生成，用于在
/// 「主程序同目录」（Tauri externalBin 落位点）按名查找。
pub fn engine_file_name() -> String {
    let triple = {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            "aarch64-apple-darwin"
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            "x86_64-apple-darwin"
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            "x86_64-unknown-linux-gnu"
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            "x86_64-pc-windows-msvc"
        }
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64")
        )))]
        {
            compile_error!("audio.cpp sidecar 尚不支持该平台");
        }
    };
    if cfg!(target_os = "windows") {
        format!("audiocpp_server-{triple}.exe")
    } else {
        format!("audiocpp_server-{triple}")
    }
}

/// 进程级搜索目录注入（宿主应用在启动时调用一次）。
///
/// GUI（src-tauri）注入 `resource_dir` 与 `current_exe` 同目录；CLI 不注入，
/// 依赖 `~/.zapmomo/engines/` 与 PATH 兜底。
static SEARCH_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// 注入引擎搜索目录（仅首次调用生效，幂等）。
pub fn set_search_dirs(dirs: Vec<PathBuf>) {
    let _ = SEARCH_DIRS.set(dirs);
}

/// 注入的搜索目录快照（未注入 → 空）。
///
/// 供 spawn 时构造子进程 DLL 搜索路径（Windows CUDA 运行时 DLL 随
/// resources 落 resource_dir，与引擎 exe 不同目录时依赖子进程 PATH 解析）。
pub fn search_dirs() -> Vec<PathBuf> {
    SEARCH_DIRS.get().cloned().unwrap_or_default()
}

/// 引擎目录（`<data_dir>/engines`，未自定义时为 `~/.zapmomo/engines`）。
pub fn engines_dir() -> PathBuf {
    crate::config::settings::get_data_dir()
        .unwrap_or_else(crate::config::settings::get_settings_dir)
        .join("engines")
}

/// 在目录内查找引擎：优先带 triple 后缀的 externalBin 命名，其次无后缀
/// `audiocpp_server`（手动放置场景）。
fn find_in_dir(dir: &std::path::Path) -> Option<PathBuf> {
    let triple_name = engine_file_name();
    let plain = if cfg!(target_os = "windows") {
        "audiocpp_server.exe"
    } else {
        "audiocpp_server"
    };
    for name in [triple_name.as_str(), plain] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 遍历 `PATH` 环境变量查找引擎（`split_paths` 自动处理平台分隔符）。
fn find_in_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| find_in_dir(&dir))
}

/// 定位 audiocpp_server 引擎二进制。
///
/// 优先级：显式覆盖（`[tts].engine_path` / CLI `--engine-path`）> 注入目录
/// （GUI externalBin 落位点）> `~/.zapmomo/engines/` > `PATH`。
/// 全部未命中返回 [`AudiocppError::EngineNotFound`]（带已搜索路径列表）。
pub fn locate_engine(explicit: Option<&std::path::Path>) -> Result<PathBuf, AudiocppError> {
    let engines = engines_dir();
    let mut searched: Vec<PathBuf> = Vec::new();

    if let Some(p) = explicit {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        searched.push(p.to_path_buf());
    }
    if let Some(dirs) = SEARCH_DIRS.get() {
        for dir in dirs {
            if let Some(found) = find_in_dir(dir) {
                return Ok(found);
            }
            searched.push(dir.clone());
        }
    }
    if let Some(found) = find_in_dir(&engines) {
        return Ok(found);
    }
    searched.push(engines);
    if let Some(found) = find_in_path() {
        return Ok(found);
    }

    Err(AudiocppError::EngineNotFound { searched })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_file_name_contains_triple() {
        let name = engine_file_name();
        assert!(name.starts_with("audiocpp_server-"), "name: {name}");
        if cfg!(target_os = "windows") {
            assert!(name.ends_with(".exe"));
        } else {
            assert!(!name.ends_with(".exe"));
        }
    }

    #[test]
    fn test_locate_engine_explicit_missing_reports_searched() {
        // 隔离 HOME：显式路径不存在且后续候选（engines 目录）为空 → EngineNotFound，
        // 文案含该路径（真机 ~/.zapmomo/engines/ 可能放有引擎，不隔离会 fallback 命中）
        crate::test_util::run_with_temp_home(|_| {
            let err = locate_engine(Some(std::path::Path::new("/nonexistent/audiocpp_server")))
                .unwrap_err();
            let msg = err.to_user_message();
            assert!(msg.contains("未找到 audiocpp_server"), "msg: {msg}");
            assert!(msg.contains("/nonexistent/audiocpp_server"), "msg: {msg}");
        });
    }

    #[test]
    fn test_locate_engine_explicit_hit_short_circuits() {
        // 显式路径存在 → 直接命中，不触及其它候选
        let base = tempfile::tempdir().unwrap();
        let exe = base.path().join(engine_file_name());
        std::fs::write(&exe, b"stub").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(locate_engine(Some(&exe)).unwrap(), exe);
    }

    #[test]
    fn test_find_in_dir_prefers_triple_then_plain() {
        let base = tempfile::tempdir().unwrap();
        // 空 → 无
        assert!(find_in_dir(base.path()).is_none());
        // 无后缀命中
        let plain = base.path().join(if cfg!(windows) {
            "audiocpp_server.exe"
        } else {
            "audiocpp_server"
        });
        std::fs::write(&plain, b"x").unwrap();
        assert_eq!(find_in_dir(base.path()), Some(plain));
        // 带 triple 优先
        let triple = base.path().join(engine_file_name());
        std::fs::write(&triple, b"x").unwrap();
        assert_eq!(find_in_dir(base.path()), Some(triple));
    }
}
