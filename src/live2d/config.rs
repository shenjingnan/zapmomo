//! Live2D 模型配置解析与目录扫描。
//!
//! 把 `settings.toml` 的 `[live2d]` 表解析成 `ResolvedLive2dConfig`，
//! 并在用户选择的目录里定位模型清单文件（`.model3.json` / `model.json`）。

use crate::config::settings::{Live2dSettings, resolve_env_ref};
use std::path::{Path, PathBuf};

/// Live2D 模型格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live2dFormat {
    /// Cubism 2（`.moc` + `model.json`），老旧格式。
    Cubism2,
    /// Cubism 3/4/5（`.moc3` + `.model3.json`）。
    Cubism3,
}

impl Live2dFormat {
    /// 转成给前端展示的字符串。
    pub fn to_str(self) -> &'static str {
        match self {
            Live2dFormat::Cubism2 => "cubism2",
            Live2dFormat::Cubism3 => "cubism3",
        }
    }
}

/// 解析后的 Live2D 配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLive2dConfig {
    /// 模型根目录。
    pub model_dir: PathBuf,
    /// 模型清单文件（`.model3.json` 或 `model.json`），未配置/未找到时为 `None`。
    pub model_file: Option<PathBuf>,
    /// 模型格式。
    pub format: Option<Live2dFormat>,
}

impl Default for ResolvedLive2dConfig {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            model_file: None,
            format: None,
        }
    }
}

/// 用户默认 Live2D 模型目录：`~/.zapmomo/models/live2d`。
///
/// data_dir 切换后主根尚无而旧根有存量时，回退旧根 `live2d` 子目录。
pub fn default_model_dir() -> PathBuf {
    let new = crate::config::settings::get_models_dir().join("live2d");
    if new.join("model.json").exists() || scan_for_model(&new).is_some() {
        return new;
    }
    if let Some(legacy) = crate::config::settings::legacy_models_dir() {
        let legacy_dir = legacy.join("live2d");
        if scan_for_model(&legacy_dir).is_some() {
            return legacy_dir;
        }
    }
    new
}

/// 在指定目录中定位模型清单文件。
///
/// 优先顶层扫描，找不到再递归一层子目录。匹配规则：
/// - `*.model3.json` → Cubism 3/4/5
/// - `model.json` → Cubism 2
pub fn find_model_file(dir: &Path) -> Option<(PathBuf, Live2dFormat)> {
    if let Some(found) = scan_for_model(dir) {
        return Some(found);
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let sub = entry.path();
        if sub.is_dir()
            && let Some(found) = scan_for_model(&sub)
        {
            return Some(found);
        }
    }
    None
}

/// 在单个目录里扫描模型清单文件（不递归）。
fn scan_for_model(dir: &Path) -> Option<(PathBuf, Live2dFormat)> {
    let mut cubism2: Option<PathBuf> = None;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name()?.to_string_lossy();
        if name.ends_with(".model3.json") {
            return Some((path, Live2dFormat::Cubism3));
        }
        if name == "model.json" && cubism2.is_none() {
            cubism2 = Some(path);
        }
    }
    cubism2.map(|p| (p, Live2dFormat::Cubism2))
}

/// 解析模型目录：优先 `settings` 中配置的目录（支持 `${env.VAR}` 与相对路径
/// 锚定配置目录），未配置时回退默认目录。
fn resolve_model_dir(settings: Option<&Live2dSettings>) -> Result<PathBuf, String> {
    if let Some(dir) = settings.and_then(|s| s.model_dir.as_deref()) {
        let expanded = resolve_env_ref(dir)?;
        let p = PathBuf::from(expanded);
        return Ok(if p.is_absolute() {
            p
        } else {
            crate::config::settings::get_settings_dir().join(p)
        });
    }
    Ok(default_model_dir())
}

/// 合并配置：解析模型目录并定位模型清单文件。
pub fn resolve(settings: Option<&Live2dSettings>) -> Result<ResolvedLive2dConfig, String> {
    let model_dir = resolve_model_dir(settings)?;
    let (model_file, format) = find_model_file(&model_dir)
        .map(|(p, f)| (Some(p), Some(f)))
        .unwrap_or((None, None));
    Ok(ResolvedLive2dConfig {
        model_dir,
        model_file,
        format,
    })
}

/// 轻量校验目录内的 Live2D 模型是否可基础加载。
///
/// 只解析 `*.model3.json` 的 `FileReferences`，硬校验 `Moc` 与全部 `Textures[]` 文件
/// 存在；**不初始化 PIXI / 不读取纹理字节**。路径相对 model3.json 所在目录解析，
/// 并拒绝越界（`..`）路径。Motions / Expressions / Physics 等可选资源不强制要求。
pub fn validate_managed_model(dir: &Path) -> Result<(), String> {
    let (model_file, format) = find_model_file(dir)
        .ok_or_else(|| "目录中未找到 Live2D 模型清单（*.model3.json 或 model.json）".to_string())?;
    if format == Live2dFormat::Cubism2 {
        return Err(
            "暂不支持 Cubism 2 模型（.moc + model.json），请使用 Cubism 3/4/5 模型（.moc3 + .model3.json）"
                .to_string(),
        );
    }

    let content =
        std::fs::read_to_string(&model_file).map_err(|e| format!("读取模型清单失败: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("模型清单 JSON 解析失败: {e}"))?;
    let base = model_file.parent().unwrap_or_else(|| Path::new("."));
    let file_refs = json
        .get("FileReferences")
        .ok_or_else(|| "模型清单缺少 FileReferences".to_string())?;

    // Moc 文件必须存在。
    let moc = file_refs
        .get("Moc")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "模型清单缺少 FileReferences.Moc".to_string())?;
    let moc_path = resolve_in(base, moc)?;
    if !moc_path.is_file() {
        return Err(format!("模型缺失 Moc 文件: {}", moc_path.display()));
    }

    // Textures 全部必须存在（空数组也算合法清单，但缺失字段则拒绝）。
    let textures = file_refs
        .get("Textures")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "模型清单缺少 FileReferences.Textures".to_string())?;
    for texture in textures {
        let Some(name) = texture.as_str() else {
            continue;
        };
        let tex_path = resolve_in(base, name)?;
        if !tex_path.is_file() {
            return Err(format!("模型缺失纹理文件: {}", tex_path.display()));
        }
    }

    Ok(())
}

/// 在模型根目录探测一张「封面/预览图」（best-effort）。
///
/// Live2D 模型本身没有封面概念（`model3.json` 只含 Moc / Textures），但很多模型包会
/// 附带一张预览图（`preview` / `thumbnail` / `cover` / `icon` …）。这里只扫顶层文件，
/// 排除明显是贴图 / 清单 / moc 的文件：优先常见封面关键词命名，其次「唯一的根目录图片」。
pub fn find_cover_image(dir: &Path) -> Option<PathBuf> {
    const KEYWORDS: &[&str] = &[
        "thumbnail",
        "thumb",
        "preview",
        "cover",
        "icon",
        "sample",
        "eyecatch",
        "illust",
        "sketch",
        "poster",
        "card",
        "stand",
    ];
    const EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    let mut keyword_match: Option<PathBuf> = None;
    let mut generic: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|s| s.to_string_lossy().to_lowercase()) else {
            continue;
        };
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
            continue;
        };
        // 排除明显是贴图 / 清单 / moc 的文件。
        let looks_like_texture = name.contains("texture")
            || name.contains("tex_")
            || name.ends_with(".moc3")
            || name.ends_with(".model3.json")
            || name == "model.json"
            || stem.chars().all(|c| c.is_ascii_digit());
        if looks_like_texture {
            continue;
        }
        let is_image = EXTS.iter().any(|ext| name.ends_with(&format!(".{ext}")));
        if !is_image {
            continue;
        }
        if keyword_match.is_none() && KEYWORDS.iter().any(|k| name.contains(k)) {
            keyword_match = Some(path);
        } else {
            generic.push(path);
        }
    }

    keyword_match.or_else(|| (generic.len() == 1).then(|| generic[0].clone()))
}

/// 道具键所属的手。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    Left,
    Right,
}

/// 一个按键贴图（爪子按在某键上的预渲染图）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BongoCatKey {
    /// 键名（文件名第一个 `.` 之前，如 `KeyA`、`CapsLock`）。
    pub key: String,
    /// 贴图文件路径。
    pub path: PathBuf,
    /// 所属的手。
    pub hand: Hand,
}

/// BongoCat 格式道具资源探测结果。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct BongoCatProps {
    /// 键盘背景图（`resources/background.png`）。
    pub background: Option<PathBuf>,
    /// 按键贴图清单（left-keys + right-keys）。
    pub keys: Vec<BongoCatKey>,
}

/// 探测模型目录是否带 BongoCat 格式道具资源。
///
/// 判定标准与 BongoCat 资源包规范一致：`resources/background.png` 存在，且
/// `left-keys/` / `right-keys/` 至少一个目录含图片。图片扩展白名单与 BongoCat
/// `isImage` 对齐；键名取文件名第一个 `.` 之前的部分（`KeyA.png` → `KeyA`）。
///
/// `model_dir` 应为模型清单（`*.model3.json`）所在目录（BongoCat 模型的
/// `resources/` 与 model3.json 平级），而非托管根目录。
pub fn detect_bongocat(model_dir: &Path) -> Option<BongoCatProps> {
    const IMAGE_EXTS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "avif", "gif", "svg", "bmp", "ico", "tif", "tiff", "heic",
        "apng",
    ];

    let resources = model_dir.join("resources");
    let background = resources.join("background.png");
    let background = background.is_file().then_some(background);

    let mut keys = Vec::new();
    for (hand, dir_name) in [(Hand::Left, "left-keys"), (Hand::Right, "right-keys")] {
        let dir = resources.join(dir_name);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let lower = name.to_lowercase();
            if !IMAGE_EXTS
                .iter()
                .any(|ext| lower.ends_with(&format!(".{ext}")))
            {
                continue;
            }
            // 键名 = 文件名第一个 `.` 之前（与 BongoCat `name.split('.')[0]` 对齐）
            let key = name.split('.').next().unwrap_or(&name).to_string();
            if key.is_empty() {
                continue;
            }
            keys.push(BongoCatKey { key, path, hand });
        }
    }

    if background.is_none() || keys.is_empty() {
        return None;
    }
    Some(BongoCatProps { background, keys })
}

/// 把清单里的相对引用解析到 model3.json 所在目录，拒绝绝对路径与 `..` 越界。
///
/// 注意 `Path::starts_with` 是纯词法比较、不规范化 `..`，因此不能依赖它做越界防护；
/// 这里显式检查路径组件。
fn resolve_in(base: &Path, rel: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("模型清单包含越界路径: {rel}"));
    }
    Ok(base.join(rel))
}

/// 一个可播放动作（菜单展示名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionInfo {
    /// 展示名：File basename 去掉 `.motion3.json`（与前端 previewManager 口径一致）。
    pub name: String,
}

/// 一个动作组（model3.json `FileReferences.Motions` 的一个键）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotionGroupInfo {
    /// 组名（如 `Idle` / `TapBody` / `Extra`）。
    pub group: String,
    /// 组内动作；**数组位置即前端 `motionManager` 的播放下标**，不可重排/过滤。
    pub motions: Vec<MotionInfo>,
}

/// 动作展示名：File basename 去掉 `.motion3.json` 扩展。
fn motion_display_name(file: &str) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    name.strip_suffix(".motion3.json")
        .unwrap_or(name)
        .to_string()
}

/// 解析模型清单的可播放动作目录（`FileReferences.Motions`）。
///
/// 供右键菜单「状态切换」列出动作：播放由前端按 (组名, 组内下标) 调
/// `motionManager.startMotion`，因此这里**不需要**把 File 解析成绝对路径，
/// 但下标必须与清单数组逐位对齐（缺 `File` 的项用占位名保位置，前端播放
/// 时自然失败，与清单本身的病态一致）。
///
/// - 缺 `Motions` 键 → `Ok(vec![])`（可选资源，非错误）；
/// - 空组 / 非数组的组值跳过（构建菜单与点击解析共用本结果，自洽）；
/// - 组间顺序为清单 JSON 的键序（serde_json 默认按字母序）。
pub fn parse_motion_catalog(model_file: &Path) -> Result<Vec<MotionGroupInfo>, String> {
    let content =
        std::fs::read_to_string(model_file).map_err(|e| format!("读取模型清单失败: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("模型清单 JSON 解析失败: {e}"))?;
    let Some(groups) = json
        .get("FileReferences")
        .and_then(|refs| refs.get("Motions"))
        .and_then(|m| m.as_object())
    else {
        return Ok(Vec::new());
    };
    let mut catalog = Vec::new();
    for (group, defs) in groups {
        let Some(defs) = defs.as_array() else {
            continue;
        };
        let motions: Vec<MotionInfo> = defs
            .iter()
            .map(|d| MotionInfo {
                name: d
                    .get("File")
                    .and_then(|f| f.as_str())
                    .map(motion_display_name)
                    .unwrap_or_else(|| "（未命名）".to_string()),
            })
            .collect();
        if motions.is_empty() {
            continue;
        }
        catalog.push(MotionGroupInfo {
            group: group.clone(),
            motions,
        });
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    /// 在临时目录下创建最小 Live2D 模型骨架（仅清单文件，不校验 moc3 内容）。
    fn make_model(dir: &Path, manifest_name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let manifest = dir.join(manifest_name);
        std::fs::write(&manifest, "{}").unwrap();
        manifest
    }

    #[test]
    fn test_default_model_dir_dual_root_fallback() {
        run_with_temp_home(|home| {
            let data = crate::test_util::set_custom_data_dir(home);
            let legacy_dir = home.join(".zapmomo/models/live2d");
            let new_dir = data.join("models/live2d");

            // 只有旧根有模型 → 默认目录回退旧根
            make_model(&legacy_dir, "cat.model3.json");
            assert_eq!(default_model_dir(), legacy_dir);

            // 新根有 → 新根优先
            make_model(&new_dir, "cat.model3.json");
            assert_eq!(default_model_dir(), new_dir);
        });
    }

    #[test]
    fn test_find_model_file_top_level_model3() {
        run_with_temp_home(|home| {
            let dir = home.join("m1");
            make_model(&dir, "火花.model3.json");
            let (path, fmt) = find_model_file(&dir).unwrap();
            assert!(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".model3.json")
            );
            assert_eq!(fmt, Live2dFormat::Cubism3);
        });
    }

    #[test]
    fn test_find_model_file_top_level_model_json() {
        run_with_temp_home(|home| {
            let dir = home.join("m2");
            make_model(&dir, "model.json");
            let (_, fmt) = find_model_file(&dir).unwrap();
            assert_eq!(fmt, Live2dFormat::Cubism2);
        });
    }

    #[test]
    fn test_find_model_file_prefers_model3() {
        run_with_temp_home(|home| {
            let dir = home.join("m3");
            make_model(&dir, "model.json");
            make_model(&dir, "a.model3.json");
            let (_, fmt) = find_model_file(&dir).unwrap();
            assert_eq!(fmt, Live2dFormat::Cubism3);
        });
    }

    #[test]
    fn test_find_model_file_recurses_one_level() {
        run_with_temp_home(|home| {
            let dir = home.join("outer");
            let sub = dir.join("inner");
            make_model(&sub, "m.model3.json");
            let (path, fmt) = find_model_file(&dir).unwrap();
            assert_eq!(fmt, Live2dFormat::Cubism3);
            assert_eq!(path, sub.join("m.model3.json"));
        });
    }

    #[test]
    fn test_find_model_file_missing() {
        run_with_temp_home(|home| {
            let dir = home.join("empty");
            std::fs::create_dir_all(&dir).unwrap();
            assert!(find_model_file(&dir).is_none());
        });
    }

    #[test]
    fn test_resolve_default_dir() {
        run_with_temp_home(|home| {
            let cfg = resolve(None).unwrap();
            assert_eq!(cfg.model_dir, home.join(".zapmomo/models/live2d"));
            assert!(cfg.model_file.is_none());
        });
    }

    #[test]
    fn test_resolve_custom_dir() {
        run_with_temp_home(|home| {
            let dir = home.join("custom");
            make_model(&dir, "c.model3.json");
            let settings = Live2dSettings {
                model_dir: Some(dir.display().to_string()),
                ..Default::default()
            };
            let cfg = resolve(Some(&settings)).unwrap();
            assert_eq!(cfg.model_dir, dir);
            assert!(cfg.model_file.is_some());
            assert_eq!(cfg.format, Some(Live2dFormat::Cubism3));
        });
    }

    #[test]
    fn test_format_to_str() {
        assert_eq!(Live2dFormat::Cubism2.to_str(), "cubism2");
        assert_eq!(Live2dFormat::Cubism3.to_str(), "cubism3");
    }

    /// 构造一个结构合法的 Cubism3 模型目录（model3.json + moc3 + textures）。
    fn make_valid_model(dir: &Path) {
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("model.moc3"), b"moc").unwrap();
        std::fs::write(dir.join("textures/texture_00.png"), b"png").unwrap();
        std::fs::write(
            dir.join("foo.model3.json"),
            r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"]}}"#,
        )
        .unwrap();
    }

    #[test]
    fn test_validate_managed_model_ok() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            make_valid_model(&dir);
            validate_managed_model(&dir).unwrap();
        });
    }

    #[test]
    fn test_validate_managed_model_missing_moc() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(dir.join("textures")).unwrap();
            std::fs::write(dir.join("textures/texture_00.png"), b"png").unwrap();
            std::fs::write(
                dir.join("foo.model3.json"),
                r#"{"FileReferences":{"Moc":"nope.moc3","Textures":["textures/texture_00.png"]}}"#,
            )
            .unwrap();
            let err = validate_managed_model(&dir).unwrap_err();
            assert!(err.contains("Moc"), "错误应指出缺失 Moc: {err}");
        });
    }

    #[test]
    fn test_validate_managed_model_missing_texture() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("model.moc3"), b"moc").unwrap();
            std::fs::write(
                dir.join("foo.model3.json"),
                r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/missing.png"]}}"#,
            )
            .unwrap();
            let err = validate_managed_model(&dir).unwrap_err();
            assert!(err.contains("纹理"), "错误应指出缺失纹理: {err}");
        });
    }

    #[test]
    fn test_validate_managed_model_cubism2_rejected() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("model.json"), "{}").unwrap();
            let err = validate_managed_model(&dir).unwrap_err();
            assert!(err.contains("Cubism 2"), "应拒绝 Cubism2: {err}");
        });
    }

    #[test]
    fn test_find_cover_image_prefers_keyword() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("texture_00.png"), b"tex").unwrap();
            std::fs::write(dir.join("preview.png"), b"cover").unwrap();
            std::fs::write(dir.join("foo.png"), b"other").unwrap();
            let cover = find_cover_image(&dir).unwrap();
            assert_eq!(cover.file_name().unwrap().to_string_lossy(), "preview.png");
        });
    }

    #[test]
    fn test_find_cover_image_single_root_image() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("sample_img.png"), b"only").unwrap();
            let cover = find_cover_image(&dir).unwrap();
            assert_eq!(
                cover.file_name().unwrap().to_string_lossy(),
                "sample_img.png"
            );
        });
    }

    #[test]
    fn test_find_cover_image_none_when_ambiguous() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("a.png"), b"1").unwrap();
            std::fs::write(dir.join("b.png"), b"2").unwrap();
            assert!(find_cover_image(&dir).is_none());
        });
    }

    #[test]
    fn test_find_cover_image_none_when_only_textures() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(dir.join("textures")).unwrap();
            std::fs::write(dir.join("textures/texture_00.png"), b"tex").unwrap();
            std::fs::write(dir.join("00.png"), b"numeric tex").unwrap();
            std::fs::write(dir.join("model.moc3"), b"moc").unwrap();
            assert!(find_cover_image(&dir).is_none());
        });
    }

    #[test]
    fn test_validate_managed_model_traversal_rejected() {
        run_with_temp_home(|home| {
            let dir = home.join("m");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("foo.model3.json"),
                r#"{"FileReferences":{"Moc":"../outside.moc3","Textures":[]}}"#,
            )
            .unwrap();
            let err = validate_managed_model(&dir).unwrap_err();
            assert!(err.contains("越界"), "应拒绝越界路径: {err}");
        });
    }

    // ---------- detect_bongocat ----------

    /// 在临时目录搭建 BongoCat 风格资源骨架（resources/background.png + 指定贴图）。
    fn make_bongocat_resources(dir: &Path, left: &[&str], right: &[&str]) {
        std::fs::create_dir_all(dir.join("resources")).unwrap();
        std::fs::write(dir.join("resources/background.png"), b"bg").unwrap();
        for key in left {
            std::fs::create_dir_all(dir.join("resources/left-keys")).unwrap();
            std::fs::write(dir.join(format!("resources/left-keys/{key}.png")), b"img").unwrap();
        }
        for key in right {
            std::fs::create_dir_all(dir.join("resources/right-keys")).unwrap();
            std::fs::write(dir.join(format!("resources/right-keys/{key}.png")), b"img").unwrap();
        }
    }

    #[test]
    fn test_detect_bongocat_full_model() {
        let dir = tempfile::tempdir().unwrap();
        make_bongocat_resources(dir.path(), &["KeyA", "KeyB"], &["ShiftLeft"]);
        let props = detect_bongocat(dir.path()).unwrap();
        assert_eq!(
            props.background,
            Some(dir.path().join("resources/background.png"))
        );
        assert_eq!(props.keys.len(), 3);
        let a = props.keys.iter().find(|k| k.key == "KeyA").unwrap();
        assert_eq!(a.hand, Hand::Left);
        assert!(a.path.ends_with("left-keys/KeyA.png"));
        let s = props.keys.iter().find(|k| k.key == "ShiftLeft").unwrap();
        assert_eq!(s.hand, Hand::Right);
    }

    #[test]
    fn test_detect_bongocat_left_only_ok() {
        // standard 模式：只有左手键（右手在鼠标上），仍应判定为 BongoCat
        let dir = tempfile::tempdir().unwrap();
        make_bongocat_resources(dir.path(), &["KeyA"], &[]);
        assert!(detect_bongocat(dir.path()).is_some());
    }

    #[test]
    fn test_detect_bongocat_missing_background_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources/left-keys")).unwrap();
        std::fs::write(dir.path().join("resources/left-keys/KeyA.png"), b"img").unwrap();
        // 无 background.png → 不判定
        assert!(detect_bongocat(dir.path()).is_none());
    }

    #[test]
    fn test_detect_bongocat_no_keys_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources")).unwrap();
        std::fs::write(dir.path().join("resources/background.png"), b"bg").unwrap();
        // 有背景但无 keys 目录 → 不判定
        assert!(detect_bongocat(dir.path()).is_none());
    }

    #[test]
    fn test_detect_bongocat_filters_non_image_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources/left-keys")).unwrap();
        std::fs::write(dir.path().join("resources/background.png"), b"bg").unwrap();
        std::fs::write(dir.path().join("resources/left-keys/KeyA.png"), b"img").unwrap();
        std::fs::write(dir.path().join("resources/left-keys/readme.txt"), b"text").unwrap();
        let props = detect_bongocat(dir.path()).unwrap();
        assert_eq!(props.keys.len(), 1, "txt 不应计入");
        assert_eq!(props.keys[0].key, "KeyA");
    }

    #[test]
    fn test_detect_bongocat_key_name_takes_first_dot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources/left-keys")).unwrap();
        std::fs::write(dir.path().join("resources/background.png"), b"bg").unwrap();
        std::fs::write(dir.path().join("resources/left-keys/foo.bar.png"), b"img").unwrap();
        let props = detect_bongocat(dir.path()).unwrap();
        assert_eq!(
            props.keys[0].key, "foo",
            "键名取第一个点之前，与 BongoCat split('.')[0] 对齐"
        );
    }

    #[test]
    fn test_detect_bongocat_extension_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("resources/left-keys")).unwrap();
        std::fs::write(dir.path().join("resources/background.png"), b"bg").unwrap();
        std::fs::write(dir.path().join("resources/left-keys/KeyA.PNG"), b"img").unwrap();
        let props = detect_bongocat(dir.path()).unwrap();
        assert_eq!(props.keys.len(), 1);
        assert_eq!(props.keys[0].key, "KeyA");
    }

    #[test]
    fn test_detect_bongocat_empty_dir_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_bongocat(dir.path()).is_none());
    }

    #[test]
    fn test_bongocat_props_serializes_snake_case_hand() {
        let props = BongoCatProps {
            background: Some(PathBuf::from("/x/resources/background.png")),
            keys: vec![BongoCatKey {
                key: "KeyA".to_string(),
                path: PathBuf::from("/x/resources/left-keys/KeyA.png"),
                hand: Hand::Left,
            }],
        };
        let s = serde_json::to_string(&props).unwrap();
        assert!(
            s.contains(r#""hand":"left""#),
            "hand 应序列化为 snake_case: {s}"
        );
        assert!(s.contains(r#""key":"KeyA""#), "key 字段: {s}");
    }

    /// 写一个带 Motions 清单的模型文件，返回清单路径。
    fn make_model_with_motions(dir: &Path, manifest: &str, motions: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(manifest);
        std::fs::write(
            &path,
            format!(r#"{{ "FileReferences": {{ "Motions": {motions} }} }}"#),
        )
        .unwrap();
        path
    }

    #[test]
    fn test_parse_motion_catalog_groups_and_names() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = make_model_with_motions(
            dir.path(),
            "hi.model3.json",
            r#"{
                "Idle": [{ "File": "motions/idle_01.motion3.json" }],
                "TapBody": [
                    { "File": "motions/wave.motion3.json" },
                    { "File": "motions\\win\\wave.motion3.json" }
                ]
            }"#,
        );
        let catalog = parse_motion_catalog(&manifest).unwrap();
        assert_eq!(catalog.len(), 2, "两个非空组");
        let idle = catalog.iter().find(|g| g.group == "Idle").unwrap();
        assert_eq!(idle.motions.len(), 1);
        assert_eq!(idle.motions[0].name, "idle_01");
        let tap = catalog.iter().find(|g| g.group == "TapBody").unwrap();
        assert_eq!(
            tap.motions[1].name, "wave",
            "Windows 反斜杠分隔 + 空格路径同样取 basename 去扩展"
        );
    }

    #[test]
    fn test_parse_motion_catalog_keeps_index_alignment() {
        // 缺 File 的项用占位名保位置：下标必须与清单数组逐位对齐（播放按 index）。
        let dir = tempfile::tempdir().unwrap();
        let manifest = make_model_with_motions(
            dir.path(),
            "hi.model3.json",
            r#"{ "G": [{ "File": "a.motion3.json" }, {}, { "Sound": "x.wav" }] }"#,
        );
        let catalog = parse_motion_catalog(&manifest).unwrap();
        let g = &catalog[0];
        assert_eq!(g.motions.len(), 3, "缺 File 的项不剔除，保下标对齐");
        assert_eq!(g.motions[0].name, "a");
        assert_eq!(g.motions[1].name, "（未命名）");
        assert_eq!(g.motions[2].name, "（未命名）");
    }

    #[test]
    fn test_parse_motion_catalog_skips_empty_and_invalid_groups() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = make_model_with_motions(
            dir.path(),
            "hi.model3.json",
            r#"{ "Empty": [], "NotArray": 3, "Ok": [{ "File": "m.motion3.json" }] }"#,
        );
        let catalog = parse_motion_catalog(&manifest).unwrap();
        assert_eq!(catalog.len(), 1, "空组与非数组组跳过");
        assert_eq!(catalog[0].group, "Ok");
    }

    #[test]
    fn test_parse_motion_catalog_missing_motions_key_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let manifest = dir.path().join("hi.model3.json");
        std::fs::write(&manifest, r#"{ "FileReferences": { "Moc": "a.moc3" } }"#).unwrap();
        assert!(parse_motion_catalog(&manifest).unwrap().is_empty());

        // 完全空对象同样为空（可选资源语义）
        let bare = dir.path().join("bare.model3.json");
        std::fs::write(&bare, "{}").unwrap();
        assert!(parse_motion_catalog(&bare).unwrap().is_empty());
    }

    #[test]
    fn test_parse_motion_catalog_malformed_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let manifest = dir.path().join("bad.model3.json");
        std::fs::write(&manifest, "not json at all").unwrap();
        let err = parse_motion_catalog(&manifest).unwrap_err();
        assert!(err.contains("解析"), "{err}");
    }

    #[test]
    fn test_parse_motion_catalog_missing_file_errors() {
        let err = parse_motion_catalog(Path::new("/nonexistent/a.model3.json")).unwrap_err();
        assert!(err.contains("读取"), "{err}");
    }

    #[test]
    fn test_parse_motion_catalog_chinese_display_name() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = make_model_with_motions(
            dir.path(),
            "hi.model3.json",
            r#"{ "Extra": [{ "File": "motions/跳舞.motion3.json" }] }"#,
        );
        let catalog = parse_motion_catalog(&manifest).unwrap();
        assert_eq!(catalog[0].motions[0].name, "跳舞");
    }
}
