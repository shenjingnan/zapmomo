//! dsh 集成状态检测：纯文件级读取，判定 dsh 环境 / 插件安装 / 激活状态。
//!
//! 「有 dsh 环境」的锚点是数据目录 `~/.dsh`（而非 CLI 在 PATH 中——fnm/nvm 等
//! node 版本管理器把全局 bin 隔离到版本目录里，GUI 进程几乎必然找不到，实测如此）。
//! web profile 是 pnpm 包结构：`profiles/web/package.json` 的 `dependencies` 记录
//! 安装（link/git/npm 值均算），`dsh.profile.bundles` 记录激活（dsh 启动时加载）。

use serde::Serialize;
use std::path::Path;

/// dsh 桥插件包名（dependencies 键 / dsh.profile.bundles 条目）。
pub const PLUGIN_PACKAGE: &str = "@zapmomo-ai/dsh-plugin";

/// 手动安装命令（一键安装失败时的复制兜底，插件 README 同款）。
pub const MANUAL_COMMAND: &str = "dsh plugin --profile web add @zapmomo-ai/dsh-plugin";

/// web profile 的 package.json 路径：`<dsh_home>/profiles/web/package.json`。
fn profile_package_json(dsh_home: &Path) -> std::path::PathBuf {
    dsh_home.join("profiles").join("web").join("package.json")
}

/// dsh 集成状态（全部纯文件级判定，无子进程；tauri 侧 `detect_dsh_integration` 直出）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DshIntegrationStatus {
    /// `~/.dsh/` 数据目录存在（有 dsh 环境）
    pub dsh_home_detected: bool,
    /// web profile 的 package.json 存在（通常 = 至少跑过一次 `dsh web`）
    pub profile_ready: bool,
    /// 插件已安装为 profile 依赖（dependencies 含包名；link/git/npm 值均算）
    pub plugin_installed: bool,
    /// 插件已激活（dsh.profile.bundles 含包名，dsh 启动时会加载）
    pub plugin_activated: bool,
}

/// 检测 `dsh_home`（生产传 `~/.dsh`）下的集成状态。只读；package.json 缺失或解析
/// 失败都按未安装算——半成品态（装了依赖没激活）必须对用户可见而非静默合并。
pub fn detect(dsh_home: &Path) -> DshIntegrationStatus {
    let dsh_home_detected = dsh_home.is_dir();
    let package_json = profile_package_json(dsh_home);
    let profile_ready = package_json.is_file();
    let (plugin_installed, plugin_activated) = match std::fs::read_to_string(&package_json) {
        Ok(body) => parse_profile(&body),
        Err(_) => (false, false),
    };
    DshIntegrationStatus {
        dsh_home_detected,
        profile_ready,
        plugin_installed,
        plugin_activated,
    }
}

/// 解析 profile package.json：`dependencies` 含包名 → installed；
/// `dsh.profile.bundles` 数组含包名 → activated。字段缺失/类型漂移均按 false。
fn parse_profile(body: &str) -> (bool, bool) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return (false, false);
    };
    let installed = v
        .get("dependencies")
        .and_then(|d| d.get(PLUGIN_PACKAGE))
        .is_some();
    let activated = v
        .pointer("/dsh/profile/bundles")
        .and_then(|b| b.as_array())
        .is_some_and(|bundles| bundles.iter().any(|e| e.as_str() == Some(PLUGIN_PACKAGE)));
    (installed, activated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 在临时目录搭出 `<root>/.dsh/profiles/web/package.json`，返回 dsh_home 路径。
    fn make_home(root: &TempDir, package_json: Option<&str>) -> std::path::PathBuf {
        let home = root.path().join(".dsh");
        std::fs::create_dir_all(home.join("profiles/web")).unwrap();
        if let Some(body) = package_json {
            std::fs::write(home.join("profiles/web/package.json"), body).unwrap();
        }
        home
    }

    /// 全装（npm 注册值）：依赖键 + bundles 数组各含包名。
    const FULL_JSON: &str = r#"{
        "name": "dsh-profile-web",
        "dependencies": { "@zapmomo-ai/dsh-plugin": "0.1.0" },
        "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "@zapmomo-ai/dsh-plugin"] } }
    }"#;

    #[test]
    fn test_detect_all_missing() {
        let root = TempDir::new().unwrap();
        let s = detect(&root.path().join(".dsh"));
        assert!(!s.dsh_home_detected);
        assert!(!s.profile_ready);
        assert!(!s.plugin_installed);
        assert!(!s.plugin_activated);
    }

    #[test]
    fn test_detect_home_only() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join(".dsh")).unwrap();
        let s = detect(&root.path().join(".dsh"));
        assert!(s.dsh_home_detected);
        assert!(
            !s.profile_ready,
            "无 web profile 时 profile_ready 应为 false"
        );
        assert!(!s.plugin_installed);
        assert!(!s.plugin_activated);
    }

    #[test]
    fn test_detect_profile_without_plugin() {
        let root = TempDir::new().unwrap();
        let home = make_home(&root, Some(r#"{"dependencies":{}}"#));
        let s = detect(&home);
        assert!(s.dsh_home_detected);
        assert!(s.profile_ready);
        assert!(!s.plugin_installed);
        assert!(!s.plugin_activated);
    }

    #[test]
    fn test_detect_half_installed_without_bundles() {
        // 半成品态：装了依赖但 bundles 未激活（手动挂载漏第 2 步的典型现场）
        let root = TempDir::new().unwrap();
        let home = make_home(
            &root,
            Some(r#"{"dependencies":{"@zapmomo-ai/dsh-plugin":"0.1.0"}}"#),
        );
        let s = detect(&home);
        assert!(s.plugin_installed);
        assert!(!s.plugin_activated);
    }

    #[test]
    fn test_detect_bundles_without_dependency() {
        // 反向半成品：patch 插了但依赖没装（loader import 不到）
        let root = TempDir::new().unwrap();
        let home = make_home(
            &root,
            Some(r#"{"dsh":{"profile":{"bundles":["@zapmomo-ai/dsh-plugin"]}}}"#),
        );
        let s = detect(&home);
        assert!(!s.plugin_installed);
        assert!(s.plugin_activated);
    }

    #[test]
    fn test_detect_full_with_npm_value() {
        let root = TempDir::new().unwrap();
        let home = make_home(&root, Some(FULL_JSON));
        let s = detect(&home);
        assert!(s.plugin_installed);
        assert!(s.plugin_activated);
    }

    #[test]
    fn test_detect_full_with_link_value() {
        // link 模式（本仓库开发调试同款）：值是本地路径，键存在即算已安装
        let root = TempDir::new().unwrap();
        let home = make_home(
            &root,
            Some(
                r#"{"dependencies":{"@zapmomo-ai/dsh-plugin":"link:/tmp/zapmomo/integrations/dsh-plugin"}}"#,
            ),
        );
        let s = detect(&home);
        assert!(s.plugin_installed);
        assert!(
            !s.plugin_activated,
            "link 装法 bundles 是否包含视 patch 而定"
        );
    }

    #[test]
    fn test_detect_broken_package_json() {
        // 解析失败宁可显示未安装（引导重装）也不误报已装
        let root = TempDir::new().unwrap();
        let home = make_home(&root, Some("not-json"));
        let s = detect(&home);
        assert!(s.profile_ready);
        assert!(!s.plugin_installed);
        assert!(!s.plugin_activated);
    }

    #[test]
    fn test_detect_package_json_missing_but_profile_dir_exists() {
        // profile 目录在但 package.json 缺失（pnpm add 中途崩溃等）：不算 ready
        let root = TempDir::new().unwrap();
        let home = make_home(&root, None);
        let s = detect(&home);
        assert!(!s.profile_ready);
        assert!(!s.plugin_installed);
    }

    #[test]
    fn test_constants_shape() {
        // 锁定契约：包名与手动命令的形态（前端复制按钮直出 MANUAL_COMMAND）
        assert_eq!(PLUGIN_PACKAGE, "@zapmomo-ai/dsh-plugin");
        assert!(MANUAL_COMMAND.contains(PLUGIN_PACKAGE));
        assert!(MANUAL_COMMAND.contains("--profile web"));
    }
}
