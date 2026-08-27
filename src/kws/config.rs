/// KWS 配置解析与校验。
///
/// 负责把 `settings.toml` 的 `[kws]` 表与 CLI flag 合并成一份已展开、已填默认值的
/// `ResolvedKwsConfig`。优先级：CLI `--model-dir` > settings > 内置默认。
use crate::config::settings::{KwsSettings, resolve_env_ref};
use std::path::{Path, PathBuf};

/// 模型包内默认文件名（chunk-16 变体，与官方测试命令一致）。
pub const DEFAULT_ENCODER: &str = "encoder-epoch-13-avg-2-chunk-16-left-64.onnx";
pub const DEFAULT_DECODER: &str = "decoder-epoch-13-avg-2-chunk-16-left-64.onnx";
pub const DEFAULT_JOINER: &str = "joiner-epoch-13-avg-2-chunk-16-left-64.onnx";
pub const DEFAULT_TOKENS: &str = "tokens.txt";
/// 模型包内自带的关键词文件（中英混合，开箱即用）。
pub const DEFAULT_KEYWORDS_REL: &str = "test_wavs/keywords.txt";

/// 解析后的完整 KWS 配置。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedKwsConfig {
    /// 是否启用 KWS（启动自动监听的前提），缺省 false
    pub enabled: bool,
    pub model_dir: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
    pub keywords_file: PathBuf,
    pub provider: String,
    pub num_threads: i32,
    /// 每次喂给模型的采样数（@sample_rate）
    pub chunk_size: usize,
    pub sample_rate: i32,
    pub keywords_score: f32,
    pub keywords_threshold: f32,
    pub debug: bool,
}

impl Default for ResolvedKwsConfig {
    fn default() -> Self {
        let model_dir = default_model_dir();
        let join = |name: &str| model_dir.join(name);
        Self {
            enabled: false,
            keywords_file: join(DEFAULT_KEYWORDS_REL),
            encoder: join(DEFAULT_ENCODER),
            decoder: join(DEFAULT_DECODER),
            joiner: join(DEFAULT_JOINER),
            tokens: join(DEFAULT_TOKENS),
            model_dir,
            provider: "cpu".to_string(),
            num_threads: 2,
            chunk_size: 3200,
            sample_rate: 16000,
            keywords_score: 1.0,
            keywords_threshold: 0.25,
            debug: false,
        }
    }
}

/// 用户默认模型目录：`~/.zapmomo/models/<模型名>`
pub fn user_default_model_dir() -> PathBuf {
    crate::kws::model::user_model_dir()
}

/// 源码仓库中的模型目录（开发者 `./models/<模型名>`，仅作开发回退）。
///
/// 打包后的 app 中该路径在用户机器上不存在，`choose_default_model_dir` 会因
/// `is_file()` 失败而回落，因此不会产生「CI 路径泄漏」。
fn repo_models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("models")
        .join(&crate::kws::model::default_asset().name)
}

/// 默认模型目录选择：用户已安装 > 旧默认根存量（data_dir 切换后）> 源码仓库已下载（开发便利）> 用户默认。
///
/// 纯决策函数（不访问真实文件系统），便于测试注入路径。
fn choose_default_model_dir(user: &Path, legacy: Option<&Path>, repo: &Path) -> PathBuf {
    if user.join(DEFAULT_TOKENS).is_file() {
        user.to_path_buf()
    } else if legacy.is_some_and(|l| l.join(DEFAULT_TOKENS).is_file()) {
        legacy.unwrap().to_path_buf()
    } else if repo.join(DEFAULT_TOKENS).is_file() {
        repo.to_path_buf()
    } else {
        user.to_path_buf()
    }
}

/// 默认模型目录（运行时解析：优先用户目录，旧根存量兜底，源码开发时回退到仓库 `./models/`）。
pub fn default_model_dir() -> PathBuf {
    // legacy 与 user 层次对等：旧根下对应模型的子目录（user 是 `models/<模型名>`）
    let legacy = crate::config::settings::legacy_models_dir()
        .map(|l| l.join(&crate::kws::model::default_asset().name));
    choose_default_model_dir(
        &user_default_model_dir(),
        legacy.as_deref(),
        &repo_models_dir(),
    )
}

/// 展开 settings 中的路径字符串（支持 `${env.VAR}`），未配置时用默认文件名。
/// 返回的路径若为相对路径则拼接在 `model_dir` 下。
fn resolve_file(
    settings_value: Option<&str>,
    default_name: &str,
    model_dir: &Path,
) -> Result<PathBuf, String> {
    match settings_value {
        Some(v) => {
            let expanded = resolve_env_ref(v)?;
            let p = PathBuf::from(&expanded);
            Ok(if p.is_absolute() {
                p
            } else {
                model_dir.join(p)
            })
        }
        None => Ok(model_dir.join(default_name)),
    }
}

/// onnx 默认文件名探测：settings 未显式配置某 onnx 文件时按模型目录内容选择。
///
/// 规则（确定性）：
/// 1. 默认常量文件名存在 → 直接用（zh-en 已装用户零行为变化，混放两代文件时偏默认代）；
/// 2. 否则扫目录中 `{prefix}-` 开头、`.onnx` 结尾、含 `chunk-16` 且非 `.int8` 的文件，
///    排序取第一个（read_dir 顺序不确定，排序保证确定性；字母序下 epoch-12 优先于 epoch-99）；
/// 3. 目录不存在或无匹配 → 回退默认常量名（后续预检报「缺少模型文件」，错误路径清晰）。
fn detect_default_onnx(model_dir: &Path, prefix: &str, fallback: &str) -> String {
    if model_dir.join(fallback).is_file() {
        return fallback.to_string();
    }
    let Ok(entries) = std::fs::read_dir(model_dir) else {
        return fallback.to_string();
    };
    let mut candidates: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| {
            n.starts_with(&format!("{prefix}-"))
                && n.ends_with(".onnx")
                && n.contains("chunk-16")
                && !n.contains(".int8")
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| fallback.to_string())
}

/// keywords 默认文件名探测：不同模型包自带的关键词文件名不同，按候选链取第一个存在的。
fn detect_keywords_rel(model_dir: &Path) -> String {
    /// 候选链（固定顺序）：zh-en 的 `test_wavs/keywords.txt` 在前（与 DEFAULT_KEYWORDS_REL
    /// 一致），其余为其它 sherpa KWS 包（external/HF 导入）的常见布局兜底。
    const CANDIDATES: [&str; 4] = [
        DEFAULT_KEYWORDS_REL,
        "test_wavs/test_keywords.txt",
        "test_keywords.txt",
        "keywords.txt",
    ];
    CANDIDATES
        .iter()
        .find(|c| model_dir.join(c).is_file())
        .copied()
        .unwrap_or(DEFAULT_KEYWORDS_REL)
        .to_string()
}

/// settings 未显式配置某文件字段时的默认名探测入口（tokens 各模型同名，不探测）。
fn detect_default_name(field: &str, model_dir: &Path, fallback: &str) -> String {
    match field {
        "encoder" => detect_default_onnx(model_dir, "encoder", fallback),
        "decoder" => detect_default_onnx(model_dir, "decoder", fallback),
        "joiner" => detect_default_onnx(model_dir, "joiner", fallback),
        "keywords_file" => detect_keywords_rel(model_dir),
        _ => fallback.to_string(),
    }
}

/// 目录内是否探测得到完整的一套 KWS 模型文件（模型无关，替代按 zh-en 文件名硬编码的
/// [`crate::kws::model::is_installed`]，供模型库 external/HF 导入的完整性判断复用）。
pub fn kws_files_present(model_dir: &Path) -> bool {
    let files = [
        detect_default_onnx(model_dir, "encoder", DEFAULT_ENCODER),
        detect_default_onnx(model_dir, "decoder", DEFAULT_DECODER),
        detect_default_onnx(model_dir, "joiner", DEFAULT_JOINER),
        DEFAULT_TOKENS.to_string(),
        detect_keywords_rel(model_dir),
    ];
    files.iter().all(|f| model_dir.join(f).is_file())
}

/// 解析模型目录：CLI > settings > 默认。
fn resolve_model_dir(
    settings: Option<&KwsSettings>,
    cli_model_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(dir) = cli_model_dir {
        return Ok(dir.to_path_buf());
    }
    if let Some(dir) = settings.and_then(|s| s.model_dir.as_deref()) {
        let expanded = resolve_env_ref(dir)?;
        let p = PathBuf::from(expanded);
        return Ok(if p.is_absolute() {
            p
        } else {
            // 相对路径锚定用户配置目录（~/.zapmomo），不再依赖编译期仓库路径
            crate::config::settings::get_settings_dir().join(p)
        });
    }
    Ok(default_model_dir())
}

/// 合并配置并填充默认值。
pub fn resolve(
    settings: Option<&KwsSettings>,
    cli_model_dir: Option<&Path>,
) -> Result<ResolvedKwsConfig, String> {
    let mut cfg = ResolvedKwsConfig {
        model_dir: resolve_model_dir(settings, cli_model_dir)?,
        ..ResolvedKwsConfig::default()
    };

    let s = settings;
    let file = |field: &str, default_name: &str| {
        let value = match field {
            "encoder" => s.and_then(|s| s.encoder.as_deref()),
            "decoder" => s.and_then(|s| s.decoder.as_deref()),
            "joiner" => s.and_then(|s| s.joiner.as_deref()),
            "tokens" => s.and_then(|s| s.tokens.as_deref()),
            "keywords_file" => s.and_then(|s| s.keywords_file.as_deref()),
            _ => None,
        };
        // 未显式配置时按模型目录内容探测默认文件名（不同模型包内文件名不同，
        // 如 epoch-12 系列三件套 + test_wavs/test_keywords.txt 布局）
        let detected = if value.is_none() {
            detect_default_name(field, &cfg.model_dir, default_name)
        } else {
            default_name.to_string()
        };
        resolve_file(value, &detected, &cfg.model_dir)
    };

    cfg.encoder = file("encoder", DEFAULT_ENCODER)?;
    cfg.decoder = file("decoder", DEFAULT_DECODER)?;
    cfg.joiner = file("joiner", DEFAULT_JOINER)?;
    cfg.tokens = file("tokens", DEFAULT_TOKENS)?;
    cfg.keywords_file = file("keywords_file", DEFAULT_KEYWORDS_REL)?;

    cfg.provider = s
        .and_then(|s| s.provider.clone())
        .unwrap_or_else(|| "cpu".to_string());
    cfg.num_threads = s.and_then(|s| s.num_threads).unwrap_or(2);
    cfg.chunk_size = s.and_then(|s| s.chunk_size).unwrap_or(3200);
    cfg.sample_rate = s.and_then(|s| s.sample_rate).unwrap_or(16000);
    cfg.keywords_score = s.and_then(|s| s.keywords_score).unwrap_or(1.0);
    cfg.keywords_threshold = s.and_then(|s| s.keywords_threshold).unwrap_or(0.25);
    cfg.debug = s.and_then(|s| s.debug).unwrap_or(false);
    cfg.enabled = s.and_then(|s| s.enabled).unwrap_or(false);

    Ok(cfg)
}

/// 解析 keywords 文件，返回显示词列表（供日志与校验）。
///
/// 每行一个：跳过空行与 `#` 注释；取 `@` 后的显示词，无 `@` 时整行作为关键词。
pub fn parse_keywords_file(path: &Path) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取关键词文件 {}: {}", path.display(), e))?;
    let mut keywords = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let display = line.rsplit_once('@').map(|(_, d)| d).unwrap_or(line);
        keywords.push(display.trim().to_string());
    }
    if keywords.is_empty() {
        return Err(format!("关键词文件 {} 为空", path.display()));
    }
    Ok(keywords)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::KwsSettings;
    use crate::test_util::run_with_temp_home;
    use std::io::Write;

    fn temp_keywords_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_default_model_dir_dual_root_fallback() {
        run_with_temp_home(|home| {
            crate::test_util::set_custom_data_dir(home);
            let new_dir = user_default_model_dir();
            let legacy_dir = home
                .join(".zapmomo")
                .join("models")
                .join(new_dir.file_name().unwrap());

            // 双方都有 → 新根优先
            for d in [&new_dir, &legacy_dir] {
                std::fs::create_dir_all(d).unwrap();
                std::fs::write(d.join(DEFAULT_TOKENS), b"t").unwrap();
            }
            assert_eq!(default_model_dir(), new_dir);

            // 新根清空 → 回退旧根（data_dir 切换后存量模型保持可用）
            std::fs::remove_dir_all(&new_dir).unwrap();
            assert_eq!(default_model_dir(), legacy_dir);

            // 双方都没有 → 不落旧根
            std::fs::remove_dir_all(&legacy_dir).unwrap();
            assert_ne!(default_model_dir(), legacy_dir);
        });
    }

    #[test]
    fn test_default_config_points_to_default_model_dir() {
        // 不依赖本机仓库/用户目录状态：只校验目录名来自内嵌清单、文件名为常量
        let cfg = ResolvedKwsConfig::default();
        assert_eq!(
            cfg.model_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string()),
            Some(crate::kws::model::default_asset().name.clone())
        );
        assert_eq!(cfg.encoder.file_name().unwrap(), DEFAULT_ENCODER);
        assert_eq!(cfg.decoder.file_name().unwrap(), DEFAULT_DECODER);
        assert_eq!(cfg.joiner.file_name().unwrap(), DEFAULT_JOINER);
        assert_eq!(cfg.tokens.file_name().unwrap(), DEFAULT_TOKENS);
        assert_eq!(cfg.sample_rate, 16000);
        assert_eq!(cfg.chunk_size, 3200);
        assert_eq!(cfg.keywords_threshold, 0.25);
    }

    #[test]
    fn test_user_default_model_dir() {
        run_with_temp_home(|home| {
            let dir = super::user_default_model_dir();
            assert_eq!(
                dir,
                home.join(".zapmomo/models")
                    .join(crate::kws::model::default_asset().name.as_str())
            );
        });
    }

    #[test]
    fn test_choose_default_model_dir_priority() {
        let base = tempfile::tempdir().unwrap();
        let user = base.path().join("user-model");
        let repo = base.path().join("repo-model");

        // 都未安装 → 用户目录
        assert_eq!(choose_default_model_dir(&user, None, &repo), user);

        // 仅仓库有 → 仓库（开发回退）
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(choose_default_model_dir(&user, None, &repo), repo);

        // 用户也有 → 用户优先
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(choose_default_model_dir(&user, None, &repo), user);

        // legacy（data_dir 切换后旧根存量）在 user 无 tokens 时兜底
        std::fs::remove_file(user.join(DEFAULT_TOKENS)).unwrap();
        let legacy = base.path().join("legacy-model");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(DEFAULT_TOKENS), b"t").unwrap();
        assert_eq!(
            choose_default_model_dir(&user, Some(&legacy), &repo),
            legacy
        );
    }

    #[test]
    fn test_resolve_relative_model_dir_anchored_to_user_dir() {
        run_with_temp_home(|home| {
            let settings = KwsSettings {
                model_dir: Some("models/my-model".to_string()),
                ..KwsSettings::default()
            };
            let cfg = resolve(Some(&settings), None).unwrap();
            assert_eq!(cfg.model_dir, home.join(".zapmomo/models/my-model"));
        });
    }

    #[test]
    fn test_resolve_no_settings_uses_defaults() {
        let cfg = resolve(None, None).unwrap();
        assert_eq!(cfg, ResolvedKwsConfig::default());
    }

    #[test]
    fn test_resolve_enabled_default_false_and_override() {
        assert!(!resolve(None, None).unwrap().enabled);
        let settings = KwsSettings {
            enabled: Some(true),
            ..KwsSettings::default()
        };
        assert!(resolve(Some(&settings), None).unwrap().enabled);
    }

    /// 构造跨平台绝对路径（Windows 上 `/xxx` 无盘符不是绝对路径，避免测试依赖 POSIX 语义）
    fn abs_path(rel: &str) -> PathBuf {
        std::path::absolute(rel).unwrap()
    }

    #[test]
    fn test_resolve_cli_model_dir_overrides_settings() {
        let settings = KwsSettings {
            model_dir: Some("settings-model".to_string()),
            ..KwsSettings::default()
        };
        let cli = abs_path("tmp/cli-model");
        let cfg = resolve(Some(&settings), Some(&cli)).unwrap();
        assert_eq!(cfg.model_dir, cli);
        assert_eq!(cfg.encoder.parent().unwrap(), cli);
    }

    #[test]
    fn test_resolve_settings_model_dir() {
        let dir = abs_path("opt/kws");
        let settings = KwsSettings {
            model_dir: Some(dir.to_string_lossy().to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_dir, dir);
        assert_eq!(cfg.encoder, dir.join(DEFAULT_ENCODER));
        assert_eq!(cfg.keywords_file, dir.join(DEFAULT_KEYWORDS_REL));
    }

    #[test]
    fn test_resolve_relative_encoder_joins_model_dir() {
        let dir = abs_path("opt/kws");
        let settings = KwsSettings {
            model_dir: Some(dir.to_string_lossy().to_string()),
            encoder: Some("my-encoder.int8.onnx".to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.encoder, dir.join("my-encoder.int8.onnx"));
    }

    #[test]
    fn test_resolve_absolute_encoder_kept_as_is() {
        let dir = abs_path("opt/kws");
        let enc = abs_path("elsewhere/enc.onnx");
        let settings = KwsSettings {
            model_dir: Some(dir.to_string_lossy().to_string()),
            encoder: Some(enc.to_string_lossy().to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.encoder, enc);
    }

    #[test]
    fn test_resolve_env_ref_in_model_dir() {
        let dir = abs_path("env/kws");
        unsafe {
            std::env::set_var("KWS_MODEL_DIR", dir.to_string_lossy().as_ref());
        }
        let settings = KwsSettings {
            model_dir: Some("${env.KWS_MODEL_DIR}".to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.model_dir, dir);
        unsafe {
            std::env::remove_var("KWS_MODEL_DIR");
        }
    }

    #[test]
    fn test_resolve_numeric_overrides() {
        let settings = KwsSettings {
            num_threads: Some(4),
            chunk_size: Some(1600),
            keywords_threshold: Some(0.5),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.num_threads, 4);
        assert_eq!(cfg.chunk_size, 1600);
        assert_eq!(cfg.keywords_threshold, 0.5);
    }

    #[test]
    fn test_parse_keywords_file_basic() {
        let f = temp_keywords_file("L AY1 T AH1 P @LIGHT_UP\nw én s ēn @文森\n");
        let kws = parse_keywords_file(f.path()).unwrap();
        assert_eq!(kws, vec!["LIGHT_UP".to_string(), "文森".to_string()]);
    }

    #[test]
    fn test_parse_keywords_file_skips_blank_and_comments() {
        let f = temp_keywords_file(
            "# 注释行\n\n  \nL AY1 T AH1 P @LIGHT_UP\n# 另一个注释\nn ǚ ér @女儿\n",
        );
        let kws = parse_keywords_file(f.path()).unwrap();
        assert_eq!(kws, vec!["LIGHT_UP".to_string(), "女儿".to_string()]);
    }

    #[test]
    fn test_parse_keywords_file_without_at_sign() {
        let f = temp_keywords_file("L AY1 T AH1 P\n");
        let kws = parse_keywords_file(f.path()).unwrap();
        assert_eq!(kws, vec!["L AY1 T AH1 P".to_string()]);
    }

    #[test]
    fn test_parse_keywords_file_empty_errors() {
        let f = temp_keywords_file("  \n# only comment\n");
        assert!(parse_keywords_file(f.path()).is_err());
    }

    #[test]
    fn test_parse_keywords_file_missing_file_errors() {
        assert!(parse_keywords_file(Path::new("/nonexistent/kw.txt")).is_err());
    }

    /// 构造 KWS 模型目录（epoch 系列文件名 + tokens + 关键词文件）。
    fn fake_model_dir(
        encoder: &str,
        decoder: &str,
        joiner: &str,
        keywords_rel: &str,
        extra: &[&str],
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in [encoder, decoder, joiner, "tokens.txt"] {
            std::fs::write(dir.path().join(f), b"m").unwrap();
        }
        let kw = dir.path().join(keywords_rel);
        std::fs::create_dir_all(kw.parent().unwrap()).unwrap();
        std::fs::write(kw, "n ǐ h ǎo @你好").unwrap();
        for f in extra {
            std::fs::write(dir.path().join(f), b"m").unwrap();
        }
        dir
    }

    #[test]
    fn test_resolve_detects_epoch_layout() {
        // 非默认布局（external/HF 导入的 sherpa KWS 包常见形态）：
        // epoch-12 三件套 + test_wavs/test_keywords.txt
        let dir = fake_model_dir(
            "encoder-epoch-12-avg-2-chunk-16-left-64.onnx",
            "decoder-epoch-12-avg-2-chunk-16-left-64.onnx",
            "joiner-epoch-12-avg-2-chunk-16-left-64.onnx",
            "test_wavs/test_keywords.txt",
            &[],
        );
        let settings = KwsSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(
            cfg.encoder,
            dir.path()
                .join("encoder-epoch-12-avg-2-chunk-16-left-64.onnx")
        );
        assert_eq!(
            cfg.decoder,
            dir.path()
                .join("decoder-epoch-12-avg-2-chunk-16-left-64.onnx")
        );
        assert_eq!(
            cfg.joiner,
            dir.path()
                .join("joiner-epoch-12-avg-2-chunk-16-left-64.onnx")
        );
        assert_eq!(cfg.tokens, dir.path().join("tokens.txt"));
        assert_eq!(
            cfg.keywords_file,
            dir.path().join("test_wavs/test_keywords.txt")
        );
        assert!(kws_files_present(dir.path()));
    }

    #[test]
    fn test_resolve_default_layout_prefers_constant_names() {
        // zh-en 布局：默认常量名存在 → 全部命中常量（既有用户零行为变化）
        let dir = fake_model_dir(
            DEFAULT_ENCODER,
            DEFAULT_DECODER,
            DEFAULT_JOINER,
            DEFAULT_KEYWORDS_REL,
            &[],
        );
        let settings = KwsSettings {
            model_dir: Some(dir.path().to_string_lossy().to_string()),
            ..KwsSettings::default()
        };
        let cfg = resolve(Some(&settings), None).unwrap();
        assert_eq!(cfg.encoder, dir.path().join(DEFAULT_ENCODER));
        assert_eq!(cfg.decoder, dir.path().join(DEFAULT_DECODER));
        assert_eq!(cfg.joiner, dir.path().join(DEFAULT_JOINER));
        assert_eq!(cfg.keywords_file, dir.path().join(DEFAULT_KEYWORDS_REL));
    }

    #[test]
    fn test_detect_prefers_non_int8_and_earliest_epoch() {
        // epoch-12 fp32 与 int8、epoch-99 fp32 并存 → 排序后取 epoch-12 fp32
        let dir = fake_model_dir(
            "encoder-epoch-99-avg-1-chunk-16-left-64.onnx",
            DEFAULT_DECODER,
            DEFAULT_JOINER,
            DEFAULT_KEYWORDS_REL,
            &[
                "encoder-epoch-12-avg-2-chunk-16-left-64.onnx",
                "encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx",
                "encoder-epoch-99-avg-1-chunk-16-left-64.int8.onnx",
            ],
        );
        let enc = detect_default_onnx(dir.path(), "encoder", DEFAULT_ENCODER);
        assert_eq!(enc, "encoder-epoch-12-avg-2-chunk-16-left-64.onnx");
    }

    #[test]
    fn test_detect_int8_only_falls_back_to_constant() {
        // 目录里只有 int8 变体（非默认布局）→ 回退常量名
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join("encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx"),
            b"m",
        )
        .unwrap();
        let enc = detect_default_onnx(dir.path(), "encoder", DEFAULT_ENCODER);
        assert_eq!(enc, DEFAULT_ENCODER);
    }

    #[test]
    fn test_detect_missing_dir_falls_back_to_constant() {
        // 目录不存在 → 回退常量名（与 resolve 既有行为一致，报错路径清晰）
        let enc = detect_default_onnx(Path::new("/nonexistent-kws"), "encoder", DEFAULT_ENCODER);
        assert_eq!(enc, DEFAULT_ENCODER);
        assert_eq!(
            detect_keywords_rel(Path::new("/nonexistent-kws")),
            DEFAULT_KEYWORDS_REL
        );
    }

    #[test]
    fn test_detect_keywords_candidate_chain() {
        // 仅根目录 keywords.txt（部分模型包布局）→ 候选链兜底命中
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keywords.txt"), b"k").unwrap();
        assert_eq!(detect_keywords_rel(dir.path()), "keywords.txt");
    }

    #[test]
    fn test_kws_files_present() {
        // 完整 epoch 布局目录 → true；缺 encoder → false；空目录 → false
        let dir = fake_model_dir(
            "encoder-epoch-12-avg-2-chunk-16-left-64.onnx",
            "decoder-epoch-12-avg-2-chunk-16-left-64.onnx",
            "joiner-epoch-12-avg-2-chunk-16-left-64.onnx",
            "test_wavs/test_keywords.txt",
            &[],
        );
        assert!(kws_files_present(dir.path()));
        std::fs::remove_file(
            dir.path()
                .join("encoder-epoch-12-avg-2-chunk-16-left-64.onnx"),
        )
        .unwrap();
        assert!(!kws_files_present(dir.path()));

        let empty = tempfile::tempdir().unwrap();
        assert!(!kws_files_present(empty.path()));
        assert!(!kws_files_present(Path::new("/nonexistent-kws")));
    }
}
