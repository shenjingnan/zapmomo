//! dsh 可执行文件发现：在 node 版本管理器与常见 bin 目录中探测 `dsh`。
//!
//! GUI 进程的 PATH 极窄，且 fnm/nvm 按 node 版本隔离全局 bin（实测：dsh 在 fnm
//! v22 的 bin、pnpm 在 v24 的 bin），靠 `which dsh` 基本必失败。因此按候选目录
//! 清单逐一探测，命中后以 `--version` 轻量验证；全 miss 返回带 searched 列表的
//! [`DshDiscoveryError`]（对齐 audiocpp locator 的 EngineNotFound 诊断惯例）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--version` 验证超时：超过视为不可用（坏 node / 卡死的 shim / 假文件）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 发现失败：全部探测过的目录（诊断展示用）。
#[derive(Debug, Clone, PartialEq)]
pub struct DshDiscoveryError {
    pub searched: Vec<String>,
}

impl std::fmt::Display for DshDiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "未找到可用的 dsh 可执行文件，已尝试 {} 个位置",
            self.searched.len()
        )
    }
}

/// 平台可执行文件名（Windows npm 全局包落为 `.cmd` 垫片，spawn 需经 `cmd /C`）。
fn executable_name() -> &'static str {
    if cfg!(windows) { "dsh.cmd" } else { "dsh" }
}

/// 版本目录根（每个子目录是一个 node 版本）与其内部 bin 的相对路径。
fn versioned_roots(home: &Path) -> Vec<(PathBuf, &'static str)> {
    vec![
        // fnm：node-versions/<ver>/installation/bin（实测本机 dsh 在此）
        (
            home.join(".local/share/fnm/node-versions"),
            "installation/bin",
        ),
        // nvm：versions/node/<ver>/bin
        (home.join(".nvm/versions/node"), "bin"),
    ]
}

/// 固定 bin 目录（volta shims / homebrew / npm 全局默认前缀）。
fn fixed_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".volta/bin"),
        home.join(".local/bin"),
    ];
    if cfg!(windows)
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        dirs.insert(0, PathBuf::from(appdata).join("npm"));
    }
    dirs
}

/// 版本排序键：`v22.22.2` → [22, 22, 2] 逐段数值比较（非数值段按 0），名称兜底。
fn version_key(path: &Path) -> (Vec<u64>, String) {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let nums = name
        .trim_start_matches('v')
        .split('.')
        .map(|seg| seg.parse::<u64>().unwrap_or(0))
        .collect();
    (nums, name.to_string())
}

/// 收集版本管理器下的候选 bin 目录：每个 root 内版本数值降序（新版本优先）。
fn collect_versioned_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for (root, bin_rel) in versioned_roots(home) {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        versions.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
        for v in versions {
            dirs.push(v.join(bin_rel));
        }
    }
    dirs
}

/// 全部 node 工具链 bin 目录（版本管理器 + 固定目录，仅存在的、symlink 去重）。
///
/// 供代安装时增补 PATH：pnpm 可能住在与 dsh 不同的 node 版本目录（实测：dsh 在
/// fnm v22 的 bin、pnpm 经 corepack 在 v24 的 bin），只补 dsh 所在目录会
/// `pnpm not found`。
pub fn node_bin_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs = collect_versioned_dirs(home);
    dirs.extend(fixed_dirs(home));
    dirs.retain(|d| d.is_dir());
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.canonicalize().unwrap_or_else(|_| d.clone())));
    dirs
}

/// 汇总候选目录：PATH 优先（终端启动的开发模式最可信），随后版本管理器、固定目录。
/// 只收真实存在的目录并记入 searched；symlink 去重防同目录重复探测。
fn candidate_dirs(home: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut searched: Vec<String> = Vec::new();
    let mut push = |dir: PathBuf| {
        if !dir.is_dir() {
            return;
        }
        searched.push(dir.display().to_string());
        candidates.push(dir);
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&paths) {
            push(p);
        }
    }
    for dir in node_bin_dirs(home) {
        push(dir);
    }
    let mut seen = HashSet::new();
    candidates.retain(|d| seen.insert(d.canonicalize().unwrap_or_else(|_| d.clone())));
    (candidates, searched)
}

/// 发现入口：候选目录逐一探测 + `--version` 验证；全 miss 返回 searched 诊断。
pub fn find_dsh_executable(home: &Path) -> Result<PathBuf, DshDiscoveryError> {
    let name = executable_name();
    let (dirs, searched) = candidate_dirs(home);
    find_in_dirs(&dirs, name, &probe_version).map_err(|()| DshDiscoveryError { searched })
}

/// 按序探测候选目录：第一个含 `<name>` 且通过 `probe` 验证的文件胜出。
fn find_in_dirs(
    dirs: &[PathBuf],
    name: &str,
    probe: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, ()> {
    for dir in dirs {
        let exe = dir.join(name);
        if exe.is_file() && probe(&exe) {
            return Ok(exe);
        }
    }
    Err(())
}

/// 轻量验证：`<dsh> --version` 在 [`PROBE_TIMEOUT`] 内成功退出即视为可用。
/// 输出直接丢弃（null stdio，避免管道写阻塞）；超时 kill。
fn probe_version(dsh: &Path) -> bool {
    let mut cmd = if cfg!(windows) {
        // .cmd 垫片必须经 cmd 解释器启动
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(dsh);
        c
    } else {
        std::process::Command::new(dsh)
    };
    cmd.arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 同 audiocpp server：不弹控制台窗
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// probe 恒真（纯文件布局测试，不真 spawn）。
    fn probe_ok(_: &Path) -> bool {
        true
    }

    /// probe 恒假（模拟 --version 全部失败）。
    fn probe_fail(_: &Path) -> bool {
        false
    }

    /// 在临时目录造 `<fnm>/node-versions/<ver>/installation/bin/dsh`。
    fn make_fnm_dsh(fnm_root: &Path, version: &str) -> PathBuf {
        let bin = fnm_root
            .join("node-versions")
            .join(version)
            .join("installation/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("dsh"), "#!/bin/sh\n").unwrap();
        bin
    }

    #[test]
    fn test_version_key_numeric_not_lexicographic() {
        // 字典序 v9 > v22，数值序必须反过来
        let v22 = version_key(Path::new("v22.22.2"));
        let v9 = version_key(Path::new("v9.11.0"));
        assert!(v22 > v9, "数值感知排序：{v22:?} 应大于 {v9:?}");
        let weird = version_key(Path::new("not-a-version"));
        assert_eq!(weird.0, vec![0], "非版本名数值段按 0");
    }

    #[test]
    fn test_collect_versioned_dirs_descending() {
        let tmp = tempfile::TempDir::new().unwrap();
        let fnm = tmp.path().join("fnm");
        make_fnm_dsh(&fnm, "v9.0.0");
        make_fnm_dsh(&fnm, "v22.22.2");
        make_fnm_dsh(&fnm, "v20.10.0");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".local/share")).unwrap();
        std::fs::rename(&fnm, home.join(".local/share/fnm")).unwrap();

        let dirs = collect_versioned_dirs(&home);
        assert!(
            dirs[0].ends_with("v22.22.2/installation/bin"),
            "最高版本应排最前: {dirs:?}"
        );
        assert!(dirs.iter().any(|d| d.ends_with("v9.0.0/installation/bin")));
        // 空版本目录（无 bin）也应产出候选路径（探测时 is_file 兜底），此处只验顺序
        let names: Vec<String> = dirs
            .iter()
            .filter_map(|d| {
                // <...>/node-versions/<ver>/installation/bin → 往上两级取 <ver>
                d.parent()
                    .and_then(Path::parent)
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .map(str::to_owned)
            })
            .collect();
        assert_eq!(names[0], "v22.22.2");
        assert_eq!(names[1], "v20.10.0");
        assert_eq!(names[2], "v9.0.0");
    }

    #[test]
    fn test_find_in_dirs_first_hit_wins() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("dsh"), "").unwrap();
        let dirs = vec![a.clone(), b.clone()];

        let hit = find_in_dirs(&dirs, "dsh", &probe_ok).unwrap();
        assert_eq!(hit, b.join("dsh"), "第一个含可执行文件的目录胜出");
        // 目录不存在 / 文件缺失 → 跳过
        let miss = find_in_dirs(&[tmp.path().join("none")], "dsh", &probe_ok);
        assert!(miss.is_err());
        // probe 全败（坏 shim）→ 不选中
        let rejected = find_in_dirs(&dirs, "dsh", &probe_fail);
        assert!(rejected.is_err(), "probe 失败的候选应被跳过");
    }

    #[test]
    fn test_find_in_dirs_skips_non_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("bin");
        std::fs::create_dir_all(dir.join("dsh")).unwrap(); // 同名目录而非文件
        assert!(find_in_dirs(&[dir], "dsh", &probe_ok).is_err());
    }

    #[test]
    fn test_executable_name_by_platform() {
        // 契约锁定：Windows 走 .cmd 垫片，其余 dsh
        if cfg!(windows) {
            assert_eq!(executable_name(), "dsh.cmd");
        } else {
            assert_eq!(executable_name(), "dsh");
        }
    }

    #[test]
    fn test_discovery_error_display() {
        let err = DshDiscoveryError {
            searched: vec!["/a".into(), "/b".into()],
        };
        assert!(err.to_string().contains('2'), "诊断应含尝试数量");
    }
}
