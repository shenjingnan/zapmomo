//! 角色包形象（sprites/）：枚举可用形象 + LLM 工具执行入口 + 切换事件通知。
//!
//! 约定：角色包托管目录内可选 `sprites/` 子目录，图片文件名 stem 即形象语义
//! （如 `happy.png` → 「开心」），供 LLM 通过 `set_character_sprite` 工具切换。
//!
//! - 枚举：一层目录、`png`/`gif`/`webp`，stem 冲突按 png > gif > webp 取优先；
//! - 执行：名字与枚举 stem 做大小写不敏感精确匹配，**从不拼接路径**，
//!   路径穿越在构造上不可能；`default` 恢复默认立绘（character.png）；
//! - 通知：经全局通道发给宿主（src-tauri 转发为 `companion-sprite-changed` 事件），
//!   未注册时静默跳过（CLI / 测试环境）。
//!
//! 形象是会话态：不持久化，重启 / 切换伙伴由前端回退默认立绘。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Sender;

use serde::Serialize;

/// 形象目录名（托管目录内约定，不入库、不参与导入校验）。
const SPRITES_DIR: &str = "sprites";
/// 恢复默认立绘的保留名。
pub const DEFAULT_SPRITE_NAME: &str = "default";

/// 支持的图片扩展名（小写）；stem 冲突按数组顺序取优先。
const SPRITE_EXTS: &[&str] = &["png", "gif", "webp"];

/// 形象切换事件载荷（src-tauri 侧转发为 `companion-sprite-changed`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpriteEvent {
    /// 事件所属伙伴 id（前端据此校验是否当前展示的伙伴）。
    pub companion_id: String,
    /// 形象名（stem）；`default` 表示恢复默认立绘。
    pub name: String,
    /// 图片绝对路径（default 时为 character.png）。
    pub path: String,
}

/// 一个可用形象。
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteInfo {
    /// 文件名 stem（如 `happy`，保留原大小写）。
    pub name: String,
    /// 图片绝对路径。
    pub path: PathBuf,
}

/// 全局通知通道。宿主（src-tauri）setup 时注册；None = 无接收方（CLI / 测试）。
static NOTIFIER: Mutex<Option<Sender<SpriteEvent>>> = Mutex::new(None);

/// 注册形象切换通知通道（覆盖旧值）。宿主进程内只应注册一次。
pub fn register_notifier(tx: Sender<SpriteEvent>) {
    *NOTIFIER.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
}

#[cfg(test)]
pub(crate) fn reset_notifier_for_test() {
    *NOTIFIER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// best-effort 发送：无注册者 / 接收端已关闭都只记日志，绝不影响工具执行。
fn notify(ev: SpriteEvent) {
    let guard = NOTIFIER.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(tx) => {
            if let Err(e) = tx.send(ev) {
                tracing::warn!("形象切换通知发送失败（接收端已关闭？）: {e}");
            }
        }
        None => tracing::debug!("形象切换无通知接收方，跳过（{}）", ev.name),
    }
}

/// active 的角色包伙伴（无 active / 非角色包 → None）。
fn active_character() -> Option<crate::companion::CompanionModel> {
    let lib = crate::companion::load_library_fast()
        .map_err(|e| {
            tracing::warn!("读取伙伴库失败（跳过形象探测）: {e}");
            e
        })
        .ok()?;
    let model = crate::companion::active_model(&lib)?;
    crate::companion::is_character(model).then(|| model.clone())
}

/// 枚举 active 角色包的可用形象（stem 升序）。非角色包 / 目录缺失 → 空。
pub fn list_active_sprites() -> Vec<SpriteInfo> {
    let Some(model) = active_character() else {
        return Vec::new();
    };
    list_sprites_in(Path::new(&model.model_dir))
}

/// 枚举托管目录 `sprites/` 下的一层图片文件。
///
/// 只接受 [`SPRITE_EXTS`] 列出的扩展名（大小写不敏感）；同名 stem 按
/// png > gif > webp 优先只保留一个；跳过子目录、隐藏文件与非 UTF-8 文件名。
fn list_sprites_in(model_dir: &Path) -> Vec<SpriteInfo> {
    let dir = model_dir.join(SPRITES_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    // (stem, 扩展名优先级, 路径)；同 stem 保留优先级更高（position 更小）者
    let mut best: Vec<(String, usize, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let Some(priority) = ext.and_then(|e| SPRITE_EXTS.iter().position(|x| *x == e)) else {
            continue;
        };
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.is_empty() || stem.starts_with('.') {
            continue;
        }
        match best.iter_mut().find(|(s, _, _)| s == stem) {
            Some(slot) => {
                if priority < slot.1 {
                    slot.1 = priority;
                    slot.2 = path;
                }
            }
            None => best.push((stem.to_string(), priority, path)),
        }
    }
    best.sort_by(|a, b| a.0.cmp(&b.0));
    best.into_iter()
        .map(|(name, _, path)| SpriteInfo { name, path })
        .collect()
}

/// `set_character_sprite` 工具执行入口：解析参数 → 校验 → 通知 → 返回结果文本。
///
/// 模型可见的失败一律返回提示文本（失败即结果），绝不 `Err` 中断 Agent Loop。
pub fn apply_tool_call(arguments: &str) -> String {
    let name = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|v| v.get("name")?.as_str().map(str::to_string));
    let Some(name) = name else {
        return "参数错误：缺少字符串字段 name".to_string();
    };
    let name = name.trim();
    if name.is_empty() {
        return "参数错误：name 不能为空".to_string();
    }

    let Some(model) = active_character() else {
        return "当前没有使用角色包伙伴，无法切换形象".to_string();
    };

    let sprites = list_sprites_in(Path::new(&model.model_dir));
    let (final_name, path) = if name == DEFAULT_SPRITE_NAME {
        // 默认立绘 = character.png（角色包导入时 model_file 即指向它）。
        (
            DEFAULT_SPRITE_NAME.to_string(),
            PathBuf::from(&model.model_file),
        )
    } else {
        let Some(hit) = sprites
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .cloned()
        else {
            return match sprites.is_empty() {
                true => format!("未找到形象「{name}」，当前角色没有可用形象"),
                false => {
                    let names: Vec<&str> = sprites.iter().map(|s| s.name.as_str()).collect();
                    format!(
                        "未找到形象「{name}」。可用形象：{}（或 default 恢复默认立绘）",
                        names.join(", ")
                    )
                }
            };
        };
        (hit.name, hit.path)
    };

    notify(SpriteEvent {
        companion_id: model.id.clone(),
        name: final_name.clone(),
        path: path.display().to_string(),
    });
    format!("已切换形象：{final_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;
    use std::sync::mpsc;
    use std::time::Duration;

    /// 构造最小合法角色包（character.md + character.png）。
    fn make_character_pack(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("character.md"), "# 芙宁娜\n\n你是芙宁娜。\n").unwrap();
        std::fs::write(dir.join("character.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
    }

    /// 补 sprites/ 目录：三张 png + 一个非图片文件（应被忽略）。
    fn add_sprites(dir: &Path) {
        std::fs::create_dir_all(dir.join("sprites")).unwrap();
        std::fs::write(dir.join("sprites/happy.png"), b"png").unwrap();
        std::fs::write(dir.join("sprites/angry.png"), b"png").unwrap();
        std::fs::write(dir.join("sprites/sad.png"), b"png").unwrap();
        std::fs::write(dir.join("sprites/notes.txt"), b"not-an-image").unwrap();
    }

    /// 导入带 sprites 的角色包并设为 active，返回托管目录。
    fn import_active_pack_with_sprites(home: &Path) -> std::path::PathBuf {
        let src = home.join("furina");
        make_character_pack(&src);
        add_sprites(&src);
        let (model, _) = crate::companion::import_character_from_dir(&src).unwrap();
        PathBuf::from(model.model_dir)
    }

    #[test]
    fn test_list_active_sprites_sorted_stems_ignores_non_images() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let sprites = list_active_sprites();
            let names: Vec<&str> = sprites.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(
                names,
                vec!["angry", "happy", "sad"],
                "stem 升序、忽略非图片"
            );
            assert!(sprites[1].path.ends_with("sprites/happy.png"));
        });
    }

    #[test]
    fn test_list_sprites_supports_png_gif_webp_with_priority() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_character_pack(&src);
            std::fs::create_dir_all(src.join("sprites")).unwrap();
            std::fs::write(src.join("sprites/happy.png"), b"png").unwrap();
            std::fs::write(src.join("sprites/happy.gif"), b"gif").unwrap();
            std::fs::write(src.join("sprites/wave.webp"), b"webp").unwrap();
            std::fs::write(src.join("sprites/bounce.gif"), b"gif").unwrap();
            // 大写扩展名同样识别
            std::fs::write(src.join("sprites/angry.PNG"), b"png").unwrap();
            crate::companion::import_character_from_dir(&src).unwrap();

            let sprites = list_active_sprites();
            let names: Vec<&str> = sprites.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, vec!["angry", "bounce", "happy", "wave"]);
            // 同 stem 冲突取 png 优先
            assert!(sprites[2].path.ends_with("happy.png"));
        });
    }

    #[test]
    fn test_list_active_sprites_empty_cases() {
        run_with_temp_home(|home| {
            // 无 active
            assert!(list_active_sprites().is_empty());

            // 角色包但没有 sprites/ 目录
            let src = home.join("furina");
            make_character_pack(&src);
            crate::companion::import_character_from_dir(&src).unwrap();
            assert!(list_active_sprites().is_empty());

            // active 切到 GIF 伙伴
            let gif = home.join("舞.gif");
            std::fs::write(&gif, b"GIF89a\x01\x00\x01\x00\x00").unwrap();
            let (gif_model, _) = crate::companion::import_gif_from_file(&gif).unwrap();
            crate::companion::set_active(&gif_model.id).unwrap();
            assert!(list_active_sprites().is_empty());
        });
    }

    #[test]
    fn test_apply_switches_and_notifies() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let (tx, rx) = mpsc::channel();
            register_notifier(tx);

            let out = apply_tool_call(r#"{"name":"happy"}"#);
            assert!(out.contains("happy"), "应确认切换：{out}");
            let ev = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(ev.name, "happy");
            // ev.path 是 String（display 序列化），Windows 下分隔符为 `\`：
            // 子串 ends_with 会挂，转 Path 按组件比较（跨平台）。
            assert!(
                std::path::Path::new(&ev.path)
                    .ends_with(std::path::Path::new("sprites").join("happy.png")),
                "ev.path: {}",
                ev.path
            );
            assert!(ev.companion_id.starts_with("companion-"));

            reset_notifier_for_test();
        });
    }

    #[test]
    fn test_apply_unknown_name_lists_available_without_event() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let (tx, rx) = mpsc::channel();
            register_notifier(tx);

            let out = apply_tool_call(r#"{"name":"shy"}"#);
            assert!(out.contains("未找到"), "{out}");
            assert!(out.contains("angry"), "应列出可用形象：{out}");
            assert!(out.contains("default"), "应提示 default：{out}");
            // 路径穿越尝试同样只是「未找到」，不拼接路径
            let out = apply_tool_call(r#"{"name":"../character.png"}"#);
            assert!(out.contains("未找到"), "{out}");
            assert!(rx.try_recv().is_err(), "失败的调用不应发出事件");

            reset_notifier_for_test();
        });
    }

    #[test]
    fn test_apply_default_restores_character_png() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let (tx, rx) = mpsc::channel();
            register_notifier(tx);

            let out = apply_tool_call(r#"{"name":"default"}"#);
            assert!(out.contains("default"), "{out}");
            let ev = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(ev.name, "default");
            assert!(
                ev.path.ends_with("character.png"),
                "默认立绘应为 character.png：{}",
                ev.path
            );

            reset_notifier_for_test();
        });
    }

    #[test]
    fn test_apply_matches_case_insensitively_and_reports_canonical_name() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            let (tx, rx) = mpsc::channel();
            register_notifier(tx);

            let out = apply_tool_call(r#"{"name":"  HAPPY  "}"#);
            assert!(out.contains("happy"), "应报告 stem 原名：{out}");
            let ev = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(ev.name, "happy", "事件用 stem 原名而非模型输入");

            reset_notifier_for_test();
        });
    }

    #[test]
    fn test_apply_bad_arguments() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            for args in ["{}", "not json", r#"{"name":"   "}"#, r#"{"name":123}"#] {
                let out = apply_tool_call(args);
                assert!(out.contains("参数错误"), "{args} → {out}");
            }
        });
    }

    #[test]
    fn test_apply_without_alive_receiver_still_succeeds() {
        run_with_temp_home(|home| {
            import_active_pack_with_sprites(home);
            // 注册后立刻丢弃接收端：send 失败被吞，工具仍成功
            let (tx, rx) = mpsc::channel();
            register_notifier(tx);
            drop(rx);

            let out = apply_tool_call(r#"{"name":"happy"}"#);
            assert!(out.contains("已切换"), "{out}");

            reset_notifier_for_test();
        });
    }

    #[test]
    fn test_apply_without_active_companion() {
        run_with_temp_home(|_home| {
            let out = apply_tool_call(r#"{"name":"happy"}"#);
            assert!(out.contains("没有"), "{out}");
        });
    }
}
