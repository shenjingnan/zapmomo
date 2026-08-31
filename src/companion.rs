//! 伙伴库（Companion Library）：管理用户导入的伙伴集合与「当前使用」项。
//!
//! 伙伴形态（`CompanionModel.format` 判别）：
//! - `"cubism3"`：Live2D 模型目录（`.model3.json` 清单）
//! - `"gif"`：GIF 动图文件
//! - `"character"`：角色包目录（`character.md` 人设 + `character.png` 立绘 +
//!   可选 `character.json` 声明（`CompanionManifest`，作者预设随包流通）+
//!   可选 `voice/reference.wav` + `voice/reference.txt` 音色克隆参考）
//!
//! 音色（任意 format 均可拥有，见 `companion_voice_in` 三级解析）：
//! 1. 托管目录 `voice/reference.wav + reference.txt`（角色包自带，优先）；
//! 2. `CompanionModel.voice_id` 绑定的音色库条目（`~/.zapmomo/voices/`，UI 可绑/解绑）；
//! 3. 都没有 → 上层回退全局默认音色（`[tts].voice`）。
//!
//! 数据模型：
//! - 清单：`~/.zapmomo/companions/library.json`（`CompanionLibrary`，schema_version = 1）
//! - 模型文件：`~/.zapmomo/companions/{id}/`（导入时把整个源目录复制进来，**应用托管**，
//!   源目录被删后伙伴仍可用）
//!
//! 导入采用「prepare（复制到临时目录 + 校验）→ commit（锁内二次去重 + 原子 rename）」，
//! 最终托管目录只经 `rename` 一次性出现，避免半成品目录；并发导入同一源时独立 tmp 目录，
//! 只有一个 `rename` 成功，其余返回 `already_imported`。
//!
//! 一致性：`CompanionLibrary.active_model_id` 是唯一逻辑 Source of Truth；
//! `settings.toml [live2d].model_dir` 只是兼容桌宠窗口的 derived runtime cache，
//! 由上层 `reconcile_active` 负责最终一致。

use crate::config::settings;
use crate::live2d::config as live2d_cfg;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 伙伴库 schema 版本。
pub const SCHEMA_VERSION: u32 = 1;

const LIBRARY_FILE: &str = "library.json";
const TMP_PREFIX: &str = ".tmp-";
const ID_PREFIX: &str = "companion-";
/// 补注册动作的统一组名。**绝不写 "Idle"**：pixi-live2d-display 对 Idle 组自动循环
/// 播放（空闲时每帧随机挑动作），写入会改变桌宠「静态展示」行为。
const EXTRA_MOTION_GROUP: &str = "Extra";
/// 存量伙伴补注册迁移标记（写入 `completed_migrations` 闸门，防重复执行）。
const MOTION_REGISTRATION_MIGRATION: &str = "motion-registration-v1";

/// 角色默认欢迎语文案：唤醒词喊的是角色名，欢迎语回报名字形成身份闭环。
/// `{name}` 占位符由 `effective_welcome_text` 展开。
pub const DEFAULT_WELCOME_TEMPLATE: &str = "你好，我是{name}。";
/// 自定义唤醒词长度上限（chars 计数，对齐 `MAX_NAME_CHARS` 风格）。
pub const MAX_WAKE_WORD_CHARS: usize = 20;
/// 自定义欢迎语长度上限（chars 计数）。
pub const MAX_WELCOME_CHARS: usize = 200;

/// 串行化 Library 的读改写与 commit 决策。**不得跨大型目录复制/删除持有。**
static COMPANION_LOCK: Mutex<()> = Mutex::new(());

/// 一个已导入的 Live2D 伙伴。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompanionModel {
    /// 稳定 id：`companion-` + sha256(canonical source 路径) 前 12 位。
    pub id: String,
    /// 角色名（导入目录 basename）。
    pub name: String,
    /// 原始导入目录（仅去重/记录来源；运行时绝不依赖，源删除后伙伴仍有效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// 应用托管目录：`~/.zapmomo/companions/{id}`。
    pub model_dir: String,
    /// 托管目录内的 `.model3.json` 绝对路径。
    pub model_file: String,
    /// 模型格式（"cubism3"）。
    pub format: String,
    /// 导入时间（RFC3339）。
    pub imported_at: String,
    /// 绑定的音色库条目 id（`~/.zapmomo/voices/`；`None` = 未绑定）。
    ///
    /// 只存绑定关系不存路径：解析时经 `tts::voice_store::find_voice_by_id` 严格按 id
    /// 查询，条目被删后 fail-open 回退全局默认（见 `companion_voice_in`）。存 id 不存
    /// 路径，伙伴托管目录搬迁（`relocate_payload`）天然不影响引用。生效优先级低于
    /// 托管目录自带的 `voice/`（目录 > 绑定 > 全局默认，见 `companion_voice_in`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    /// 伙伴私有窗口布局（尺寸/位置）；`None` = 从未单独配置，沿用全局默认
    /// （`settings.toml [live2d]`）与当前窗口状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<CompanionLayout>,
    /// 自定义唤醒词（原始字符串，非 token）；`None` = 跟随 `name`（rename 自动跟随）。
    /// 生效经 `resolve_wake_word`（编码失败回退全局链），存储侧不做格式约束。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_word: Option<String>,
    /// 自定义欢迎语文本；`None` = 按 `DEFAULT_WELCOME_TEMPLATE` 按 name 展开。
    /// 预合成音频的新鲜度由 `companion_welcome` 指纹判定，此处只存文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome_text: Option<String>,
}

/// 伙伴私有窗口布局（尺寸/位置）。
///
/// 字段为 `Option`：`None` = 该项从未单独配置，读取方回退全局默认或沿用当前窗口状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CompanionLayout {
    /// 缩放比例（1.0 = 100%）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// 窗口左上角坐标（逻辑像素）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<settings::CompanionWindowPosition>,
}

/// 伙伴库清单（持久化到 `library.json`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompanionLibrary {
    /// schema 版本；缺失字段按 v1（本版本首次发布，宽容处理）。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub models: Vec<CompanionModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_model_id: Option<String>,
    /// 已完成的一次性迁移标记（如 "motion-registration-v1"）；缺失视为均未执行。
    /// 不 bump SCHEMA_VERSION：新字段对老文件是宽容默认，老代码读新文件忽略未知字段。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_migrations: Vec<String>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for CompanionLibrary {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            models: Vec::new(),
            active_model_id: None,
            completed_migrations: Vec::new(),
        }
    }
}

/// 伙伴库根目录：`~/.zapmomo/companions`。
///
/// 这是**清单目录**（library.json 所在地），永远不跟随 `data_dir`；
/// 模型**载荷目录**见 `get_companions_store_dir`（可自定义）。
pub fn get_companions_dir() -> PathBuf {
    settings::get_settings_dir().join("companions")
}

/// 伙伴载荷存储根集合：当前 store 目录 + 旧默认根（去重）。
///
/// 供「拒绝导入已托管目录」检查与临时目录清理遍历——自定义 `data_dir` 后
/// 旧默认根 `~/.zapmomo/companions` 下的存量载荷仍属托管范围。
pub fn companion_store_roots() -> Vec<PathBuf> {
    let store = settings::get_companions_store_dir();
    let mut roots = vec![store];
    if let Some(legacy) = settings::legacy_companions_dir()
        && !roots.contains(&legacy)
    {
        roots.push(legacy);
    }
    roots
}

fn library_path() -> PathBuf {
    get_companions_dir().join(LIBRARY_FILE)
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    COMPANION_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 读取库清单（**调用者必须已持有 COMPANION_LOCK**）。
///
/// - 文件不存在 → 正常返回空库（首次运行）。
/// - 存在但解析失败 / schema 高于支持版本 → 返回错误，**不覆盖原文件**。
fn load_library_inner() -> Result<CompanionLibrary, String> {
    let path = library_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CompanionLibrary::default());
        }
        Err(e) => return Err(format!("读取伙伴库失败: {e}")),
    };
    let lib: CompanionLibrary = serde_json::from_str(&content).map_err(|e| {
        format!(
            "伙伴库文件损坏（{}）：{e}，原文件已保留，请勿手动覆盖",
            path.display()
        )
    })?;
    if lib.schema_version > SCHEMA_VERSION {
        return Err(format!(
            "伙伴库版本 {} 高于当前 ZapMomo 支持的版本 {SCHEMA_VERSION}，请升级应用",
            lib.schema_version
        ));
    }
    Ok(lib)
}

/// 原子保存库清单（**调用者必须已持有 COMPANION_LOCK**）。
///
/// 「临时文件 + rename」模式，与 `tts/voice_store.rs` 一致；所有写库必须经此函数，
/// 禁止其它代码直接写 `library.json`。
fn save_library_inner(lib: &CompanionLibrary) -> Result<(), String> {
    let dir = get_companions_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建伙伴库目录失败: {e}"))?;
    let tmp = dir.join(format!("{LIBRARY_FILE}.tmp"));
    let content =
        serde_json::to_string_pretty(lib).map_err(|e| format!("序列化伙伴库失败: {e}"))?;
    std::fs::write(&tmp, content).map_err(|e| format!("写入伙伴库临时文件失败: {e}"))?;
    match std::fs::rename(&tmp, library_path()) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows：rename 无法覆盖已存在目标，先移除再重试。
            if library_path().exists() {
                std::fs::remove_file(library_path())
                    .map_err(|e| format!("移除旧伙伴库失败: {e}"))?;
            }
            std::fs::rename(&tmp, library_path()).map_err(|e| format!("保存伙伴库失败: {e}"))
        }
    }
}

/// 由规范化源绝对路径推导稳定 id。
pub fn derive_id(source_abs: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_abs.to_string_lossy().as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{ID_PREFIX}{}", &hex[..12])
}

/// 快速有效判定：托管目录与清单文件都还在磁盘上（View 用，保持快速）。
pub fn quick_valid(model: &CompanionModel) -> bool {
    Path::new(&model.model_dir).is_dir() && Path::new(&model.model_file).is_file()
}

/// GIF 伙伴的 format 标识（与 Live2D 的 "cubism3" 同一字段空间，判别式见 `is_gif`）。
pub const GIF_FORMAT: &str = "gif";

/// format 判别：GIF 伙伴。
pub fn is_gif(model: &CompanionModel) -> bool {
    model.format == GIF_FORMAT
}

/// 角色包伙伴的 format 标识（判别式见 `is_character`）。
pub const CHARACTER_FORMAT: &str = "character";

/// format 判别：角色包伙伴（character.md + character.png + 可选 voice/）。
pub fn is_character(model: &CompanionModel) -> bool {
    model.format == CHARACTER_FORMAT
}

// 角色包约定文件名（托管目录内按约定探测，不入库）。
/// `pub(crate)`：角色包导出/导入白名单（`companion_share`）复用同一组文件名约定。
pub(crate) const CHARACTER_MD: &str = "character.md";
pub(crate) const CHARACTER_PNG: &str = "character.png";
pub(crate) const CHARACTER_JSON: &str = "character.json";
pub(crate) const VOICE_DIR: &str = "voice";
pub(crate) const REFERENCE_WAV: &str = "reference.wav";
pub(crate) const REFERENCE_TXT: &str = "reference.txt";
/// 作者原版音色备份（`upload_companion_voice` 首次覆盖前生成；恢复后删除）。
/// 刻意不进 companion_share 的导出/解压白名单（两处均为精确名匹配）：
/// 备份是本机恢复用的私有资产，不随包流通，收到的分享包里也不该有它。
pub(crate) const REFERENCE_ORIGINAL_WAV: &str = "reference.original.wav";
pub(crate) const REFERENCE_ORIGINAL_TXT: &str = "reference.original.txt";

/// 角色包声明文件（与 `character.md` / `character.png` 三件套，全字段可选）。
///
/// 作者预设随包流通的 **A 层**：任何人导入同一角色包，角色名/预设唤醒词/预设
/// 欢迎语一致（不再依赖「猜 character.md 的 H1」这类启发式）。导入时
/// `wake_word` / `welcome_text` **预填**进 library.json 条目作为初始值（导入者
/// 随后可自行覆盖，覆盖值存 B 层 library.json，不回写包）；无此文件走推导兜底
/// （C 层），存量角色包不受影响。存在但解析失败 → **导入报错**：声明文件是
/// 格式约定，静默忽略会让作者的预设无声失效。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CompanionManifest {
    pub version: Option<u32>,
    pub name: Option<String>,
    pub wake_word: Option<String>,
    pub welcome_text: Option<String>,
}

impl CompanionManifest {
    /// 读取角色包目录内的声明文件（可选文件；不存在 → 空清单）。
    pub fn read(dir: &Path) -> Result<Self, String> {
        let content = match std::fs::read_to_string(dir.join(CHARACTER_JSON)) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("读取角色包 {CHARACTER_JSON} 失败: {e}")),
        };
        serde_json::from_str(&content).map_err(|e| format!("角色包 {CHARACTER_JSON} 格式错误: {e}"))
    }

    /// 预填字段统一清洗：trim；空白归 `None`（宽松，不做长度硬校验——生效侧
    /// 编码失败会自动回退并提示，不应让作者写的预设卡住导入）。
    /// `pub(crate)`：导出合成 character.json（`companion_share`）复用同一清洗规则。
    pub(crate) fn preset(field: &Option<String>) -> Option<String> {
        field
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

/// 校验托管 GIF 伙伴：文件存在且带合法 GIF 文件头（GIF87a/GIF89a）。
///
/// 只读前 6 字节，不加载大尺寸 GIF 的全文。
pub fn validate_gif_file(file: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(file).map_err(|e| format!("GIF 文件不存在或无法读取: {e}"))?;
    let mut magic = [0u8; 6];
    f.read_exact(&mut magic)
        .map_err(|e| format!("读取 GIF 文件头失败: {e}"))?;
    if &magic != b"GIF87a" && &magic != b"GIF89a" {
        return Err("不是合法的 GIF 文件（缺少 GIF87a/GIF89a 文件头）".to_string());
    }
    Ok(())
}

/// 校验 PNG 立绘：文件存在且带合法 PNG 签名（`\x89PNG\r\n\x1a\n`）。
///
/// 只读前 8 字节，不加载图片全文。
pub fn validate_png_file(file: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut f =
        std::fs::File::open(file).map_err(|_| "角色包缺少可读的 character.png 立绘".to_string())?;
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic)
        .map_err(|e| format!("读取 character.png 文件头失败: {e}"))?;
    if &magic != b"\x89PNG\r\n\x1a\n" {
        return Err("character.png 不是合法的 PNG 文件（签名不匹配）".to_string());
    }
    Ok(())
}

/// 校验 wav 文件头（RIFF/WAVE，前 12 字节）。
/// 轻量 wav 头校验（RIFF + WAVE，12 字节）。
///
/// `pub(crate)`：音色上传链在重量级解码（`audio::read_wav_mono`）之前做早期
/// 干净报错复用本函数。
pub(crate) fn validate_wav_header(file: &Path) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open(file).map_err(|e| format!("参考音频不存在或无法读取: {e}"))?;
    let mut magic = [0u8; 12];
    f.read_exact(&mut magic)
        .map_err(|e| format!("读取参考音频文件头失败: {e}"))?;
    if &magic[..4] != b"RIFF" || &magic[8..] != b"WAVE" {
        return Err("reference.wav 不是合法的 wav 文件（缺少 RIFF/WAVE 文件头）".to_string());
    }
    Ok(())
}

/// 校验角色包目录结构（导入时深校验 + set_active/sanitize 轻量校验共用）。
///
/// 规则：`character.md` 存在且非空；`character.png` 为合法 PNG；`character.json`
/// 可选、存在则必须为合法声明；`voice/` 可选，存在时 `reference.wav` 与
/// `reference.txt` 必须成对（缺其一报错）。均为读文件头/小文件级操作，与 GIF
/// 魔数校验同量级。
pub fn validate_character_pack(dir: &Path) -> Result<(), String> {
    // 声明文件可选，但写错了必须报错（静默忽略会让作者预设无声失效）。
    CompanionManifest::read(dir)?;
    let md = dir.join(CHARACTER_MD);
    let content = std::fs::read_to_string(&md)
        .map_err(|e| format!("角色包缺少可读的 character.md 人设文件: {e}"))?;
    if content.trim().is_empty() {
        return Err("角色包 character.md 内容为空".to_string());
    }
    validate_png_file(&dir.join(CHARACTER_PNG))?;

    let voice_dir = dir.join(VOICE_DIR);
    if voice_dir.exists() {
        let wav = voice_dir.join(REFERENCE_WAV);
        let txt = voice_dir.join(REFERENCE_TXT);
        match (wav.is_file(), txt.is_file()) {
            (true, true) => {}
            (false, true) => {
                return Err(
                    "角色包 voice/ 缺少 reference.wav（需与 reference.txt 成对）".to_string(),
                );
            }
            (true, false) => {
                return Err(
                    "角色包 voice/ 缺少 reference.txt（reference.wav 的逐字转写）".to_string(),
                );
            }
            (false, false) => {
                return Err("角色包 voice/ 目录缺少 reference.wav 与 reference.txt".to_string());
            }
        }
        validate_wav_header(&wav)?;
        let text =
            std::fs::read_to_string(&txt).map_err(|e| format!("读取 reference.txt 失败: {e}"))?;
        if text.trim().is_empty() {
            return Err("角色包 reference.txt 内容为空（需要参考音频的逐字转写）".to_string());
        }
    }
    Ok(())
}

/// 从库中解析当前 active 伙伴（库应已 sanitize）。
pub fn active_model(lib: &CompanionLibrary) -> Option<&CompanionModel> {
    let active_id = lib.active_model_id.as_deref()?;
    lib.models.iter().find(|m| m.id.as_str() == active_id)
}

/// 按 format 分派轻量资源校验（GIF → 文件头；角色包 → 目录结构；Live2D → 目录清单）。
///
/// 只查元数据/存在性，供 sanitize / set_active 等有效性判定使用。
fn validate_managed(model: &CompanionModel) -> Result<(), String> {
    if is_gif(model) {
        validate_gif_file(Path::new(&model.model_file))
    } else if is_character(model) {
        validate_character_pack(Path::new(&model.model_dir))
    } else {
        live2d_cfg::validate_managed_model(Path::new(&model.model_dir))
    }
}

/// 枚举 active Live2D 伙伴的可播放动作目录（供右键菜单「状态切换」）。
///
/// 与 [`crate::companion_sprites::list_active_sprites`] 对称的探测式读取：
/// 非 Live2D 伙伴（GIF / 角色包）、无 active、清单读取或解析失败一律返回空，
/// 由调用方（菜单）显示「无可用动作」。组内下标即前端播放下标
/// （见 [`live2d_cfg::parse_motion_catalog`]）。
pub fn list_active_motions() -> Vec<live2d_cfg::MotionGroupInfo> {
    let lib = match load_library_fast() {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!("读取伙伴库失败（跳过动作枚举）: {e}");
            return Vec::new();
        }
    };
    let Some(model) = active_model(&lib) else {
        return Vec::new();
    };
    if is_gif(model) || is_character(model) {
        return Vec::new();
    }
    live2d_cfg::parse_motion_catalog(Path::new(&model.model_file)).unwrap_or_else(|e| {
        tracing::warn!("解析模型动作目录失败: {e}");
        Vec::new()
    })
}

/// 校正 active：指向缺失/无效模型时落到第一个有效伙伴或 `None`；返回是否变更。
///
/// 有效判定使用轻量资源校验（`validate_managed`，只查元数据/存在性）。
fn sanitize_active_inner(lib: &mut CompanionLibrary) -> bool {
    if let Some(model) = active_model(lib)
        && validate_managed(model).is_ok()
    {
        return false;
    }
    let first_valid = lib
        .models
        .iter()
        .find(|m| validate_managed(m).is_ok())
        .map(|m| m.id.clone());
    if lib.active_model_id != first_valid {
        lib.active_model_id = first_valid;
        true
    } else {
        false
    }
}

/// 读取库并校正 active（不迁移旧版配置，毫秒级）。
///
/// 供启动阶段快速 reconcile 使用；若校正改变了 active 会持久化。
/// 顺带清理超过 1 小时的残留临时导入目录（崩溃中断遗留，best-effort）。
pub fn load_library_fast() -> Result<CompanionLibrary, String> {
    cleanup_stale_tmp_dirs();
    let _g = lock();
    let mut lib = load_library_inner()?;
    if sanitize_active_inner(&mut lib) {
        save_library_inner(&lib)?;
    }
    Ok(lib)
}

/// 读取库（空库时尝试迁移旧版 `[live2d].model_dir`）。
///
/// 迁移可能涉及大目录复制，因此空库分支会**释放锁**后再走 `migrate_legacy_if_empty`。
/// 供命令（`list_companions` 等）在 `spawn_blocking` 中调用。
pub fn load_library() -> Result<CompanionLibrary, String> {
    {
        let _g = lock();
        let lib = load_library_inner()?;
        if !lib.models.is_empty() {
            let mut l = lib;
            if sanitize_active_inner(&mut l) {
                save_library_inner(&l)?;
            }
            return Ok(l);
        }
    }
    // 空库：迁移失败仅告警，不影响返回空库（页面仍可用，下次自愈）。
    if let Err(e) = migrate_legacy_if_empty() {
        tracing::warn!("旧版模型迁移失败（将在下次打开伙伴页重试）: {e}");
    }
    load_library_fast()
}

// ===========================================================================
// 导入（prepare → commit）
// ===========================================================================

/// 导入准备结果：已存在（不复制）或就绪可提交。
enum Prepared {
    /// 同源（同 id）已导入。
    Already(CompanionModel),
    /// 已复制到临时目录并通过校验，待 commit。
    Ready(PreparedImport),
}

/// 已准备好、待原子提交的导入。
struct PreparedImport {
    id: String,
    name: String,
    source_path: String,
    tmp_dir: PathBuf,
    final_dir: PathBuf,
    /// `.model3.json` 相对托管根目录的路径。
    model_file_rel: PathBuf,
    format: String,
    imported_at: String,
    /// 角色包声明（作者预设）。Live2D/GIF 路径为空清单（全 `None`）。
    manifest: CompanionManifest,
}

/// RAII：退出作用域时清理残留临时目录（rename 成功后该路径已不存在，remove 为 no-op）。
struct TmpCleanup {
    path: PathBuf,
}

impl TmpCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TmpCleanup {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("清理临时目录失败: {}", e);
        }
    }
}

/// 生成唯一临时目录名（同进程内不冲突）。
fn new_tmp_dir_name(id: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{TMP_PREFIX}{id}-{millis}-{n}")
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 从 tmp 目录名解析创建时间（毫秒）：`.tmp-{id}-{millis}-{n}`，其中 id = `companion-{hex}`。
fn tmp_dir_created_millis(name: &str) -> Option<u64> {
    let mut parts = name.strip_prefix(TMP_PREFIX)?.split('-');
    // 跳过 id 的两个段（"companion"、hex），第三段是毫秒时间戳。
    parts.next()?;
    parts.next()?;
    parts.next()?.parse().ok()
}

/// 清理残留的临时导入目录（best-effort）。
///
/// 仅清理创建时间超过 1 小时的目录，避免误删**正在进行的并发导入**的 tmp。
fn cleanup_stale_tmp_dirs() {
    const MAX_AGE_MS: u64 = 60 * 60 * 1000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(u64::MAX);
    for root in companion_store_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(millis) = tmp_dir_created_millis(&name) else {
                continue;
            };
            if now.saturating_sub(millis) > MAX_AGE_MS {
                let path = entry.path();
                if let Err(e) = std::fs::remove_dir_all(&path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!("清理残留临时目录失败: {e}");
                }
            }
        }
    }
}

/// 递归复制目录。v1 忽略 symlink（打 warn），由托管副本校验兜底。
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("创建目标目录失败: {e}"))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("读取源目录失败: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("读取文件类型失败: {e}"))?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target)
                .map_err(|e| format!("复制文件失败 ({}): {e}", entry.path().display()))?;
        } else if file_type.is_symlink() {
            tracing::warn!("导入时忽略符号链接: {}", entry.path().display());
        }
    }
    Ok(())
}

/// 拒绝导入应用已托管的伙伴路径（防自我复制递归）。
///
/// 根目录也做 canonicalize：避免 macOS 上 /var → /private/var 这类符号链接
/// 使 canonicalize 后的源路径与未规范化根路径比较失配。
/// 自定义 data_dir 后旧默认根下的存量载荷仍属托管范围，所有根都查。
fn reject_managed_source(source_abs: &Path) -> Result<(), String> {
    for root in companion_store_roots() {
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
        if source_abs.starts_with(&root_canon) {
            return Err("不能导入 ZapMomo 已托管的伙伴路径".to_string());
        }
    }
    Ok(())
}

/// 导入准备（**不持锁**，可复制大目录）：
/// 校验源 → 去重预检 → 复制到唯一 tmp → 托管副本校验 → 计算相对清单路径。
fn prepare_import(source: &Path) -> Result<Prepared, String> {
    let source_abs = source
        .canonicalize()
        .map_err(|e| format!("无法访问源目录: {e}"))?;
    if !source_abs.is_dir() {
        return Err(format!("源路径不是目录: {}", source_abs.display()));
    }
    reject_managed_source(&source_abs)?;

    let id = derive_id(&source_abs);
    let name = source_abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| id.clone());

    // 去重预检（短锁，避免已导入时白白复制大目录）。
    {
        let _g = lock();
        let lib = load_library_inner()?;
        if let Some(existing) = lib.models.iter().find(|m| m.id == id).cloned() {
            return Ok(Prepared::Already(existing));
        }
    }

    // 源目录结构校验（Cubism2 提前拒绝，避免复制后才发现）。
    let (_, source_format) = live2d_cfg::find_model_file(&source_abs)
        .ok_or_else(|| "目录中未找到 Live2D 模型清单（*.model3.json 或 model.json）".to_string())?;
    if source_format == live2d_cfg::Live2dFormat::Cubism2 {
        return Err(
            "暂不支持 Cubism 2 模型（.moc + model.json），请使用 Cubism 3/4/5 模型（.moc3 + .model3.json）"
                .to_string(),
        );
    }

    let store_dir = settings::get_companions_store_dir();
    let tmp_dir = store_dir.join(new_tmp_dir_name(&id));
    let final_dir = store_dir.join(&id);
    copy_dir_recursive(&source_abs, &tmp_dir)?;

    // 托管副本深度校验：Moc + Textures 必须存在。
    if let Err(e) = live2d_cfg::validate_managed_model(&tmp_dir) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    // 记录 model3.json 相对 tmp 根的路径，commit 后换算成托管目录内的绝对路径。
    let (model_file_in_tmp, format) =
        live2d_cfg::find_model_file(&tmp_dir).ok_or_else(|| "复制后未找到模型清单".to_string())?;

    // 补注册托管副本中未登记的动作/表情文件（best-effort：失败仅告警，不阻塞导入；
    // 此时副本还在 tmp，源目录不受影响）。
    if let Err(e) = register_missing_motion_files(&model_file_in_tmp) {
        tracing::warn!("补注册动作/表情失败（不影响导入）: {e}");
    }

    let model_file_rel = model_file_in_tmp
        .strip_prefix(&tmp_dir)
        .map_err(|_| "模型清单路径异常".to_string())?
        .to_path_buf();

    Ok(Prepared::Ready(PreparedImport {
        id,
        name,
        source_path: source_abs.display().to_string(),
        tmp_dir,
        final_dir,
        model_file_rel,
        format: format.to_str().to_string(),
        imported_at: now_rfc3339(),
        manifest: CompanionManifest::default(),
    }))
}

/// 原子提交（加锁，锁只覆盖 rename/库更新/save 临界区）：
/// 二次去重 → rename → 更新库 → save；失败时清理残留，绝不留下「半成品目录」或「孤儿条目」。
fn commit_import(prepared: PreparedImport) -> Result<(CompanionModel, bool), String> {
    // 所有失败路径的残留 tmp 都由 RAII 清理（rename 成功后 tmp 已不存在，remove 为 no-op）。
    let _tmp = TmpCleanup::new(prepared.tmp_dir.clone());

    let lib = {
        let _g = lock();
        load_library_inner()?
    };
    // 去重（第一次，锁已释放）：同源已在库 → 已导入。
    if let Some(existing) = lib.models.iter().find(|m| m.id == prepared.id).cloned() {
        return Ok((existing, true));
    }

    let model = CompanionModel {
        id: prepared.id.clone(),
        name: prepared.name.clone(),
        source_path: Some(prepared.source_path.clone()),
        model_dir: prepared.final_dir.display().to_string(),
        model_file: prepared
            .final_dir
            .join(&prepared.model_file_rel)
            .display()
            .to_string(),
        format: prepared.format.clone(),
        imported_at: prepared.imported_at.clone(),
        voice_id: None,
        layout: None,
        // 作者预设预填（角色包声明；Live2D/GIF 清单为空 → None）。只清洗不查长度：
        // 生效侧编码失败自动回退并提示，不应让预设卡住导入。
        wake_word: CompanionManifest::preset(&prepared.manifest.wake_word),
        welcome_text: CompanionManifest::preset(&prepared.manifest.welcome_text),
    };

    {
        let _g = lock();
        // 二次去重：并发导入同源可能在 prepare 与 commit 之间已插入。
        let lib2 = load_library_inner()?;
        if let Some(existing) = lib2.models.iter().find(|m| m.id == prepared.id).cloned() {
            return Ok((existing, true));
        }
        // 原子 rename（同卷）：最终托管目录只经 rename 一次性出现。
        if let Err(e) = std::fs::rename(&prepared.tmp_dir, &prepared.final_dir) {
            return Err(format!("提交托管目录失败: {e}"));
        }
        let first = lib2.models.is_empty();
        let mut new_lib = lib2;
        new_lib.models.push(model.clone());
        if first {
            new_lib.active_model_id = Some(model.id.clone());
        }
        if let Err(e) = save_library_inner(&new_lib) {
            // 库未持久化成功：先结束临界区，再移除已 rename 的最终目录防 orphan。
            drop(_g);
            if let Err(rm_e) = std::fs::remove_dir_all(&prepared.final_dir) {
                tracing::warn!("移除托管目录失败: {rm_e}");
            }
            return Err(e);
        }
    }
    Ok((model, false))
}

/// 导入 Live2D 模型目录：复制到应用托管目录并登记进伙伴库。
///
/// 返回 `(CompanionModel, already_imported)`；首次导入自动设为 active。
pub fn import_from_dir(source: &Path) -> Result<(CompanionModel, bool), String> {
    match prepare_import(source)? {
        Prepared::Already(model) => Ok((model, true)),
        Prepared::Ready(prepared) => commit_import(prepared),
    }
}

/// 导入 GIF 动图文件：复制到托管目录 `{id}/{原文件名}` 并登记进伙伴库。
///
/// 复用 `commit_import` 原子流程（同源去重 / 失败清理 / 首次导入自动 active）。
pub fn import_gif_from_file(source: &Path) -> Result<(CompanionModel, bool), String> {
    let source_abs = source
        .canonicalize()
        .map_err(|e| format!("无法访问源文件: {e}"))?;
    let is_gif_ext = source_abs
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("gif"));
    if !is_gif_ext {
        return Err("仅支持导入 .gif 文件".to_string());
    }
    // 拒绝导入应用已托管的伙伴文件（与目录导入同防线，见 reject_managed_source）。
    reject_managed_source(&source_abs)?;

    let id = derive_id(&source_abs);
    let name = source_abs
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| id.clone());

    // 去重预检（短锁，避免已导入时白白复制大文件）。
    {
        let _g = lock();
        let lib = load_library_inner()?;
        if let Some(existing) = lib.models.iter().find(|m| m.id == id).cloned() {
            return Ok((existing, true));
        }
    }

    let store_dir = settings::get_companions_store_dir();
    let tmp_dir = store_dir.join(new_tmp_dir_name(&id));
    let final_dir = store_dir.join(&id);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let file_name = source_abs
        .file_name()
        .ok_or_else(|| "源文件缺少文件名".to_string())?
        .to_owned();
    std::fs::copy(&source_abs, tmp_dir.join(&file_name))
        .map_err(|e| format!("复制 GIF 失败: {e}"))?;

    // 托管副本校验：GIF 魔数必须合法，失败时清理 tmp（commit 内 RAII 只覆盖后续路径）。
    if let Err(e) = validate_gif_file(&tmp_dir.join(&file_name)) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    commit_import(PreparedImport {
        id,
        name,
        source_path: source_abs.display().to_string(),
        tmp_dir,
        final_dir,
        model_file_rel: PathBuf::from(&file_name),
        format: GIF_FORMAT.to_string(),
        imported_at: now_rfc3339(),
        manifest: CompanionManifest::default(),
    })
}

/// 统一导入入口：目录含 `character.md` → 角色包导入；其余目录 → Live2D 模型目录导入；
/// 文件 → GIF 动图导入。
pub fn import_source(source: &Path) -> Result<(CompanionModel, bool), String> {
    if source.is_file() {
        import_gif_from_file(source)
    } else if source.join(CHARACTER_MD).is_file() {
        import_character_from_dir(source)
    } else {
        import_from_dir(source)
    }
}

// ===========================================================================
// 角色包（character.md + character.png + 可选 voice/）
// ===========================================================================

/// 按 chars 计数截断到 `MAX_NAME_CHARS`（与 `rename` 校验一致）。
fn truncate_name(s: &str) -> String {
    s.chars().take(MAX_NAME_CHARS).collect()
}

/// 从 character.md 提取角色名：第一个 `# ` 开头的 H1 行（启发式兜底，C 层）。
///
/// 缺失/为空时回退 `fallback`（导入目录 basename）；超 `MAX_NAME_CHARS` 截断
/// （chars 计数，与 `rename` 校验一致）。文件不可读时同样回退（内容校验由
/// `validate_character_pack` 在托管副本上兜底）。正式约定见 `CompanionManifest`
/// （character.json 的 `name` 字段优先于本函数）。
fn extract_character_name(character_md: &Path, fallback: &str) -> String {
    let fallback = if fallback.is_empty() {
        "未命名角色"
    } else {
        fallback
    };
    let Ok(content) = std::fs::read_to_string(character_md) else {
        return fallback.to_string();
    };
    let name = content
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .filter(|s| !s.is_empty());
    let Some(name) = name else {
        return fallback.to_string();
    };
    let truncated = truncate_name(name);
    if truncated.is_empty() {
        fallback.to_string()
    } else {
        truncated
    }
}

/// 把多声道 wav 混音为 16-bit PCM 单声道（就地改写，tmp + rename 原子替换）。
///
/// 单声道文件不动（返回 false）。采样率原样保留：采样率归一由合成时
/// `tts::normalize_reference` 负责；声道是它的盲区——sherpa `Wave::read` 只接受
/// 单声道，多声道直接读取失败，因此必须在导入侧解决。
/// `pub(crate)`：音色上传链对暂存 wav 做同样的单声道改写（`upload_companion_voice`）。
pub(crate) fn convert_reference_to_mono(wav: &Path) -> Result<bool, String> {
    let mut reader = hound::WavReader::open(wav)
        .map_err(|e| format!("无法解码参考音频（{}）: {e}", wav.display()))?;
    let spec = reader.spec();
    if spec.channels == 1 {
        return Ok(false);
    }
    let channels = spec.channels as usize;

    // 统一读成 f32：Int 按位宽归一，Float 直接读。
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取参考音频采样失败: {e}"))?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取参考音频采样失败: {e}"))?,
    };

    // 按帧平均混音为多声道 → 单声道。
    let frames: Vec<f32> = samples
        .chunks(channels)
        .filter(|frame| !frame.is_empty())
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();

    let tmp = wav.with_extension("mono.tmp.wav");
    let write_result = (|| -> Result<(), String> {
        let out_spec = hound::WavSpec {
            channels: 1,
            sample_rate: spec.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&tmp, out_spec)
            .map_err(|e| format!("创建单声道参考音频失败: {e}"))?;
        for s in &frames {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer
                .write_sample(v)
                .map_err(|e| format!("写入单声道参考音频失败: {e}"))?;
        }
        writer
            .finalize()
            .map_err(|e| format!("完成单声道参考音频失败: {e}"))?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    match std::fs::rename(&tmp, wav) {
        Ok(()) => Ok(true),
        Err(_) => {
            rename_replace(&tmp, wav)?;
            Ok(true)
        }
    }
}

/// rename 落位，Windows 上 rename 无法覆盖已存在目标 → 先移除再重试。
///
/// 从 `convert_reference_to_mono` 抽出，音色上传/恢复落位共用。
fn rename_replace(from: &Path, to: &Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::remove_file(to).map_err(|e| format!("移除旧文件失败（{}）: {e}", to.display()))?;
    std::fs::rename(from, to).map_err(|e| format!("替换文件失败（{}）: {e}", to.display()))
}

// ===========================================================================
// 音色上传覆盖 / 恢复作者原版（用户对自带音色不满意时替换；解析链零改动）
// ===========================================================================

/// 托管目录内音色四件套路径（reference.* 当前生效 + reference.original.* 作者备份）。
struct CompanionVoicePaths {
    wav: PathBuf,
    txt: PathBuf,
    original_wav: PathBuf,
    original_txt: PathBuf,
}

fn voice_paths(model_dir: &Path) -> CompanionVoicePaths {
    let voice_dir = model_dir.join(VOICE_DIR);
    CompanionVoicePaths {
        wav: voice_dir.join(REFERENCE_WAV),
        txt: voice_dir.join(REFERENCE_TXT),
        original_wav: voice_dir.join(REFERENCE_ORIGINAL_WAV),
        original_txt: voice_dir.join(REFERENCE_ORIGINAL_TXT),
    }
}

/// 首次覆盖前备份作者原版（逐文件独立判断，自愈半完成备份；**拷贝不 move**）：
/// 仅当 `original.X 不存在 && reference.X 存在` 时拷贝——重复上传不覆盖备份，
/// 备份永远保持作者原版。
fn backup_original_voice(paths: &CompanionVoicePaths) -> Result<(), String> {
    for (src, dst) in [
        (&paths.wav, &paths.original_wav),
        (&paths.txt, &paths.original_txt),
    ] {
        if !dst.is_file() && src.is_file() {
            std::fs::copy(src, dst)
                .map_err(|e| format!("备份作者原版音色失败（{}）: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// 伙伴是否留有作者原版音色备份（`reference.original.*` 成对；true = 可一键恢复）。
///
/// 任意 format 均可（上传不限定角色包）；成对性严格判定——缺一即视为无备份，
/// 避免「按钮亮着但恢复必失败」的半状态。毫秒级 stat，与 `has_persona` 同量级。
pub fn has_original_voice(model: &CompanionModel) -> bool {
    let paths = voice_paths(Path::new(&model.model_dir));
    paths.original_wav.is_file() && paths.original_txt.is_file()
}

/// 上传自定义参考音频，覆盖托管目录内角色音色（`voice/reference.wav + reference.txt`）。
///
/// 首次覆盖前把作者原版备份为 `reference.original.*`（重复上传不覆盖备份）。
/// 任意 format 均可；不写 library.json（音色是目录级资产，voice_id 绑定不受影响）。
///
/// 事务性：所有校验/转换先在暂存文件上完成，备份（纯拷贝）成功后才允许落位——
/// 任何失败路径都不得破坏既有 `reference.*`。落位顺序 txt 先、wav 最后「翻面」：
/// 两文件无法真正原子，但落位前 pair 已存在即保住「成对」不变量
/// （`validate_character_pack` 校验成对，破对会被 sanitize 判无效）。
/// **同步阻塞（秒级），调用方必须 spawn_blocking。**
pub fn upload_companion_voice(
    id: &str,
    source_wav: &Path,
    reference_text: &str,
) -> Result<(), String> {
    let text = reference_text.trim();
    if text.is_empty() {
        return Err("请提供参考音频的逐字转写文本".to_string());
    }
    let model_dir = {
        let _g = lock();
        let lib = load_library_inner()?;
        lib.models
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?
            .model_dir
            .clone()
    };
    let paths = voice_paths(Path::new(&model_dir));
    std::fs::create_dir_all(Path::new(&model_dir).join(VOICE_DIR))
        .map_err(|e| format!("创建 voice 目录失败: {e}"))?;

    // 暂存文件上完成全部校验/转换（mono 改写的是暂存副本，源文件不动）。
    let tmp_wav = paths.wav.with_extension("upload.tmp.wav");
    let tmp_txt = paths.txt.with_extension("upload.tmp.txt");
    let stage = (|| -> Result<(), String> {
        std::fs::copy(source_wav, &tmp_wav).map_err(|e| format!("读取上传音频失败: {e}"))?;
        // 早期干净报错（12 字节头），重量级解码前拦截明显不是 wav 的文件。
        validate_wav_header(&tmp_wav)?;
        // 立体声/高位宽/float → 16-bit 单声道（采样率原样，合成侧 normalize 归一）。
        convert_reference_to_mono(&tmp_wav)?;
        // 终检：hound 全量解码 = 合成链同款解码器，拦下头合法但数据坏/空的文件。
        crate::audio::read_wav_mono(&tmp_wav)?;
        backup_original_voice(&paths)?;
        // 破坏性边界之上：到这里原 reference.* 一字未动。
        std::fs::write(&tmp_txt, text).map_err(|e| format!("写入转写文本失败: {e}"))?;
        rename_replace(&tmp_txt, &paths.txt)?;
        rename_replace(&tmp_wav, &paths.wav)?;
        Ok(())
    })();
    if let Err(e) = stage {
        let _ = std::fs::remove_file(&tmp_wav);
        let _ = std::fs::remove_file(&tmp_txt);
        return Err(e);
    }
    tracing::info!(
        "伙伴 {id} 音色已更新（作者原版备份状态：original.wav 存在={}）",
        paths.original_wav.is_file()
    );
    Ok(())
}

/// 恢复作者原版音色：`reference.original.*` 拷回 `reference.*` 并删除备份。
///
/// 无备份（或不成对）→ Err；备份在恢复成功前绝不删除，中途失败可重试。
/// `reference.wav` 被手动删除的情况天然修复（copy 不依赖目标存在）。
pub fn restore_companion_voice(id: &str) -> Result<(), String> {
    let model_dir = {
        let _g = lock();
        let lib = load_library_inner()?;
        lib.models
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?
            .model_dir
            .clone()
    };
    let paths = voice_paths(Path::new(&model_dir));
    if !paths.original_wav.is_file() || !paths.original_txt.is_file() {
        return Err("没有可恢复的原始音色备份".to_string());
    }

    let tmp_wav = paths.wav.with_extension("restore.tmp.wav");
    let tmp_txt = paths.txt.with_extension("restore.tmp.txt");
    let result = (|| -> Result<(), String> {
        std::fs::copy(&paths.original_wav, &tmp_wav)
            .map_err(|e| format!("读取备份音频失败: {e}"))?;
        std::fs::copy(&paths.original_txt, &tmp_txt)
            .map_err(|e| format!("读取备份转写失败: {e}"))?;
        // 自检备份未被手动损坏：坏备份宁可报错也不写坏生效音色。
        validate_wav_header(&tmp_wav)?;
        rename_replace(&tmp_txt, &paths.txt)?;
        rename_replace(&tmp_wav, &paths.wav)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp_wav);
        let _ = std::fs::remove_file(&tmp_txt);
        return Err(e);
    }
    // 落位成功后才清备份（逐个 best-effort，NotFound 无害）。
    let _ = std::fs::remove_file(&paths.original_wav);
    let _ = std::fs::remove_file(&paths.original_txt);
    tracing::info!("伙伴 {id} 已恢复作者原版音色");
    Ok(())
}

/// 导入角色包目录：复制到托管目录（含立体声→单声道改写）并登记进伙伴库。
///
/// 复用 `commit_import` 原子流程（同源去重 / 失败清理 / 首次导入自动 active）。
pub fn import_character_from_dir(source: &Path) -> Result<(CompanionModel, bool), String> {
    let source_abs = source
        .canonicalize()
        .map_err(|e| format!("无法访问源目录: {e}"))?;
    if !source_abs.is_dir() {
        return Err(format!("源路径不是目录: {}", source_abs.display()));
    }
    reject_managed_source(&source_abs)?;

    let id = derive_id(&source_abs);
    let fallback_name = source_abs
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| id.clone());
    // 名字解析链（A > C）：character.json.name > character.md H1 > 目录 basename。
    let manifest = CompanionManifest::read(&source_abs)?;
    let name = CompanionManifest::preset(&manifest.name)
        .map(|n| truncate_name(&n))
        .unwrap_or_else(|| extract_character_name(&source_abs.join(CHARACTER_MD), &fallback_name));

    // 去重预检（短锁，避免已导入时白白复制目录）。
    {
        let _g = lock();
        let lib = load_library_inner()?;
        if let Some(existing) = lib.models.iter().find(|m| m.id == id).cloned() {
            return Ok((existing, true));
        }
    }

    let store_dir = settings::get_companions_store_dir();
    let tmp_dir = store_dir.join(new_tmp_dir_name(&id));
    let final_dir = store_dir.join(&id);
    copy_dir_recursive(&source_abs, &tmp_dir)?;

    // 托管副本校验：md/png/voice 结构必须合法，失败时清理 tmp（commit 内 RAII 只覆盖后续路径）。
    if let Err(e) = validate_character_pack(&tmp_dir) {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    // 参考音频多声道 → 单声道（就地改写托管副本，源目录不动；
    // 对齐 register_missing_motion_files「只改托管副本」先例）。
    let managed_wav = tmp_dir.join(VOICE_DIR).join(REFERENCE_WAV);
    if managed_wav.is_file()
        && let Err(e) = convert_reference_to_mono(&managed_wav)
    {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    commit_import(PreparedImport {
        id,
        name,
        source_path: source_abs.display().to_string(),
        tmp_dir,
        final_dir,
        model_file_rel: PathBuf::from(CHARACTER_PNG),
        format: CHARACTER_FORMAT.to_string(),
        imported_at: now_rfc3339(),
        manifest,
    })
}

// ===========================================================================
// 伙伴运行时探测（人设 / 音色；约定文件名 + library.json 绑定）
// ===========================================================================

/// 角色包音色克隆参考（参考音频 + 逐字转写）。
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterVoice {
    /// 托管目录内 `voice/reference.wav` 绝对路径（或音色库条目 wav 路径）。
    pub wav: PathBuf,
    /// 参考音频的逐字转写。
    pub text: String,
}

/// 伙伴音色的生效来源（`companion_voice_in` 命中哪一级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompanionVoiceSource {
    /// 托管目录 `voice/` 自带（第 1 级，角色包自带或用户手动放入）。
    Pack,
    /// 音色库绑定（第 2 级，`CompanionModel.voice_id`）。
    Library,
}

/// 伙伴音色三级解析（目录自带 > 音色库绑定 > None），同时给出命中来源。
///
/// - 第 1 级：托管目录 `voice/reference.wav + reference.txt` 成对非空（任意 format
///   都可探测，不限定角色包）；
/// - 第 2 级：`voice_id` 经 `voice_store::find_voice_by_id` 严格按 id 查——刻意
///   不走 `resolve_reference` 的「id 或名称」宽容匹配与 builtin 回退（名称碰撞 /
///   绑 builtin 跨模型失效在此是歧义源）；
/// - 绑定失效（条目已删）fail-open：warn 后返回 None，上层回退全局默认音色。
pub fn companion_voice_in(
    model: &CompanionModel,
) -> Option<(CharacterVoice, CompanionVoiceSource)> {
    if let Some(voice) = character_voice_in(Path::new(&model.model_dir)) {
        return Some((voice, CompanionVoiceSource::Pack));
    }
    let voice_id = model.voice_id.as_deref()?;
    match crate::tts::voice_store::find_voice_by_id(voice_id) {
        Some(v) => Some((
            CharacterVoice {
                wav: v.wav_path,
                text: v.reference_text,
            },
            CompanionVoiceSource::Library,
        )),
        None => {
            tracing::warn!(
                "伙伴 {} 绑定的音色不存在（{}），回退全局默认",
                model.id,
                voice_id
            );
            None
        }
    }
}

/// 当前 active 的角色包伙伴（无 active 或非角色包 → None）。
fn active_character_model() -> Option<CompanionModel> {
    let lib = load_library_fast()
        .map_err(|e| {
            tracing::warn!("读取伙伴库失败（跳过角色包探测）: {e}");
            e
        })
        .ok()?;
    let model = active_model(&lib)?;
    is_character(model).then(|| model.clone())
}

/// active 角色包的人设文本（character.md 全文）。
///
/// 读取失败或内容为空按「无」降级（warn 日志），不让伙伴文件问题炸掉语音链路。
pub fn active_persona() -> Option<String> {
    let model = active_character_model()?;
    let path = Path::new(&model.model_dir).join(CHARACTER_MD);
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => Some(content),
        Ok(_) => {
            tracing::warn!("角色包 character.md 为空（{}）", path.display());
            None
        }
        Err(e) => {
            tracing::warn!("读取角色包人设失败（{}）: {e}", path.display());
            None
        }
    }
}

/// active 伙伴的音色克隆参考（目录 `voice/` > 音色库绑定 > None）。
///
/// 任意 format 均可（不限定角色包）。文件缺失 / 绑定失效按「无」降级（warn 日志）。
pub fn active_companion_voice() -> Option<CharacterVoice> {
    let lib = load_library_fast()
        .map_err(|e| {
            tracing::warn!("读取伙伴库失败（跳过伙伴音色探测）: {e}");
            e
        })
        .ok()?;
    let model = active_model(&lib)?;
    companion_voice_in(model).map(|(voice, _)| voice)
}

/// 当前 active 伙伴（任意 format；库读取失败或无 active → None）。
///
/// 与 `active_character_model`（仅角色包）相对；唤醒词/欢迎语不限定角色包，
/// GIF 桌宠同样可以拥有（音色回退全局默认）。
pub fn active_model_fast() -> Option<CompanionModel> {
    let lib = load_library_fast()
        .map_err(|e| {
            tracing::warn!("读取伙伴库失败（跳过伙伴探测）: {e}");
            e
        })
        .ok()?;
    active_model(&lib).cloned()
}

/// 伙伴生效唤醒词：自定义优先，未自定义跟随当前 `name`（rename 自动跟随）。
pub fn effective_wake_word(model: &CompanionModel) -> String {
    model
        .wake_word
        .clone()
        .unwrap_or_else(|| model.name.clone())
}

/// 伙伴生效欢迎语文本：自定义优先，未自定义按默认模板展开 name。
pub fn effective_welcome_text(model: &CompanionModel) -> String {
    model
        .welcome_text
        .clone()
        .unwrap_or_else(|| DEFAULT_WELCOME_TEMPLATE.replace("{name}", &model.name))
}

/// active 伙伴的生效唤醒词（无 active → None）。
pub fn active_wake_word() -> Option<String> {
    active_model_fast().map(|m| effective_wake_word(&m))
}

/// 唤醒词解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeWordResolution {
    /// 最终生效唤醒词（已确认可编码为 token）；`None` = 沿用 KWS 模型内置关键词。
    pub word: Option<String>,
    /// active 伙伴的角色级唤醒词（参与判定的原始值）；无 active 伙伴时为 None。
    pub companion_word: Option<String>,
    /// 角色级唤醒词能否编码为 token；`false` = 已回退 `fallback`（前端据此提示）。
    pub companion_ok: bool,
}

/// 解析生效唤醒词：active 伙伴词 > `fallback` > None（模型内置）。
///
/// `fallback` 传 `resolve()` 合并后的既有唤醒词链（cli > [voice].keywords >
/// [kws].custom_keywords），本函数在其上叠加「激活即换词」语义——active 伙伴词
/// **压过 CLI flag**：唤醒词跟随当前角色是显式产品行为，CLI 仅在无角色或角色词
/// 不可编码时兜底。角色词编码失败（生僻字/表情等）回退 `fallback` 并置
/// `companion_ok = false`，由调用方告警。
pub fn resolve_wake_word(fallback: Option<&str>, tokens: &Path) -> WakeWordResolution {
    let Some(word) = active_wake_word() else {
        return WakeWordResolution {
            word: fallback.map(str::to_string),
            companion_word: None,
            companion_ok: true,
        };
    };
    let ok = crate::kws::token::encode_custom_keywords(&word, tokens).is_ok();
    if ok {
        WakeWordResolution {
            word: Some(word.clone()),
            companion_word: Some(word),
            companion_ok: true,
        }
    } else {
        WakeWordResolution {
            word: fallback.map(str::to_string),
            companion_word: Some(word),
            companion_ok: false,
        }
    }
}

/// 探测托管目录内的角色音色（voice/reference.wav + reference.txt 成对且非空）。
fn character_voice_in(model_dir: &Path) -> Option<CharacterVoice> {
    let voice_dir = model_dir.join(VOICE_DIR);
    let wav = voice_dir.join(REFERENCE_WAV);
    if !wav.is_file() {
        return None;
    }
    match std::fs::read_to_string(voice_dir.join(REFERENCE_TXT)) {
        Ok(text) if !text.trim().is_empty() => Some(CharacterVoice {
            wav,
            text: text.trim().to_string(),
        }),
        Ok(_) => {
            tracing::warn!("角色包 reference.txt 为空（{}）", voice_dir.display());
            None
        }
        Err(e) => {
            tracing::warn!("读取角色包 reference.txt 失败: {e}");
            None
        }
    }
}

/// 角色包是否带人设（供列表视图 Badge；毫秒级小 IO）。
pub fn has_persona(model: &CompanionModel) -> bool {
    if !is_character(model) {
        return false;
    }
    std::fs::read_to_string(Path::new(&model.model_dir).join(CHARACTER_MD))
        .map(|c| !c.trim().is_empty())
        .unwrap_or(false)
}

/// 伙伴是否有生效音色（目录 `voice/` 自带或音色库绑定命中；供列表视图 Badge）。
///
/// 与 `has_character_voice`（已废弃的旧语义）的区别：不限定角色包 format，
/// 且绑定指向的音色库条目被删后返回 false（fail-open，UI 显示「已失效」）。
pub fn has_voice(model: &CompanionModel) -> bool {
    companion_voice_in(model).is_some()
}

/// 绑定 / 解绑伙伴音色（写 `library.json` 的 `voice_id` 字段）。
///
/// - `Some(voice_id)`：必须命中音色库条目（`find_voice_by_id` 严格按 id），
///   不存在报错，防止绑出失效引用；
/// - `None`：解绑，回退目录自带或全局默认。
/// - 与 `set_companion_scale` 等窗口属性命令无关，这里是伙伴库元数据。
pub fn set_voice_binding(id: &str, voice_id: Option<&str>) -> Result<CompanionLibrary, String> {
    if let Some(vid) = voice_id
        && crate::tts::voice_store::find_voice_by_id(vid).is_none()
    {
        return Err(format!("未找到音色: {vid}"));
    }
    let lib;
    {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let model = inner
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        model.voice_id = voice_id.map(str::to_string);
        save_library_inner(&inner)?;
        lib = inner;
    }
    Ok(lib)
}

/// 设置伙伴自定义唤醒词（`None` = 恢复跟随 `name`）。
///
/// 只存原始字符串，不做编码校验——可编码性随 KWS 模型变化，存储侧保持宽松；
/// 保存前的即时校验由命令层负责（用户保存时即知，而不是等下次启动会话）。
pub fn set_wake_word(id: &str, wake_word: Option<&str>) -> Result<CompanionLibrary, String> {
    let lib;
    {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let model = inner
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        model.wake_word = normalize_optional_text(wake_word, MAX_WAKE_WORD_CHARS, "唤醒词")?;
        save_library_inner(&inner)?;
        lib = inner;
    }
    Ok(lib)
}

/// 设置伙伴自定义欢迎语（`None` = 恢复默认模板）。预合成 wav 的重生成由命令层联动。
pub fn set_welcome_text(id: &str, text: Option<&str>) -> Result<CompanionLibrary, String> {
    let lib;
    {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let model = inner
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        model.welcome_text = normalize_optional_text(text, MAX_WELCOME_CHARS, "欢迎语")?;
        save_library_inner(&inner)?;
        lib = inner;
    }
    Ok(lib)
}

/// 自定义文本入库前的统一清洗：trim；空串归一为 `None`（= 恢复默认）；
/// 超长按 chars 计数报错。欢迎语/唤醒词共用，规则与 `rename` 一致。
fn normalize_optional_text(
    raw: Option<&str>,
    max_chars: usize,
    label: &str,
) -> Result<Option<String>, String> {
    let Some(text) = raw else {
        return Ok(None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max_chars {
        return Err(format!("{label}过长（最多 {max_chars} 个字符）"));
    }
    Ok(Some(trimmed.to_string()))
}

/// 按 id 解析角色包伙伴条目（导出分享用，`companion_share::export_pack` 消费）。
///
/// 短锁读取（不跨重型 IO）；非角色包格式 → Err：Live2D 含版权模型且无
/// character.md、GIF 为单文件，均无「角色包分享」语义。
pub(crate) fn character_pack_model(id: &str) -> Result<CompanionModel, String> {
    let _g = lock();
    let lib = load_library_inner()?;
    let model = lib
        .models
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "未找到该伙伴".to_string())?;
    if !is_character(&model) {
        return Err("仅角色包伙伴支持导出分享（Live2D 含版权模型，GIF 为单文件）".to_string());
    }
    Ok(model)
}

/// 清理所有指向 `voice_id` 的伙伴绑定（音色库条目被删除时由命令层联动调用）。
///
/// 返回被清理的伙伴 id 列表（供上层判断是否需要重启语音会话）；无引用时不写盘。
pub fn clear_voice_bindings(voice_id: &str) -> Result<Vec<String>, String> {
    let _g = lock();
    let mut inner = load_library_inner()?;
    let mut affected = Vec::new();
    for model in &mut inner.models {
        if model.voice_id.as_deref() == Some(voice_id) {
            model.voice_id = None;
            affected.push(model.id.clone());
        }
    }
    if !affected.is_empty() {
        save_library_inner(&inner)?;
    }
    Ok(affected)
}

// ===========================================================================
// active / 迁移
// ===========================================================================

/// 设置当前使用伙伴（Library 先持久化成功，再由上层 reconcile 同步桌宠）。
///
/// 会做轻量资源校验：模型必须可基础加载。
pub fn set_active(id: &str) -> Result<CompanionLibrary, String> {
    let lib;
    {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let model = inner
            .models
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        validate_managed(&model).map_err(|e| format!("该伙伴模型不可用，无法设为当前使用：{e}"))?;
        inner.active_model_id = Some(id.to_string());
        save_library_inner(&inner)?;
        lib = inner;
    }
    Ok(lib)
}

/// 移除伙伴：删除托管目录 + 库条目；若删的是 active，active 自动落到第一个有效伙伴
/// 或 `None`。先保存库清单（事实来源），再 best-effort 删除托管目录。
pub fn remove(id: &str) -> Result<CompanionLibrary, String> {
    let (lib, removed_dir) = {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let Some(idx) = inner.models.iter().position(|m| m.id == id) else {
            return Err("未找到该伙伴".to_string());
        };
        let model = inner.models.remove(idx);
        // 删的是 active → 落到第一个有效伙伴（quick_valid 足够，load 时的 sanitize 会再深校验）。
        if inner.active_model_id.as_deref() == Some(id) {
            inner.active_model_id = inner
                .models
                .iter()
                .find(|m| quick_valid(m))
                .map(|m| m.id.clone());
        }
        save_library_inner(&inner)?;
        (inner, Some(PathBuf::from(&model.model_dir)))
    };
    // 解锁后 best-effort 删除托管目录（失败仅告警，不留下「有库条目但目录缺失」的悬空状态）。
    if let Some(dir) = removed_dir
        && let Err(e) = std::fs::remove_dir_all(&dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("删除托管目录失败: {e}");
    }
    Ok(lib)
}

/// 把前端从 Live2D 渲染画布截取的 PNG 写入伙伴托管目录 `{id}/cover.png`。
///
/// `cover_image` 由视图层 `find_cover_image` 探测，无需改库结构。
pub fn save_cover(id: &str, png: &[u8]) -> Result<(), String> {
    let model_dir = {
        let _g = lock();
        let lib = load_library_inner()?;
        let model = lib
            .models
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        PathBuf::from(&model.model_dir)
    };
    std::fs::create_dir_all(&model_dir).map_err(|e| format!("创建托管目录失败: {e}"))?;
    let tmp = model_dir.join("cover.png.tmp");
    std::fs::write(&tmp, png).map_err(|e| format!("写入封面图失败: {e}"))?;
    std::fs::rename(&tmp, model_dir.join("cover.png")).map_err(|e| format!("保存封面图失败: {e}"))
}

/// 递归扫描 `base` 下所有 `*.motion3.json` / `*.exp3.json`，收集为相对 base 的
/// 正斜杠路径（Live2D 清单惯例，Windows 分隔符归一）。
fn collect_motion_assets(
    base: &Path,
    dir: &Path,
    motions: &mut Vec<String>,
    expressions: &mut Vec<String>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("读取目录失败 ({}): {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_motion_assets(base, &path, motions, expressions)?;
            continue;
        }
        let Some(rel) = path
            .strip_prefix(base)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
        else {
            continue;
        };
        if rel.ends_with(".motion3.json") {
            motions.push(rel);
        } else if rel.ends_with(".exp3.json") {
            expressions.push(rel);
        }
    }
    Ok(())
}

/// 收集清单中已注册的 File 引用（Motions 各组并集 + Expressions）。
fn collect_registered_files(json: &serde_json::Value) -> HashSet<String> {
    let mut set = HashSet::new();
    let Some(refs) = json.get("FileReferences") else {
        return set;
    };
    if let Some(groups) = refs.get("Motions").and_then(|v| v.as_object()) {
        for motions in groups.values().filter_map(|v| v.as_array()) {
            for m in motions {
                if let Some(f) = m.get("File").and_then(|v| v.as_str()) {
                    set.insert(f.to_string());
                }
            }
        }
    }
    for e in refs
        .get("Expressions")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if let Some(f) = e.get("File").and_then(|v| v.as_str()) {
            set.insert(f.to_string());
        }
    }
    set
}

/// 相对路径转展示名：basename 去掉 `.motion3.json` / `.exp3.json` 扩展。
fn asset_display_name(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(".exp3.json")
        .or_else(|| name.strip_suffix(".motion3.json"))
        .unwrap_or(name)
        .to_string()
}

/// 原子写文件（tmp + rename，Windows 先移除再 rename 兜底；模式同 `save_library_inner`）。
fn atomic_write_file(path: &Path, contents: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp-reg");
    std::fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败: {e}"))?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows：rename 无法覆盖已存在目标，先移除再重试。
            if path.exists() {
                std::fs::remove_file(path).map_err(|e| format!("移除旧文件失败: {e}"))?;
            }
            std::fs::rename(&tmp, path).map_err(|e| format!("原子替换失败: {e}"))
        }
    }
}

/// 扫描 model3.json 所在目录（含子目录）中存在但未注册的 `*.motion3.json` /
/// `*.exp3.json`，补注册进 FileReferences 后原子回写。返回是否发生了修改。
///
/// - 未注册动作统一进 `Motions["Extra"]`（不写 "Idle"，见 `EXTRA_MOTION_GROUP`）；
///   清单已有的组（含作者的 Idle）一律不动。
/// - 未注册表情进 `Expressions`，Name 取文件名去扩展名（允许重名：播放走 index）。
/// - 回写后重跑轻量校验，失败恢复原内容并按「无修改」返回（best-effort 增强，
///   注册失败不阻塞导入）。
fn register_missing_motion_files(model_file: &Path) -> Result<bool, String> {
    let Some(base) = model_file.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return Err("模型清单缺少父目录".to_string());
    };
    let original =
        std::fs::read_to_string(model_file).map_err(|e| format!("读取模型清单失败: {e}"))?;
    let mut json: serde_json::Value =
        serde_json::from_str(&original).map_err(|e| format!("解析模型清单失败: {e}"))?;

    let mut motions: Vec<String> = Vec::new();
    let mut expressions: Vec<String> = Vec::new();
    collect_motion_assets(base, base, &mut motions, &mut expressions)?;
    motions.sort();
    expressions.sort();

    let registered = collect_registered_files(&json);
    let new_motions: Vec<String> = motions
        .into_iter()
        .filter(|f| !registered.contains(f.as_str()))
        .collect();
    let new_expressions: Vec<String> = expressions
        .into_iter()
        .filter(|f| !registered.contains(f.as_str()))
        .collect();
    if new_motions.is_empty() && new_expressions.is_empty() {
        return Ok(false);
    }

    // 读-改-写（Value 无类型解析保留未知字段，serde_json::Map::entry 自动建缺失键）。
    let obj = json
        .as_object_mut()
        .ok_or_else(|| "模型清单不是 JSON 对象".to_string())?;
    let refs = obj
        .entry("FileReferences")
        .or_insert(serde_json::Value::Object(serde_json::Map::new()));
    let refs = refs
        .as_object_mut()
        .ok_or_else(|| "FileReferences 不是对象".to_string())?;
    if !new_motions.is_empty() {
        let groups = refs
            .entry("Motions")
            .or_insert(serde_json::Value::Object(serde_json::Map::new()));
        let groups = groups
            .as_object_mut()
            .ok_or_else(|| "Motions 不是对象".to_string())?;
        let extra = groups
            .entry(EXTRA_MOTION_GROUP)
            .or_insert(serde_json::Value::Array(Vec::new()));
        let extra = extra
            .as_array_mut()
            .ok_or_else(|| "Motions[Extra] 不是数组".to_string())?;
        for file in new_motions {
            extra.push(serde_json::json!({ "File": file }));
        }
    }
    if !new_expressions.is_empty() {
        let list = refs
            .entry("Expressions")
            .or_insert(serde_json::Value::Array(Vec::new()));
        let list = list
            .as_array_mut()
            .ok_or_else(|| "Expressions 不是数组".to_string())?;
        for file in new_expressions {
            let name = asset_display_name(&file);
            list.push(serde_json::json!({ "Name": name, "File": file }));
        }
    }

    let rewritten =
        serde_json::to_string_pretty(&json).map_err(|e| format!("序列化模型清单失败: {e}"))?;
    atomic_write_file(model_file, &rewritten)?;

    // 写后二次校验：失败恢复原内容（不能弄坏一个本来可导入的模型）。
    if let Err(e) = live2d_cfg::validate_managed_model(base) {
        tracing::warn!("补注册后模型校验失败，已恢复原清单: {e}");
        let _ = atomic_write_file(model_file, &original);
        return Ok(false);
    }
    Ok(true)
}

/// 伙伴展示名最大长度。
const MAX_NAME_CHARS: usize = 30;

/// 重命名伙伴（只改展示名，**不触碰托管文件**，不影响 active / 桌宠）。
pub fn rename(id: &str, name: &str) -> Result<CompanionLibrary, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("伙伴名称不能为空".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("伙伴名称过长（最多 {MAX_NAME_CHARS} 个字符）").to_string());
    }
    let lib;
    {
        let _g = lock();
        let mut inner = load_library_inner()?;
        let model = inner
            .models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| "未找到该伙伴".to_string())?;
        model.name = name.to_string();
        save_library_inner(&inner)?;
        lib = inner;
    }
    Ok(lib)
}

/// 保存伙伴私有缩放比例（增量更新：只覆盖 `layout.scale`，不动 position）。
///
/// 由角色窗口滚轮/设置面板/原生菜单缩放后调用，写入 `library.json` 中该伙伴的条目。
pub fn save_layout_scale(id: &str, scale: f64) -> Result<(), String> {
    let _g = lock();
    let mut lib = load_library_inner()?;
    let model = lib
        .models
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| "未找到该伙伴".to_string())?;
    model.layout.get_or_insert_with(Default::default).scale = Some(scale);
    save_library_inner(&lib)
}

/// 保存伙伴私有窗口位置（逻辑像素；增量更新：只覆盖 `layout.position`，不动 scale）。
///
/// 由前端在用户手动拖动窗口后（debounce）调用，写入 `library.json` 中该伙伴的条目。
pub fn save_layout_position(id: &str, x: i32, y: i32) -> Result<(), String> {
    let _g = lock();
    let mut lib = load_library_inner()?;
    let model = lib
        .models
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| "未找到该伙伴".to_string())?;
    model.layout.get_or_insert_with(Default::default).position =
        Some(settings::CompanionWindowPosition { x, y });
    save_library_inner(&lib)
}

/// 当前 active 伙伴的私有布局（无 active / 未配置 → None）。
pub fn active_layout() -> Option<CompanionLayout> {
    let lib = load_library_fast()
        .map_err(|e| {
            tracing::warn!("读取伙伴库失败（跳过布局探测）: {e}");
            e
        })
        .ok()?;
    active_model(&lib)?.layout.clone()
}

/// 迁移后改写伙伴库中某条目的载荷路径（`model_dir`/`model_file` 指向新 store 目录）。
///
/// 旧载荷根 = 条目当前路径所在的管理根（`companion_store_roots()` 中能剥离的那个）。
/// 幂等：`id` 不存在时返回 `Ok(None)`。持 `COMPANION_LOCK`，锁不跨大文件复制。
pub fn relocate_payload(id: &str, new_dir: &Path) -> Result<Option<CompanionModel>, String> {
    let _g = lock();
    let mut lib = load_library_inner()?;
    let relocated = {
        let Some(model) = lib.models.iter_mut().find(|m| m.id == id) else {
            return Ok(None);
        };
        // 旧载荷根：条目当前路径能剥离的那个管理根
        let old_root = companion_store_roots()
            .into_iter()
            .find(|r| settings::strip_prefix_ci(Path::new(&model.model_dir), r).is_some())
            .unwrap_or_else(get_companions_dir);
        model.model_dir = relocate_in_root(&model.model_dir, &old_root, new_dir);
        model.model_file = relocate_in_root(&model.model_file, &old_root, new_dir);
        model.clone()
    }; // 可变借用在此结束
    save_library_inner(&lib)?;
    Ok(Some(relocated))
}

/// 把 `path` 从 `old_root` 前缀改写为 `new_root`（不在 old_root 下则原样返回）。
fn relocate_in_root(path: &str, old_root: &Path, new_root: &Path) -> String {
    let p = Path::new(path);
    match settings::strip_prefix_ci(p, old_root) {
        Some(rest) => new_root.join(rest).display().to_string(),
        None => path.to_string(),
    }
}

/// 旧版迁移：库为空且 `[live2d].model_dir` 指向合法模型时，复用安全导入流程复制进
/// 托管目录并设为 active。返回新伙伴 id；无需迁移返回 `None`。
///
/// 幂等（库非空跳过）；由启动后台任务调用，`load_library` 亦会兜底触发。
pub fn migrate_legacy_if_empty() -> Result<Option<String>, String> {
    let legacy_dir: Option<PathBuf> = {
        let _g = lock();
        let lib = load_library_inner()?;
        if !lib.models.is_empty() {
            return Ok(None);
        }
        settings::load_settings()?
            .and_then(|s| s.live2d)
            .and_then(|l| l.model_dir)
            .map(PathBuf::from)
    };
    let Some(dir) = legacy_dir else {
        return Ok(None);
    };
    if live2d_cfg::find_model_file(&dir).is_none() {
        return Ok(None);
    }
    let (model, _already) = import_from_dir(&dir)?;
    Ok(Some(model.id))
}

/// 存量伙伴一次性迁移：为库中所有伙伴补注册未登记的动作/表情文件。
///
/// 幂等闸门：`completed_migrations` 含本迁移标记即跳过。扫描/回写**不持锁**
/// （大目录 IO 不进临界区）；标记落库在短锁内重读后只追加标记字段，避免与
/// 并发导入互相覆盖。返回发生回写的模型数（0 = 无需处理或已迁移）。
pub fn register_motions_for_existing() -> Result<usize, String> {
    let models: Vec<CompanionModel> = {
        let _g = lock();
        let lib = load_library_inner()?;
        if lib
            .completed_migrations
            .iter()
            .any(|m| m == MOTION_REGISTRATION_MIGRATION)
        {
            return Ok(0);
        }
        lib.models
            .iter()
            .filter(|m| quick_valid(m))
            .cloned()
            .collect()
    };

    let mut touched = 0usize;
    for model in &models {
        match register_missing_motion_files(Path::new(&model.model_file)) {
            Ok(true) => touched += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!("伙伴 {} 补注册动作/表情失败: {e}", model.id),
        }
    }

    {
        let _g = lock();
        let mut lib = load_library_inner()?;
        if !lib
            .completed_migrations
            .iter()
            .any(|m| m == MOTION_REGISTRATION_MIGRATION)
        {
            lib.completed_migrations
                .push(MOTION_REGISTRATION_MIGRATION.to_string());
            save_library_inner(&lib)?;
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;

    /// 构造一个校验可通过的模型目录。
    fn make_valid_model(dir: &Path, manifest: &str) {
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("model.moc3"), b"moc").unwrap();
        std::fs::write(dir.join("textures/texture_00.png"), b"png").unwrap();
        std::fs::write(
            dir.join(manifest),
            r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"]}}"#,
        )
        .unwrap();
    }

    /// 构造最小合法 GIF（GIF89a magic + 最少头部字节）。
    fn make_valid_gif(path: &Path) {
        std::fs::write(path, b"GIF89a\x01\x00\x01\x00\x00").unwrap();
    }

    /// 在 fixture 基础上补未注册的动作/表情文件（模拟「火花」这类模型：文件在、清单没登记）。
    fn add_unregistered_assets(dir: &Path) {
        std::fs::create_dir_all(dir.join("Motions")).unwrap();
        std::fs::write(dir.join("Motions/睡觉动画.motion3.json"), "{}").unwrap();
        std::fs::write(dir.join("chibang.motion3.json"), "{}").unwrap(); // 根目录散文件
        std::fs::create_dir_all(dir.join("Expressions")).unwrap();
        std::fs::write(dir.join("Expressions/03 生气.exp3.json"), "{}").unwrap();
        std::fs::write(dir.join("Expressions/07 星星眼.exp3.json"), "{}").unwrap();
    }

    /// 解析 model3.json 为 Value（测试辅助）。
    fn read_manifest(model_file: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(model_file).unwrap()).unwrap()
    }

    #[test]
    fn test_list_active_motions_lists_live2d_groups() {
        run_with_temp_home(|home| {
            let src = home.join("m");
            make_valid_model(&src, "m.model3.json");
            std::fs::write(
                src.join("m.model3.json"),
                r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"],"Motions":{"Tap":[{"File":"a.motion3.json"},{"File":"b.motion3.json"}],"Idle":[{"File":"motions/i.motion3.json"}]}}}"#,
            )
            .unwrap();
            let (model, _) = import_from_dir(&src).unwrap();
            set_active(&model.id).unwrap();

            let catalog = list_active_motions();
            assert_eq!(catalog.len(), 2, "两个非空组");
            assert_eq!(catalog[0].group, "Idle", "serde_json 键序（字母序）");
            assert_eq!(catalog[0].motions[0].name, "i");
            assert_eq!(catalog[1].group, "Tap");
            assert_eq!(
                catalog[1].motions.len(),
                2,
                "组内顺序 = 清单数组顺序（播放下标）"
            );
        });
    }

    #[test]
    fn test_list_active_motions_empty_for_non_live2d_and_no_active() {
        run_with_temp_home(|home| {
            // 无 active
            assert!(list_active_motions().is_empty());

            // 角色包
            let src = home.join("f");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("character.md"), "# 芙宁娜\n").unwrap();
            std::fs::write(src.join("character.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
            let (character, _) = import_character_from_dir(&src).unwrap();
            set_active(&character.id).unwrap();
            assert!(list_active_motions().is_empty(), "角色包无动作目录");

            // GIF 单文件
            let gif = home.join("舞.gif");
            make_valid_gif(&gif);
            let (gif_model, _) = import_gif_from_file(&gif).unwrap();
            set_active(&gif_model.id).unwrap();
            assert!(list_active_motions().is_empty(), "GIF 伙伴无动作目录");
        });
    }

    #[test]
    fn test_list_active_motions_broken_manifest_is_empty_not_panic() {
        run_with_temp_home(|home| {
            let src = home.join("m");
            make_valid_model(&src, "m.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();
            set_active(&model.id).unwrap();
            std::fs::write(Path::new(&model.model_file), "not json").unwrap();
            assert!(list_active_motions().is_empty(), "畸形清单降级为空");
        });
    }

    #[test]
    fn test_register_missing_motion_files_registers_extra_and_expressions() {
        run_with_temp_home(|home| {
            let src = home.join("m");
            make_valid_model(&src, "m.model3.json");
            add_unregistered_assets(&src);
            let model_file = src.join("m.model3.json");

            assert!(register_missing_motion_files(&model_file).unwrap());

            let json = read_manifest(&model_file);
            let refs = json.get("FileReferences").unwrap();
            // 两个动作都进 Extra 组，路径为相对 model3.json 的正斜杠相对路径。
            let extra = refs
                .get("Motions")
                .and_then(|m| m.get("Extra"))
                .and_then(|e| e.as_array())
                .unwrap();
            let files: Vec<&str> = extra
                .iter()
                .filter_map(|e| e.get("File").and_then(|f| f.as_str()))
                .collect();
            assert_eq!(
                files,
                vec!["Motions/睡觉动画.motion3.json", "chibang.motion3.json"]
            );
            // 绝不写 Idle 组（库会对 Idle 自动循环播放，改变桌宠静态行为）。
            assert!(refs.get("Motions").and_then(|m| m.get("Idle")).is_none());
            // 表情注册，Name 为文件名去扩展名（列表按路径字典序）。
            let exprs = refs.get("Expressions").and_then(|e| e.as_array()).unwrap();
            assert_eq!(exprs.len(), 2);
            assert_eq!(
                exprs[0].get("Name").and_then(|n| n.as_str()),
                Some("03 生气")
            );
            assert_eq!(
                exprs[1].get("Name").and_then(|n| n.as_str()),
                Some("07 星星眼")
            );
            // Moc/Textures 原样保留。
            assert_eq!(refs.get("Moc").and_then(|v| v.as_str()), Some("model.moc3"));
            // 写后仍可通过校验。
            live2d_cfg::validate_managed_model(&src).unwrap();
        });
    }

    #[test]
    fn test_register_missing_motion_files_idempotent_and_skips_registered() {
        run_with_temp_home(|home| {
            let src = home.join("m");
            // 已有注册：Idle 组一个动作 + 一个表情（作者自己写的，绝不能动）。
            std::fs::create_dir_all(src.join("textures")).unwrap();
            std::fs::write(src.join("model.moc3"), b"moc").unwrap();
            std::fs::write(src.join("textures/texture_00.png"), b"png").unwrap();
            std::fs::write(
                src.join("m.model3.json"),
                r#"{"FileReferences":{
                    "Moc":"model.moc3",
                    "Textures":["textures/texture_00.png"],
                    "Motions":{"Idle":[{"File":"idle.motion3.json"}]},
                    "Expressions":[{"Name":"开心","File":"happy.exp3.json"}]}}"#,
            )
            .unwrap();
            std::fs::write(src.join("idle.motion3.json"), "{}").unwrap();
            std::fs::write(src.join("happy.exp3.json"), "{}").unwrap();
            let model_file = src.join("m.model3.json");
            let before = std::fs::read_to_string(&model_file).unwrap();

            // 已全部注册 → 无修改。
            assert!(!register_missing_motion_files(&model_file).unwrap());
            assert_eq!(std::fs::read_to_string(&model_file).unwrap(), before);

            // 注册后再调用 → 幂等。
            std::fs::write(src.join("new.motion3.json"), "{}").unwrap();
            assert!(register_missing_motion_files(&model_file).unwrap());
            assert!(!register_missing_motion_files(&model_file).unwrap());
            let json = read_manifest(&model_file);
            let refs = json.get("FileReferences").unwrap();
            // 作者的 Idle 组原样（未被追加），新动作进 Extra。
            let idle = refs
                .get("Motions")
                .and_then(|m| m.get("Idle"))
                .and_then(|e| e.as_array())
                .unwrap();
            assert_eq!(idle.len(), 1);
            let extra = refs
                .get("Motions")
                .and_then(|m| m.get("Extra"))
                .and_then(|e| e.as_array())
                .unwrap();
            assert_eq!(extra.len(), 1);
        });
    }

    #[test]
    fn test_register_missing_motion_files_rejects_bad_json() {
        run_with_temp_home(|home| {
            let src = home.join("m");
            make_valid_model(&src, "m.model3.json");
            std::fs::write(src.join("m.model3.json"), "{not-json").unwrap();
            let err = register_missing_motion_files(&src.join("m.model3.json")).unwrap_err();
            assert!(err.contains("解析"), "{err}");
            // 原文件未被覆盖。
            assert_eq!(
                std::fs::read_to_string(src.join("m.model3.json")).unwrap(),
                "{not-json"
            );
        });
    }

    #[test]
    fn test_import_registers_missing_motion_files() {
        run_with_temp_home(|home| {
            let src = home.join("火花");
            make_valid_model(&src, "火花.model3.json");
            add_unregistered_assets(&src);

            let (model, _) = import_from_dir(&src).unwrap();
            // 托管副本（非源目录）的清单被补注册。
            let json = read_manifest(Path::new(&model.model_file));
            let refs = json.get("FileReferences").unwrap();
            let extra = refs
                .get("Motions")
                .and_then(|m| m.get("Extra"))
                .and_then(|e| e.as_array())
                .unwrap();
            assert_eq!(extra.len(), 2);
            assert_eq!(
                refs.get("Expressions")
                    .and_then(|e| e.as_array())
                    .unwrap()
                    .len(),
                2
            );
            // 源目录清单保持原样（不污染用户文件）。
            let src_json = read_manifest(&src.join("火花.model3.json"));
            assert!(
                src_json
                    .get("FileReferences")
                    .unwrap()
                    .get("Motions")
                    .is_none()
            );
        });
    }

    #[test]
    fn test_register_motions_for_existing_migrates_and_is_idempotent() {
        run_with_temp_home(|home| {
            let src = home.join("存量");
            make_valid_model(&src, "old.model3.json");
            add_unregistered_assets(&src);
            let (model, _) = import_from_dir(&src).unwrap();
            // 手动还原清单为未注册状态，模拟「老版本导入、还没补注册」的存量。
            std::fs::write(
                Path::new(&model.model_file),
                r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"]}}"#,
            )
            .unwrap();

            let touched = register_motions_for_existing().unwrap();
            assert_eq!(touched, 1);
            let json = read_manifest(Path::new(&model.model_file));
            assert!(json.get("FileReferences").unwrap().get("Motions").is_some());

            // 标记已写入 → 二次调用直接跳过（即使再手动抹掉注册也不再处理）。
            let lib = load_library_fast().unwrap();
            assert!(
                lib.completed_migrations
                    .iter()
                    .any(|m| m == MOTION_REGISTRATION_MIGRATION)
            );
            std::fs::write(
                Path::new(&model.model_file),
                r#"{"FileReferences":{"Moc":"model.moc3","Textures":["textures/texture_00.png"]}}"#,
            )
            .unwrap();
            assert_eq!(register_motions_for_existing().unwrap(), 0);
        });
    }

    #[test]
    fn test_library_json_without_migrations_field_still_loads() {
        run_with_temp_home(|_home| {
            let dir = get_companions_dir();
            std::fs::create_dir_all(&dir).unwrap();
            // 老版本 library.json（无 completed_migrations 字段）宽容加载。
            std::fs::write(
                dir.join(LIBRARY_FILE),
                r#"{"schema_version":1,"models":[],"active_model_id":null}"#,
            )
            .unwrap();
            let lib = load_library_fast().unwrap();
            assert!(lib.completed_migrations.is_empty());
        });
    }

    #[test]
    fn test_derive_id_stable_and_distinct() {
        run_with_temp_home(|home| {
            let a = home.join("A");
            let b = home.join("B");
            assert_eq!(derive_id(&a), derive_id(&a));
            assert_ne!(derive_id(&a), derive_id(&b));
            assert!(derive_id(&a).starts_with(ID_PREFIX));
        });
    }

    #[test]
    fn test_default_library_schema_version() {
        let lib = CompanionLibrary::default();
        assert_eq!(lib.schema_version, SCHEMA_VERSION);
        assert!(lib.models.is_empty());
        assert!(lib.active_model_id.is_none());
    }

    #[test]
    fn test_load_library_missing_file_returns_empty() {
        run_with_temp_home(|_home| {
            let lib = load_library_fast().unwrap();
            assert!(lib.models.is_empty());
        });
    }

    #[test]
    fn test_import_goes_to_custom_store_dir() {
        run_with_temp_home(|home| {
            // 设置自定义数据目录
            let data = home.join("zapdata");
            let mut config = settings::AppConfig::default();
            config.data_dir = Some(data.display().to_string());
            settings::save_settings(&config).unwrap();

            let src = home.join("srcmodel");
            make_valid_model(&src, "srcmodel.model3.json");
            let (model, _already) = import_from_dir(&src).unwrap();

            // 载荷目录在 <data_dir>/companions 下
            let store = data.join("companions");
            assert!(model.model_dir.starts_with(&store.display().to_string()));
            assert!(Path::new(&model.model_dir).is_dir());
            assert!(Path::new(&model.model_file).is_file());
            // 清单 library.json 仍留在 ~/.zapmomo/companions（不跟随 data_dir）
            assert!(home.join(".zapmomo/companions/library.json").is_file());
            // 旧默认载荷根目录下没有该载荷
            assert!(!home.join(".zapmomo/companions").join(&model.id).exists());
        });
    }

    #[test]
    fn test_relocate_payload_rewrites_paths() {
        run_with_temp_home(|home| {
            let data = crate::test_util::set_custom_data_dir(home);
            let old_root = home.join(".zapmomo/companions");
            let id = "companion-abc";
            // 造一个已导入的伙伴：旧根目录 + library.json
            make_valid_model(&old_root.join(id), "cat.model3.json");
            let lib = CompanionLibrary {
                schema_version: SCHEMA_VERSION,
                models: vec![CompanionModel {
                    id: id.to_string(),
                    name: "cat".into(),
                    source_path: None,
                    model_dir: old_root.join(id).display().to_string(),
                    model_file: old_root
                        .join(id)
                        .join("cat.model3.json")
                        .display()
                        .to_string(),
                    format: "Cubism3".into(),
                    imported_at: "t".into(),
                    voice_id: None,
                    layout: None,
                    wake_word: None,
                    welcome_text: None,
                }],
                active_model_id: Some(id.to_string()),
                completed_migrations: Vec::new(),
            };
            std::fs::create_dir_all(&old_root).unwrap();
            let json = serde_json::to_string_pretty(&lib).unwrap();
            std::fs::write(old_root.join(LIBRARY_FILE), json).unwrap();

            let new_store = data.join("companions");
            let updated = relocate_payload(id, &new_store).unwrap().unwrap();
            assert_eq!(updated.model_dir, new_store.join(id).display().to_string());
            assert_eq!(
                updated.model_file,
                new_store
                    .join(id)
                    .join("cat.model3.json")
                    .display()
                    .to_string()
            );
            // 已持久化到 library.json
            let reloaded = load_library_fast().unwrap();
            let m = reloaded.models.iter().find(|m| m.id == id).unwrap();
            assert_eq!(m.model_dir, new_store.join(id).display().to_string());
        });
    }

    #[test]
    fn test_import_rejects_legacy_default_root_when_custom() {
        run_with_temp_home(|home| {
            let data = home.join("zapdata");
            let mut config = settings::AppConfig::default();
            config.data_dir = Some(data.display().to_string());
            settings::save_settings(&config).unwrap();

            // 源目录在旧默认根下（迁移前的载荷位置）：同样视为已托管，拒绝导入
            let inside_legacy = home.join(".zapmomo/companions/companion-abc");
            make_valid_model(&inside_legacy, "m.model3.json");
            let err = import_from_dir(&inside_legacy).unwrap_err();
            assert!(err.contains("已托管"));
        });
    }

    #[test]
    fn test_import_first_model_sets_active_and_copies() {
        run_with_temp_home(|home| {
            let src = home.join("大月下");
            make_valid_model(&src, "大月下.model3.json");
            let (model, already) = import_from_dir(&src).unwrap();
            assert!(!already);
            assert!(model.name == "大月下");
            // 托管副本已复制：model_dir / model_file 均指向 companions 下，且文件存在。
            let expected_dir = get_companions_dir().join(&model.id);
            assert_eq!(model.model_dir, expected_dir.display().to_string());
            assert!(Path::new(&model.model_file).is_file());
            assert!(model.model_file.starts_with(&model.model_dir));
            // 首次导入自动 active。
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.active_model_id.as_deref(), Some(model.id.as_str()));
            assert_eq!(lib.models.len(), 1);
            // 源目录被删后仍 valid（托管副本）。
            std::fs::remove_dir_all(&src).unwrap();
            assert!(quick_valid(&model));
        });
    }

    #[test]
    fn test_import_second_model_keeps_active() {
        run_with_temp_home(|home| {
            let a = home.join("A");
            make_valid_model(&a, "a.model3.json");
            let (model_a, _) = import_from_dir(&a).unwrap();

            let b = home.join("B");
            make_valid_model(&b, "b.model3.json");
            let (model_b, already) = import_from_dir(&b).unwrap();
            assert!(!already);
            let lib = load_library_fast().unwrap();
            // active 仍是 A，B 只是加入列表。
            assert_eq!(lib.active_model_id.as_deref(), Some(model_a.id.as_str()));
            assert_eq!(lib.models.len(), 2);
            assert!(lib.models.iter().any(|m| m.id == model_b.id));
        });
    }

    #[test]
    fn test_reimport_same_source_is_already_imported() {
        run_with_temp_home(|home| {
            let src = home.join("同款");
            make_valid_model(&src, "m.model3.json");
            let (first, _) = import_from_dir(&src).unwrap();
            let (second, already) = import_from_dir(&src).unwrap();
            assert!(already);
            assert_eq!(first.id, second.id);
            assert_eq!(load_library_fast().unwrap().models.len(), 1);
        });
    }

    #[test]
    fn test_import_rejects_missing_manifest() {
        run_with_temp_home(|home| {
            let src = home.join("empty");
            std::fs::create_dir_all(&src).unwrap();
            let err = import_from_dir(&src).unwrap_err();
            assert!(err.contains("未找到 Live2D 模型清单"));
        });
    }

    #[test]
    fn test_import_rejects_cubism2() {
        run_with_temp_home(|home| {
            let src = home.join("c2");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("model.json"), "{}").unwrap();
            let err = import_from_dir(&src).unwrap_err();
            assert!(err.contains("Cubism 2"));
        });
    }

    #[test]
    fn test_import_rejects_inside_companions_root() {
        run_with_temp_home(|_home| {
            let root = get_companions_dir();
            std::fs::create_dir_all(&root).unwrap();
            let inside = root.join("companion-abc123456789");
            std::fs::create_dir_all(&inside).unwrap();
            let err = import_from_dir(&inside).unwrap_err();
            assert!(err.contains("已托管"));
        });
    }

    #[test]
    fn test_import_fails_cleanly_on_managed_validation() {
        run_with_temp_home(|home| {
            let src = home.join("bad");
            std::fs::create_dir_all(src.join("textures")).unwrap();
            std::fs::write(src.join("textures/texture_00.png"), b"png").unwrap();
            // Moc 缺失 → 托管校验失败。
            std::fs::write(
                src.join("bad.model3.json"),
                r#"{"FileReferences":{"Moc":"missing.moc3","Textures":["textures/texture_00.png"]}}"#,
            )
            .unwrap();
            let err = import_from_dir(&src).unwrap_err();
            assert!(err.contains("Moc"), "{err}");
            // 无最终目录、无库条目、无残留 tmp。
            let root = get_companions_dir();
            assert!(!root.join("companion-").exists());
            let stray = std::fs::read_dir(&root)
                .map(|mut it| {
                    it.any(|e| {
                        e.unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with(TMP_PREFIX)
                    })
                })
                .unwrap_or(false);
            assert!(!stray, "不应残留 tmp 目录");
            assert!(load_library_fast().unwrap().models.is_empty());
        });
    }

    #[test]
    fn test_corrupt_library_returns_error_and_keeps_file() {
        run_with_temp_home(|_home| {
            let dir = get_companions_dir();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(LIBRARY_FILE), "{not-json").unwrap();
            let err = load_library_fast().unwrap_err();
            assert!(err.contains("损坏"), "{err}");
            // 原文件未被覆盖。
            let content = std::fs::read_to_string(dir.join(LIBRARY_FILE)).unwrap();
            assert_eq!(content, "{not-json");
        });
    }

    #[test]
    fn test_library_unsupported_schema_version() {
        run_with_temp_home(|_home| {
            let dir = get_companions_dir();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(LIBRARY_FILE),
                r#"{"schema_version":999,"models":[],"active_model_id":null}"#,
            )
            .unwrap();
            let err = load_library_fast().unwrap_err();
            assert!(err.contains("高于"), "{err}");
            // 不覆盖。
            let content = std::fs::read_to_string(dir.join(LIBRARY_FILE)).unwrap();
            assert!(content.contains("999"));
        });
    }

    #[test]
    fn test_library_schema_missing_defaults_to_v1() {
        run_with_temp_home(|_home| {
            let dir = get_companions_dir();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(LIBRARY_FILE),
                r#"{"models":[],"active_model_id":null}"#,
            )
            .unwrap();
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.schema_version, SCHEMA_VERSION);
        });
    }

    #[test]
    fn test_set_active_validates_model() {
        run_with_temp_home(|home| {
            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();

            let b = home.join("B");
            make_valid_model(&b, "b.model3.json");
            let (model_b, _) = import_from_dir(&b).unwrap();

            // 把 A 的托管目录删掉 → A 不可用。
            std::fs::remove_dir_all(&model.model_dir).unwrap();

            // 设置 A 为 active 应失败（模型不可用）。
            let err = set_active(&model.id).unwrap_err();
            assert!(err.contains("不可用"), "{err}");

            // B 正常设为 active。
            set_active(&model_b.id).unwrap();
            assert_eq!(
                load_library_fast().unwrap().active_model_id.as_deref(),
                Some(model_b.id.as_str())
            );
        });
    }

    #[test]
    fn test_rename_companion() {
        run_with_temp_home(|home| {
            let src = home.join("旧名");
            make_valid_model(&src, "m.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();

            rename(&model.id, "  新名字  ").unwrap(); // trim 后生效
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.models[0].name, "新名字");
            assert_eq!(lib.models[0].id, model.id);
            // 只改名字，不碰托管文件与 active。
            assert_eq!(lib.active_model_id.as_deref(), Some(model.id.as_str()));
            assert!(Path::new(&model.model_dir).is_dir());

            // 空名称 / 超长拒绝。
            assert!(rename(&model.id, "   ").is_err());
            assert!(rename(&model.id, &"很".repeat(MAX_NAME_CHARS + 1)).is_err());
            // 不存在的 id 拒绝。
            assert!(rename("companion-nope123", "x").is_err());
        });
    }

    #[test]
    fn test_remove_companion() {
        run_with_temp_home(|home| {
            let a = home.join("A");
            make_valid_model(&a, "a.model3.json");
            let (model_a, _) = import_from_dir(&a).unwrap();
            let b = home.join("B");
            make_valid_model(&b, "b.model3.json");
            let (model_b, _) = import_from_dir(&b).unwrap();

            // 删除 active A → 托管目录删除、active 落到 B。
            remove(&model_a.id).unwrap();
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.models.len(), 1);
            assert_eq!(lib.active_model_id.as_deref(), Some(model_b.id.as_str()));
            assert!(!Path::new(&model_a.model_dir).exists(), "托管目录应被删除");

            // 删除不存在的 id 报错。
            assert!(remove("companion-nope123").is_err());

            // 删除最后一个 → active 置空。
            remove(&model_b.id).unwrap();
            let lib = load_library_fast().unwrap();
            assert!(lib.models.is_empty());
            assert!(lib.active_model_id.is_none());
        });
    }

    #[test]
    fn test_save_cover_writes_png_and_is_detected() {
        run_with_temp_home(|home| {
            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();

            save_cover(&model.id, b"\x89PNG\r\n\x1a\n fake-png-bytes").unwrap();
            let cover = PathBuf::from(&model.model_dir).join("cover.png");
            assert!(cover.is_file(), "封面应写入托管目录");
            // find_cover_image 应探测到生成的 cover.png。
            let found = live2d_cfg::find_cover_image(Path::new(&model.model_dir)).unwrap();
            assert_eq!(found, cover);

            // 不存在的 id 报错，不写文件。
            assert!(save_cover("companion-nope", b"x").is_err());
        });
    }

    #[test]
    fn test_sanitize_falls_back_to_first_valid() {
        run_with_temp_home(|home| {
            let a = home.join("A");
            make_valid_model(&a, "a.model3.json");
            let (model_a, _) = import_from_dir(&a).unwrap();

            let b = home.join("B");
            make_valid_model(&b, "b.model3.json");
            let (model_b, _) = import_from_dir(&b).unwrap();

            // 把 active（A）的托管目录删掉 → load 后 sanitize 应落到 B。
            std::fs::remove_dir_all(&model_a.model_dir).unwrap();
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.active_model_id.as_deref(), Some(model_b.id.as_str()));
        });
    }

    #[test]
    fn test_sanitize_clears_active_when_all_invalid() {
        run_with_temp_home(|home| {
            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();
            std::fs::remove_dir_all(&model.model_dir).unwrap();
            let lib = load_library_fast().unwrap();
            assert!(lib.active_model_id.is_none());
        });
    }

    #[test]
    fn test_migrate_legacy_imports_and_is_idempotent() {
        run_with_temp_home(|home| {
            // 模拟旧版配置：settings.toml [live2d].model_dir 指向外部模型目录。
            let legacy = home.join("旧模型");
            make_valid_model(&legacy, "旧模型.model3.json");
            let config = settings::AppConfig {
                live2d: Some(settings::Live2dSettings {
                    model_dir: Some(legacy.display().to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            settings::save_settings(&config).unwrap();

            let migrated = migrate_legacy_if_empty().unwrap();
            assert!(migrated.is_some());
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.models.len(), 1);
            assert_eq!(lib.active_model_id, migrated);

            // 幂等：库非空后不再迁移。
            assert!(migrate_legacy_if_empty().unwrap().is_none());
        });
    }

    #[test]
    fn test_load_library_triggers_migration_when_empty() {
        run_with_temp_home(|home| {
            let legacy = home.join("旧模型");
            make_valid_model(&legacy, "旧.model3.json");
            let config = settings::AppConfig {
                live2d: Some(settings::Live2dSettings {
                    model_dir: Some(legacy.display().to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            settings::save_settings(&config).unwrap();

            // load_library（含迁移）能完成，且无死锁。
            let lib = load_library().unwrap();
            assert_eq!(lib.models.len(), 1);
            assert!(lib.active_model_id.is_some());
        });
    }

    #[test]
    fn test_tmp_dir_created_millis_parses() {
        assert_eq!(
            tmp_dir_created_millis(".tmp-companion-abcdef012345-1700000000000-3"),
            Some(1_700_000_000_000)
        );
        assert!(tmp_dir_created_millis(".tmp-companion").is_none());
        assert!(tmp_dir_created_millis("companion-abcdef012345").is_none());
    }

    #[test]
    fn test_cleanup_stale_tmp_dirs_keeps_fresh() {
        run_with_temp_home(|_home| {
            let dir = get_companions_dir();
            std::fs::create_dir_all(&dir).unwrap();
            // 新鲜 tmp（当前毫秒）应保留。
            let fresh = dir.join(new_tmp_dir_name("companion-fresh123456"));
            std::fs::create_dir_all(&fresh).unwrap();
            // 过期 tmp（时间戳为 0 = 1970）应清理。
            let stale = dir.join(format!("{TMP_PREFIX}companion-stale12345-0-0"));
            std::fs::create_dir_all(&stale).unwrap();

            cleanup_stale_tmp_dirs();

            assert!(fresh.exists(), "新鲜 tmp 应保留");
            assert!(!stale.exists(), "过期 tmp 应清理");
        });
    }

    // ---- GIF 伙伴 ----

    #[test]
    fn test_validate_gif_file_accepts_gif89a_and_gif87a() {
        run_with_temp_home(|home| {
            let g = home.join("a.gif");
            make_valid_gif(&g);
            assert!(validate_gif_file(&g).is_ok());
            std::fs::write(&g, b"GIF87a\x01\x00\x01\x00\x00").unwrap();
            assert!(validate_gif_file(&g).is_ok());
        });
    }

    #[test]
    fn test_validate_gif_file_rejects_bad_magic_and_missing() {
        run_with_temp_home(|home| {
            let bad = home.join("b.gif");
            std::fs::write(&bad, b"PNG\r\n\x1a\n").unwrap();
            assert!(validate_gif_file(&bad).is_err());
            assert!(validate_gif_file(&home.join("none.gif")).is_err());
        });
    }

    #[test]
    fn test_import_gif_file_registers_and_activates() {
        run_with_temp_home(|home| {
            let src = home.join("跳舞.gif");
            make_valid_gif(&src);
            let (model, already) = import_gif_from_file(&src).unwrap();
            assert!(!already);
            assert_eq!(model.format, "gif");
            assert_eq!(model.name, "跳舞");
            assert!(model.model_file.ends_with("跳舞.gif"));
            assert!(Path::new(&model.model_file).is_file());
            assert!(model.model_file.starts_with(&model.model_dir));
            // 首次导入自动 active；源文件删除后托管副本仍有效。
            assert_eq!(
                load_library_fast().unwrap().active_model_id.as_deref(),
                Some(model.id.as_str())
            );
            std::fs::remove_file(&src).unwrap();
            assert!(quick_valid(&model));
        });
    }

    #[test]
    fn test_import_gif_rejects_non_gif_extension_and_bad_content() {
        run_with_temp_home(|home| {
            // 内容合法但扩展名不对。
            let png = home.join("fake.png");
            make_valid_gif(&png);
            let err = import_gif_from_file(&png).unwrap_err();
            assert!(err.contains(".gif"), "{err}");
            // 扩展名对但内容不是 GIF。
            let bad = home.join("fake.gif");
            std::fs::write(&bad, b"not-a-gif").unwrap();
            let err = import_gif_from_file(&bad).unwrap_err();
            assert!(err.contains("GIF"), "{err}");
            // 失败不残留 tmp、不落库。
            assert!(load_library_fast().unwrap().models.is_empty());
            let store = crate::config::settings::get_companions_store_dir();
            let stray = std::fs::read_dir(&store).map(|mut it| {
                it.any(|e| {
                    e.unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(TMP_PREFIX)
                })
            });
            assert_eq!(stray.ok(), Some(false), "不应残留 tmp 目录");
        });
    }

    #[test]
    fn test_import_gif_same_file_dedups() {
        run_with_temp_home(|home| {
            let src = home.join("dup.gif");
            make_valid_gif(&src);
            let (first, _) = import_gif_from_file(&src).unwrap();
            let (second, already) = import_gif_from_file(&src).unwrap();
            assert!(already);
            assert_eq!(first.id, second.id);
            assert_eq!(load_library_fast().unwrap().models.len(), 1);
        });
    }

    #[test]
    fn test_import_source_dispatches_by_kind() {
        run_with_temp_home(|home| {
            // 文件 → GIF 分支
            let gif = home.join("dancer.gif");
            make_valid_gif(&gif);
            let (m, _) = import_source(&gif).unwrap();
            assert_eq!(m.format, "gif");
            // 目录 → Live2D 分支
            let dir = home.join("L");
            make_valid_model(&dir, "l.model3.json");
            let (m2, _) = import_source(&dir).unwrap();
            assert_eq!(m2.format, "cubism3");
        });
    }

    #[test]
    fn test_sanitize_and_set_active_support_gif() {
        run_with_temp_home(|home| {
            // 混合库：Live2D + GIF。
            let m = home.join("L");
            make_valid_model(&m, "l.model3.json");
            let (l2d, _) = import_from_dir(&m).unwrap();
            let g = home.join("舞.gif");
            make_valid_gif(&g);
            let (gif, _) = import_gif_from_file(&g).unwrap();

            // set_active(gif) 成功（校验走 GIF 文件头，不再走 validate_managed_model）。
            set_active(&gif.id).unwrap();
            // 删掉 Live2D 托管目录，sanitize 不应清掉 GIF active（GIF 仍有效）。
            std::fs::remove_dir_all(&l2d.model_dir).unwrap();
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.active_model_id.as_deref(), Some(gif.id.as_str()));

            // 篡改 GIF 魔数 → set_active 报「不可用」。
            std::fs::write(Path::new(&gif.model_file), b"xxxxxx").unwrap();
            let err = set_active(&gif.id).unwrap_err();
            assert!(err.contains("不可用"), "{err}");
        });
    }

    // ---- 角色包伙伴 ----

    /// 构造最小合法 PNG（签名 + 填充字节）。
    fn make_valid_png(path: &Path) {
        std::fs::write(path, b"\x89PNG\r\n\x1a\n fake-png-bytes").unwrap();
    }

    /// 构造最小合法角色包目录（character.md + character.png）。
    fn make_character_pack(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(CHARACTER_MD), "# 芙宁娜\n\n你是芙宁娜。\n").unwrap();
        make_valid_png(&dir.join(CHARACTER_PNG));
    }

    /// 用 hound 写一个指定声道数/采样率的 16-bit PCM wav（帧数 = frames）。
    fn make_wav(path: &Path, channels: u16, sample_rate: u32, frames: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames {
            for _ in 0..channels {
                writer.write_sample((i % 100) as i16).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    /// 给角色包补上 voice/（48k 立体声 reference.wav + reference.txt）。
    fn add_voice_pack(dir: &Path, frames: usize) {
        make_wav(&dir.join("voice/reference.wav"), 2, 48000, frames);
        std::fs::write(dir.join("voice/reference.txt"), "哼~没错，就是我。").unwrap();
    }

    #[test]
    fn test_validate_character_pack_ok() {
        run_with_temp_home(|home| {
            let dir = home.join("furina");
            make_character_pack(&dir);
            assert!(validate_character_pack(&dir).is_ok());
            // 带成对 voice/ 也合法
            add_voice_pack(&dir, 100);
            assert!(validate_character_pack(&dir).is_ok());
        });
    }

    #[test]
    fn test_validate_character_pack_errors() {
        run_with_temp_home(|home| {
            // 缺 character.md
            let dir = home.join("a");
            std::fs::create_dir_all(&dir).unwrap();
            make_valid_png(&dir.join(CHARACTER_PNG));
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("character.md"), "{err}");

            // character.md 空白
            let dir = home.join("b");
            make_character_pack(&dir);
            std::fs::write(dir.join(CHARACTER_MD), "  \n\n").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("为空"), "{err}");

            // character.png 缺失 / 坏签名
            let dir = home.join("c");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(CHARACTER_MD), "# X\n").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("character.png"), "{err}");
            std::fs::write(dir.join(CHARACTER_PNG), b"not-a-png").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("PNG"), "{err}");

            // voice/ 缺 wav / 缺 txt / txt 空白 / wav 非 RIFF
            let dir = home.join("d");
            make_character_pack(&dir);
            std::fs::create_dir_all(dir.join(VOICE_DIR)).unwrap();
            std::fs::write(dir.join("voice/reference.txt"), "文本").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("reference.wav"), "{err}");

            let dir = home.join("e");
            make_character_pack(&dir);
            make_wav(&dir.join("voice/reference.wav"), 1, 48000, 10);
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("reference.txt"), "{err}");

            let dir = home.join("f");
            make_character_pack(&dir);
            make_wav(&dir.join("voice/reference.wav"), 1, 48000, 10);
            std::fs::write(dir.join("voice/reference.txt"), "   ").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("为空"), "{err}");

            let dir = home.join("g");
            make_character_pack(&dir);
            std::fs::create_dir_all(dir.join(VOICE_DIR)).unwrap();
            std::fs::write(dir.join("voice/reference.wav"), b"not-a-wav-file").unwrap();
            std::fs::write(dir.join("voice/reference.txt"), "文本").unwrap();
            let err = validate_character_pack(&dir).unwrap_err();
            assert!(err.contains("wav"), "{err}");
        });
    }

    #[test]
    fn test_import_character_pack_registers_and_activates() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_character_pack(&src);
            add_voice_pack(&src, 240);

            let (model, already) = import_character_from_dir(&src).unwrap();
            assert!(!already);
            assert_eq!(model.format, CHARACTER_FORMAT);
            assert_eq!(model.name, "芙宁娜", "角色名应取 character.md 的 H1");
            assert!(model.model_file.ends_with("character.png"));
            assert!(Path::new(&model.model_file).is_file());
            assert!(model.model_file.starts_with(&model.model_dir));
            // 首次导入自动 active；源目录删除后托管副本仍有效。
            assert_eq!(
                load_library_fast().unwrap().active_model_id.as_deref(),
                Some(model.id.as_str())
            );
            std::fs::remove_dir_all(&src).unwrap();
            assert!(quick_valid(&model));
        });
    }

    #[test]
    fn test_import_character_name_fallback_and_truncation() {
        run_with_temp_home(|home| {
            // 无 H1 → 回退目录 basename
            let src = home.join("无名目录");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join(CHARACTER_MD), "没有人设标题。\n").unwrap();
            make_valid_png(&src.join(CHARACTER_PNG));
            let (model, _) = import_character_from_dir(&src).unwrap();
            assert_eq!(model.name, "无名目录");

            // H1 超长 → 截断到 MAX_NAME_CHARS
            let src = home.join("长名");
            std::fs::create_dir_all(&src).unwrap();
            let long_name = "很".repeat(MAX_NAME_CHARS + 10);
            std::fs::write(src.join(CHARACTER_MD), format!("# {long_name}\n")).unwrap();
            make_valid_png(&src.join(CHARACTER_PNG));
            let (model, _) = import_character_from_dir(&src).unwrap();
            assert_eq!(model.name.chars().count(), MAX_NAME_CHARS);
        });
    }

    #[test]
    fn test_import_source_dispatch_character_priority() {
        run_with_temp_home(|home| {
            // 目录同时含 model3.json 与 character.md → 角色包优先
            let dir = home.join("both");
            make_character_pack(&dir);
            std::fs::write(dir.join("m.model3.json"), "{}").unwrap();
            let (m, _) = import_source(&dir).unwrap();
            assert_eq!(m.format, CHARACTER_FORMAT);

            // 目录含 character.md 但缺 character.png → 角色包专属错误，不回退 Live2D 分支
            let dir = home.join("broken");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(CHARACTER_MD), "# X\n").unwrap();
            let err = import_source(&dir).unwrap_err();
            assert!(err.contains("character.png"), "{err}");
            assert!(!err.contains("Live2D"), "{err}");
        });
    }

    #[test]
    fn test_import_character_rejects_bad_voice_pair() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_character_pack(&src);
            std::fs::create_dir_all(src.join(VOICE_DIR)).unwrap();
            // 只有 wav 没有 txt → 成对错误
            make_wav(&src.join("voice/reference.wav"), 1, 48000, 10);
            let err = import_character_from_dir(&src).unwrap_err();
            assert!(err.contains("reference.txt"), "{err}");
            // 失败不残留 tmp、不落库
            assert!(load_library_fast().unwrap().models.is_empty());
            let store = crate::config::settings::get_companions_store_dir();
            let stray = std::fs::read_dir(&store).map(|mut it| {
                it.any(|e| {
                    e.unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with(TMP_PREFIX)
                })
            });
            assert_eq!(stray.ok(), Some(false), "不应残留 tmp 目录");
        });
    }

    #[test]
    fn test_convert_reference_to_mono() {
        let dir = tempfile::tempdir().unwrap();
        // 立体声 48k → 单声道，采样率保留，帧数不变
        let wav = dir.path().join("stereo.wav");
        make_wav(&wav, 2, 48000, 480);
        assert!(convert_reference_to_mono(&wav).unwrap());
        let reader = hound::WavReader::open(&wav).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48000);
        assert_eq!(reader.duration(), 480);

        // 单声道 → 不改写（返回 false，内容不变）
        let mono = dir.path().join("mono.wav");
        make_wav(&mono, 1, 24000, 100);
        let before = std::fs::read(&mono).unwrap();
        assert!(!convert_reference_to_mono(&mono).unwrap());
        assert_eq!(std::fs::read(&mono).unwrap(), before);
    }

    #[test]
    fn test_import_character_converts_voice_to_mono() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_character_pack(&src);
            add_voice_pack(&src, 960);

            let (model, _) = import_character_from_dir(&src).unwrap();
            // 托管副本被改写为单声道 48k；源目录保持立体声不变。
            let managed = Path::new(&model.model_dir).join("voice/reference.wav");
            let spec = hound::WavReader::open(&managed).unwrap().spec();
            assert_eq!(spec.channels, 1, "托管参考音频应为单声道");
            assert_eq!(spec.sample_rate, 48000);
            let src_spec = hound::WavReader::open(src.join("voice/reference.wav"))
                .unwrap()
                .spec();
            assert_eq!(src_spec.channels, 2, "源目录不应被改写");
        });
    }

    #[test]
    fn test_sanitize_and_set_active_support_character() {
        run_with_temp_home(|home| {
            let g = home.join("舞.gif");
            make_valid_gif(&g);
            let (gif, _) = import_gif_from_file(&g).unwrap();
            let c = home.join("furina");
            make_character_pack(&c);
            let (ch, _) = import_character_from_dir(&c).unwrap();

            // set_active(character) 成功（校验走角色包结构）。
            set_active(&ch.id).unwrap();
            // 删掉 GIF 托管目录，sanitize 不应清掉角色包 active。
            std::fs::remove_dir_all(&gif.model_dir).unwrap();
            let lib = load_library_fast().unwrap();
            assert_eq!(lib.active_model_id.as_deref(), Some(ch.id.as_str()));

            // 篡改 character.png 魔数 → set_active 报「不可用」。
            std::fs::write(Path::new(&ch.model_file), b"xxxxxxxx").unwrap();
            let err = set_active(&ch.id).unwrap_err();
            assert!(err.contains("不可用"), "{err}");
        });
    }

    #[test]
    fn test_active_persona_and_voice_detection() {
        run_with_temp_home(|home| {
            // 无 active → None
            assert!(active_persona().is_none());
            assert!(active_companion_voice().is_none());

            let c = home.join("furina");
            make_character_pack(&c);
            add_voice_pack(&c, 100);
            let (ch, _) = import_character_from_dir(&c).unwrap();
            let persona = active_persona().unwrap();
            assert!(persona.contains("芙宁娜"), "{persona}");
            let voice = active_companion_voice().unwrap();
            assert!(voice.wav.ends_with("voice/reference.wav"));
            assert_eq!(voice.text, "哼~没错，就是我。");
            assert!(has_persona(&ch));
            assert!(has_voice(&ch));

            // 切到普通 GIF 伙伴 → 人设探测为 None（人设仍限定角色包）
            let g = home.join("舞.gif");
            make_valid_gif(&g);
            let (gif, _) = import_gif_from_file(&g).unwrap();
            set_active(&gif.id).unwrap();
            assert!(active_persona().is_none());
            assert!(!has_persona(&gif));
            // 音色不再限定角色包：绑定后 GIF 伙伴同样生效（见绑定解析测试）
        });
    }

    // ---- 伙伴音色绑定（目录 > 音色库绑定 > 全局默认） ----

    /// 往音色库存一条音色，返回其 id。
    fn save_library_voice(home: &Path, name: &str, text: &str) -> String {
        let src = home.join(format!("{name}-src.wav"));
        make_wav(&src, 1, 16000, 10);
        crate::tts::voice_store::save_voice(name, &src, text)
            .unwrap()
            .id
    }

    #[test]
    fn test_companion_voice_resolution_priority() {
        run_with_temp_home(|home| {
            let lib_id = save_library_voice(home, "库音色", "库转写");
            let another_id = save_library_voice(home, "另一个", "另一转写");

            // 角色包自带 voice/ 且绑定另一音色 → 目录优先（Pack）
            let c = home.join("furina");
            make_character_pack(&c);
            add_voice_pack(&c, 100);
            let (ch, _) = import_character_from_dir(&c).unwrap();
            set_voice_binding(&ch.id, Some(&another_id)).unwrap();
            let (voice, source) =
                companion_voice_in(&load_library_fast().unwrap().models[0]).unwrap();
            assert_eq!(source, CompanionVoiceSource::Pack);
            assert!(voice.wav.ends_with("voice/reference.wav"));
            assert_eq!(voice.text, "哼~没错，就是我。");

            // Live2D 伙伴无目录音色 + 绑定 → 绑定生效（Library，wav 指向音色库）
            let m = home.join("L");
            make_valid_model(&m, "l.model3.json");
            let (l2d, _) = import_from_dir(&m).unwrap();
            set_voice_binding(&l2d.id, Some(&lib_id)).unwrap();
            let lib = load_library_fast().unwrap();
            let model = lib.models.iter().find(|m| m.id == l2d.id).unwrap();
            let (voice, source) = companion_voice_in(model).unwrap();
            assert_eq!(source, CompanionVoiceSource::Library);
            assert_eq!(
                voice.wav,
                crate::tts::voice_store::find_voice_by_id(&lib_id)
                    .unwrap()
                    .wav_path
            );
            assert_eq!(voice.text, "库转写");
            assert!(has_voice(model));

            // 未绑定且无目录音色 → None（上层回退全局默认）
            let g = home.join("舞.gif");
            make_valid_gif(&g);
            let (gif, _) = import_gif_from_file(&g).unwrap();
            let lib = load_library_fast().unwrap();
            let model = lib.models.iter().find(|m| m.id == gif.id).unwrap();
            assert!(companion_voice_in(model).is_none());
            assert!(!has_voice(model));
        });
    }

    /// 绑定指向的音色被删除 → fail-open 回退 None（不 panic、不误用其它音色）。
    #[test]
    fn test_stale_binding_falls_back_to_none() {
        run_with_temp_home(|home| {
            let m = home.join("L");
            make_valid_model(&m, "l.model3.json");
            let (l2d, _) = import_from_dir(&m).unwrap();
            let lib_id = save_library_voice(home, "会消失", "转写");
            set_voice_binding(&l2d.id, Some(&lib_id)).unwrap();
            assert!(has_voice(&load_library_fast().unwrap().models[0]));

            crate::tts::voice_store::delete_voice(&lib_id).unwrap();
            let model = &load_library_fast().unwrap().models[0];
            // 绑定字段还在（引用残留），但解析为 None（fail-open）
            assert_eq!(model.voice_id.as_deref(), Some(lib_id.as_str()));
            assert!(companion_voice_in(model).is_none());
            assert!(!has_voice(model), "失效绑定不算生效音色");
            assert!(active_companion_voice().is_none());
        });
    }

    #[test]
    fn test_set_voice_binding_validation() {
        run_with_temp_home(|home| {
            let m = home.join("L");
            make_valid_model(&m, "l.model3.json");
            let (l2d, _) = import_from_dir(&m).unwrap();

            // 绑不存在的 id 报错
            let err = set_voice_binding(&l2d.id, Some("custom-nope")).unwrap_err();
            assert!(err.contains("未找到音色"), "err: {err}");
            // 绑定后字段落库
            let lib_id = save_library_voice(home, "音色A", "转写");
            set_voice_binding(&l2d.id, Some(&lib_id)).unwrap();
            assert_eq!(
                load_library_fast().unwrap().models[0].voice_id.as_deref(),
                Some(lib_id.as_str())
            );
            // None 解绑
            set_voice_binding(&l2d.id, None).unwrap();
            assert!(load_library_fast().unwrap().models[0].voice_id.is_none());
            // 未知的伙伴 id 报错
            assert!(set_voice_binding("companion-nope", None).is_err());
        });
    }

    #[test]
    fn test_clear_voice_bindings_only_hits_matching() {
        run_with_temp_home(|home| {
            let a = home.join("A");
            make_valid_model(&a, "a.model3.json");
            let (ca, _) = import_from_dir(&a).unwrap();
            let b = home.join("B");
            make_valid_model(&b, "b.model3.json");
            let (cb, _) = import_from_dir(&b).unwrap();
            let gone = save_library_voice(home, "被删", "转写");
            let keep = save_library_voice(home, "保留", "转写");
            set_voice_binding(&ca.id, Some(&gone)).unwrap();
            set_voice_binding(&cb.id, Some(&keep)).unwrap();

            let affected = clear_voice_bindings(&gone).unwrap();
            assert_eq!(affected, vec![ca.id.clone()]);
            let lib = load_library_fast().unwrap();
            let ma = lib.models.iter().find(|m| m.id == ca.id).unwrap();
            let mb = lib.models.iter().find(|m| m.id == cb.id).unwrap();
            assert!(ma.voice_id.is_none(), "命中的绑定应被清理");
            assert_eq!(mb.voice_id.as_deref(), Some(keep.as_str()), "无关绑定不动");

            // 无引用 → 空列表
            assert!(clear_voice_bindings(&gone).unwrap().is_empty());
        });
    }

    /// 老 library.json（条目无 voice_id 字段）宽容加载为 None。
    #[test]
    fn test_library_json_without_voice_field_loads() {
        run_with_temp_home(|home| {
            let root = get_companions_dir();
            let id = "companion-abc";
            make_valid_model(&root.join(id), "cat.model3.json");
            let json_path = |p: &Path| serde_json::to_string(&p.display().to_string()).unwrap();
            std::fs::write(
                root.join(LIBRARY_FILE),
                format!(
                    r#"{{"schema_version":1,"models":[{{"id":"{id}","name":"cat","model_dir":{},"model_file":{},"format":"cubism3","imported_at":"t"}}],"active_model_id":"{id}"}}"#,
                    json_path(&root.join(id)),
                    json_path(&root.join(id).join("cat.model3.json"))
                ),
            )
            .unwrap();
            let lib = load_library_fast().unwrap();
            assert!(lib.models[0].voice_id.is_none());
            assert!(lib.models[0].layout.is_none());
        });
    }

    /// 非 character format 的托管目录手动放入 voice/ 同样生效（第 1 级不限 format）。
    #[test]
    fn test_dir_voice_applies_to_any_format() {
        run_with_temp_home(|home| {
            let m = home.join("L");
            make_valid_model(&m, "l.model3.json");
            let (l2d, _) = import_from_dir(&m).unwrap();
            // 用户经 open_companion_dir 手动放入音色参考
            add_voice_pack(Path::new(&l2d.model_dir), 50);

            let model = &load_library_fast().unwrap().models[0];
            assert!(has_voice(model), "Live2D 伙伴目录自带音色应生效");
            let (voice, source) = companion_voice_in(model).unwrap();
            assert_eq!(source, CompanionVoiceSource::Pack);
            assert!(voice.wav.ends_with("voice/reference.wav"));
        });
    }

    // ---- 伙伴私有布局（尺寸/位置） ----

    #[test]
    fn test_layout_save_and_reload_roundtrip() {
        run_with_temp_home(|home| {
            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();
            // 新导入的伙伴无私有布局。
            assert!(model.layout.is_none());

            save_layout_scale(&model.id, 1.5).unwrap();
            save_layout_position(&model.id, 120, 800).unwrap();

            let lib = load_library_fast().unwrap();
            let m = lib.models.iter().find(|m| m.id == model.id).unwrap();
            let layout = m.layout.as_ref().unwrap();
            assert_eq!(layout.scale, Some(1.5));
            assert_eq!(
                layout.position,
                Some(settings::CompanionWindowPosition { x: 120, y: 800 })
            );
        });
    }

    #[test]
    fn test_layout_incremental_update_keeps_other_field() {
        run_with_temp_home(|home| {
            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();

            save_layout_scale(&model.id, 1.5).unwrap();
            save_layout_position(&model.id, 10, 20).unwrap();
            // 再改 scale：position 不被清掉。
            save_layout_scale(&model.id, 0.8).unwrap();

            let lib = load_library_fast().unwrap();
            let layout = lib.models[0].layout.as_ref().unwrap();
            assert_eq!(layout.scale, Some(0.8));
            assert_eq!(
                layout.position,
                Some(settings::CompanionWindowPosition { x: 10, y: 20 })
            );
        });
    }

    #[test]
    fn test_layout_save_unknown_id_errors() {
        run_with_temp_home(|_home| {
            assert!(save_layout_scale("companion-nope", 1.0).is_err());
            assert!(save_layout_position("companion-nope", 0, 0).is_err());
        });
    }

    #[test]
    fn test_active_layout_reads_active_companion() {
        run_with_temp_home(|home| {
            // 无 active → None。
            assert!(active_layout().is_none());

            let src = home.join("A");
            make_valid_model(&src, "a.model3.json");
            let (model, _) = import_from_dir(&src).unwrap();
            // active 但未配置 → None。
            assert!(active_layout().is_none());

            save_layout_scale(&model.id, 1.2).unwrap();
            let layout = active_layout().unwrap();
            assert_eq!(layout.scale, Some(1.2));
            assert!(layout.position.is_none());
        });
    }

    #[test]
    fn test_library_json_without_layout_field_loads() {
        run_with_temp_home(|home| {
            // 老版本 library.json（条目无 layout 字段）宽容加载为 None。
            let root = get_companions_dir();
            let id = "companion-abc";
            make_valid_model(&root.join(id), "cat.model3.json");
            // 路径须经 JSON 字符串转义后再拼入，否则 Windows 反斜杠路径是非法 JSON。
            let json_path = |p: &Path| serde_json::to_string(&p.display().to_string()).unwrap();
            std::fs::write(
                root.join(LIBRARY_FILE),
                format!(
                    r#"{{"schema_version":1,"models":[{{"id":"{id}","name":"cat","model_dir":{},"model_file":{},"format":"cubism3","imported_at":"t"}}],"active_model_id":"{id}"}}"#,
                    json_path(&root.join(id)),
                    json_path(&root.join(id).join("cat.model3.json"))
                ),
            )
            .unwrap();
            let lib = load_library_fast().unwrap();
            assert!(lib.models[0].layout.is_none());
        });
    }

    #[test]
    fn test_concurrent_import_same_source_yields_single_companion() {
        run_with_temp_home(|home| {
            let src = home.join("同款并发");
            make_valid_model(&src, "m.model3.json");

            let mut handles = Vec::new();
            for _ in 0..4 {
                let src = src.clone();
                handles.push(std::thread::spawn(move || {
                    import_from_dir(&src).map(|(_m, already)| already)
                }));
            }
            let results: Vec<bool> = handles
                .into_iter()
                .map(|h| h.join().unwrap().unwrap())
                .collect();
            // 至少一个不是 already_imported；其余是。
            assert!(results.iter().any(|a| !a));
            assert_eq!(results.iter().filter(|a| !*a).count(), 1);

            let lib = load_library_fast().unwrap();
            assert_eq!(lib.models.len(), 1);
            // 磁盘上只有一个正式托管目录。
            let dirs: Vec<_> = std::fs::read_dir(get_companions_dir())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("companion-"))
                .collect();
            assert_eq!(dirs.len(), 1, "并发导入只能有一个正式目录: {dirs:?}");
        });
    }

    // ==================== 唤醒词 / 欢迎语 ====================

    /// 直接写一份含单个伙伴的库清单（绕过导入流程，聚焦字段语义）。
    /// 托管目录必须通过 `validate_managed`（load_library_fast 会校正 active），
    /// 因此 seed 同步落一份最小合法模型。
    fn seed_library(id: &str, name: &str, wake_word: Option<&str>, welcome: Option<&str>) {
        let model_dir = get_companions_dir().join(id);
        make_valid_model(&model_dir, "a.model3.json");
        let lib = CompanionLibrary {
            schema_version: SCHEMA_VERSION,
            models: vec![CompanionModel {
                id: id.to_string(),
                name: name.to_string(),
                source_path: None,
                model_dir: model_dir.display().to_string(),
                model_file: model_dir.join("a.model3.json").display().to_string(),
                format: "cubism3".into(),
                imported_at: "t".into(),
                voice_id: None,
                layout: None,
                wake_word: wake_word.map(str::to_string),
                welcome_text: welcome.map(str::to_string),
            }],
            active_model_id: Some(id.to_string()),
            completed_migrations: Vec::new(),
        };
        std::fs::create_dir_all(get_companions_dir()).unwrap();
        std::fs::write(
            get_companions_dir().join(LIBRARY_FILE),
            serde_json::to_string_pretty(&lib).unwrap(),
        )
        .unwrap();
    }

    /// 写最小 KWS token 集（`n ǐ` 已 tokenized 序列可透传；不在集内的词编码失败）。
    fn seed_tokens(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("tokens.txt");
        std::fs::write(&path, "n\nǐ\nh\nǎo\nx\niǎo\nzh\nì\n").unwrap();
        path
    }

    #[test]
    fn test_wake_and_welcome_fields_serde_defaults() {
        // 老 JSON（无新字段）→ 宽容默认 None，不报错。
        let legacy = r#"{"id":"companion-x","name":"猫","model_dir":"/m","model_file":"/m/a.model3.json","format":"cubism3","imported_at":"t"}"#;
        let m: CompanionModel = serde_json::from_str(legacy).unwrap();
        assert_eq!(m.wake_word, None);
        assert_eq!(m.welcome_text, None);

        let full = CompanionModel {
            id: "companion-x".into(),
            name: "猫".into(),
            source_path: None,
            model_dir: "/m".into(),
            model_file: "/m/a.model3.json".into(),
            format: "cubism3".into(),
            imported_at: "t".into(),
            voice_id: None,
            layout: None,
            wake_word: Some("小猫".into()),
            welcome_text: None,
        };
        let json = serde_json::to_string(&full).unwrap();
        assert!(json.contains("小猫"));
        assert!(!json.contains("welcome_text"), "None 字段不序列化");
        let back: CompanionModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, full);
    }

    #[test]
    fn test_effective_wake_word_and_welcome_text() {
        let mut m = CompanionModel {
            id: "companion-x".into(),
            name: "大月下".into(),
            source_path: None,
            model_dir: "/m".into(),
            model_file: "/m/a.model3.json".into(),
            format: "cubism3".into(),
            imported_at: "t".into(),
            voice_id: None,
            layout: None,
            wake_word: None,
            welcome_text: None,
        };
        // 未自定义：唤醒词跟随 name，欢迎语按模板展开。
        assert_eq!(effective_wake_word(&m), "大月下");
        assert_eq!(effective_welcome_text(&m), "你好，我是大月下。");
        // rename 后未自定义的唤醒词自动跟随。
        m.name = "新月下".into();
        assert_eq!(effective_wake_word(&m), "新月下");
        assert_eq!(effective_welcome_text(&m), "你好，我是新月下。");
        // 自定义优先。
        m.wake_word = Some("小月".into());
        m.welcome_text = Some("嗨！".into());
        assert_eq!(effective_wake_word(&m), "小月");
        assert_eq!(effective_welcome_text(&m), "嗨！");
    }

    #[test]
    fn test_set_wake_word_and_welcome_text_roundtrip() {
        run_with_temp_home(|_home| {
            seed_library("companion-w", "猫猫", None, None);
            let lib = set_wake_word("companion-w", Some("  喵喵  ")).unwrap();
            let m = lib.models.first().unwrap();
            assert_eq!(m.wake_word.as_deref(), Some("喵喵"), "trim 后入库");

            let lib = set_welcome_text("companion-w", Some("   ")).unwrap();
            assert_eq!(
                lib.models.first().unwrap().welcome_text,
                None,
                "空白串归一为 None（恢复默认）"
            );
            let lib = set_wake_word("companion-w", None).unwrap();
            assert_eq!(lib.models.first().unwrap().wake_word, None);

            let long: String = "字".repeat(MAX_WAKE_WORD_CHARS + 1);
            assert!(
                set_wake_word("companion-w", Some(&long)).is_err(),
                "超长报错"
            );
            let long: String = "字".repeat(MAX_WELCOME_CHARS + 1);
            assert!(set_welcome_text("companion-w", Some(&long)).is_err());
            assert!(
                set_wake_word("companion-nope", Some("x")).is_err(),
                "未知 id 报错"
            );
        });
    }

    #[test]
    fn test_resolve_wake_word_priority_and_fallback() {
        run_with_temp_home(|home| {
            let tokens = seed_tokens(&home.join("kws-model"));

            // 无 active 伙伴 → 原样回退 fallback。
            let r = resolve_wake_word(Some("全局词"), &tokens);
            assert_eq!(r.word.as_deref(), Some("全局词"));
            assert_eq!(r.companion_word, None);
            assert!(r.companion_ok);

            // active 伙伴自定义可编码唤醒词 → 压过 fallback（激活即换词语义）。
            seed_library("companion-r", "猫猫", Some("n ǐ"), None);
            let r = resolve_wake_word(Some("全局词"), &tokens);
            assert_eq!(r.word.as_deref(), Some("n ǐ"));
            assert_eq!(r.companion_word.as_deref(), Some("n ǐ"));
            assert!(r.companion_ok);

            // 角色词不可编码 → 回退 fallback 且 companion_ok = false。
            seed_library("companion-r", "猫猫", Some("😂😂"), None);
            let r = resolve_wake_word(Some("全局词"), &tokens);
            assert_eq!(r.word.as_deref(), Some("全局词"));
            assert_eq!(r.companion_word.as_deref(), Some("😂😂"));
            assert!(!r.companion_ok);

            // 未自定义 → 跟随 name；名字不可编码同样回退。
            seed_library("companion-r", "😂", None, None);
            let r = resolve_wake_word(Some("全局词"), &tokens);
            assert_eq!(r.word.as_deref(), Some("全局词"));
            assert!(!r.companion_ok);
        });
    }

    // ==================== character.json（作者预设声明） ====================

    /// 构造含 character.md/png 的源角色包目录。
    fn make_character_source(dir: &Path, h1: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(CHARACTER_MD), format!("# {h1}\n\n你是{h1}。\n")).unwrap();
        std::fs::write(dir.join(CHARACTER_PNG), b"\x89PNG\r\n\x1a\n png").unwrap();
    }

    #[test]
    fn test_manifest_preset_prefills_on_import() {
        run_with_temp_home(|home| {
            let src = home.join("furina");
            make_character_source(&src, "错误的H1名字");
            // 四字段齐备：name 优先于 H1；wake_word/welcome_text 预填进库。
            std::fs::write(
                src.join(CHARACTER_JSON),
                r#"{"version":1,"name":"芙宁娜","wake_word":"水神","welcome_text":"哼~没错，就是我。"}"#,
            )
            .unwrap();

            let (model, already) = import_character_from_dir(&src).unwrap();
            assert!(!already);
            assert_eq!(model.name, "芙宁娜", "manifest.name 优先于 H1");
            assert_eq!(model.wake_word.as_deref(), Some("水神"));
            assert_eq!(model.welcome_text.as_deref(), Some("哼~没错，就是我。"));
        });
    }

    #[test]
    fn test_manifest_missing_falls_back_to_h1_and_none_fields() {
        run_with_temp_home(|home| {
            let src = home.join("大月下");
            make_character_source(&src, "大月下");
            let (model, _) = import_character_from_dir(&src).unwrap();
            // 无 character.json → 照旧走 H1 推导，预设字段为 None。
            assert_eq!(model.name, "大月下");
            assert_eq!(model.wake_word, None);
            assert_eq!(model.welcome_text, None);
        });
    }

    #[test]
    fn test_manifest_blank_fields_are_none_and_garbage_fails_import() {
        run_with_temp_home(|home| {
            // 空白字段 → 预填归 None（不产生"空唤醒词"）。
            let src = home.join("blank");
            make_character_source(&src, "空白字段");
            std::fs::write(
                src.join(CHARACTER_JSON),
                r#"{"version":1,"name":"  ","wake_word":"   ","welcome_text":""}"#,
            )
            .unwrap();
            let (model, _) = import_character_from_dir(&src).unwrap();
            // name 空白 → 回退 H1。
            assert_eq!(model.name, "空白字段");
            assert_eq!(model.wake_word, None);
            assert_eq!(model.welcome_text, None);

            // 损坏声明 → 导入报错（不静默忽略，让作者修格式）。
            let bad = home.join("bad");
            make_character_source(&bad, "坏声明");
            std::fs::write(bad.join(CHARACTER_JSON), "{not json").unwrap();
            assert!(import_character_from_dir(&bad).is_err());
        });
    }

    #[test]
    fn test_validate_character_pack_manifest_optional_but_strict() {
        run_with_temp_home(|home| {
            let dir = home.join("pack");
            make_character_source(&dir, "猫");
            // 合法声明通过。
            std::fs::write(dir.join(CHARACTER_JSON), r#"{"version":1,"name":"猫"}"#).unwrap();
            assert!(validate_character_pack(&dir).is_ok());
            // 缺失通过（存量角色包兼容）。
            std::fs::remove_file(dir.join(CHARACTER_JSON)).unwrap();
            assert!(validate_character_pack(&dir).is_ok());
            // 损坏报错。
            std::fs::write(dir.join(CHARACTER_JSON), "[]").unwrap();
            assert!(validate_character_pack(&dir).is_err());
        });
    }

    // ==================== 音色上传覆盖 / 恢复 ====================

    /// 构造「带自带音色」的角色包并导入（返回导入的伙伴 id）。
    fn import_pack_with_voice(home: &Path, name: &str) -> String {
        let src = home.join(name);
        make_character_source(&src, name);
        std::fs::create_dir_all(src.join(VOICE_DIR)).unwrap();
        make_wav(&src.join(VOICE_DIR).join(REFERENCE_WAV), 1, 16_000, 160);
        std::fs::write(src.join(VOICE_DIR).join(REFERENCE_TXT), "作者原版转写").unwrap();
        import_character_from_dir(&src).unwrap().0.id
    }

    #[test]
    fn test_upload_voice_overrides_and_backs_up_original() {
        run_with_temp_home(|home| {
            let id = import_pack_with_voice(home, "芙宁娜");
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            let paths = voice_paths(Path::new(&model.model_dir));
            let original_wav = std::fs::read(&paths.wav).unwrap();
            let original_txt = std::fs::read_to_string(&paths.txt).unwrap();
            assert!(!has_original_voice(&model));

            // 上传新音色（立体声 → 应被转 mono）。
            let up = home.join("custom.wav");
            make_wav(&up, 2, 44_100, 2);
            upload_companion_voice(&id, &up, "自定义转写").unwrap();

            // 生效音色切换 + 作者原版完整备份。
            assert_eq!(std::fs::read_to_string(&paths.txt).unwrap(), "自定义转写");
            assert_ne!(std::fs::read(&paths.wav).unwrap(), original_wav);
            assert_eq!(std::fs::read(&paths.original_wav).unwrap(), original_wav);
            assert_eq!(
                std::fs::read_to_string(&paths.original_txt).unwrap(),
                original_txt
            );
            assert!(has_original_voice(&model));
            // 立体声已转单声道。
            let spec = hound::WavReader::open(&paths.wav).unwrap().spec();
            assert_eq!(spec.channels, 1);
            // 解析链命中 Pack（零改动验证）。
            let (voice, source) = companion_voice_in(&model).unwrap();
            assert_eq!(voice.text, "自定义转写");
            assert_eq!(source, CompanionVoiceSource::Pack);
            // 上传残留在托管目录的 tmp 已清理。
            assert!(!paths.wav.with_extension("upload.tmp.wav").exists());
        });
    }

    #[test]
    fn test_upload_voice_repeated_keeps_first_backup() {
        run_with_temp_home(|home| {
            let id = import_pack_with_voice(home, "芙宁娜");
            let up1 = home.join("a.wav");
            make_wav(&up1, 1, 16_000, 160);
            upload_companion_voice(&id, &up1, "第一次").unwrap();
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            let paths = voice_paths(Path::new(&model.model_dir));
            let backup = std::fs::read(&paths.original_wav).unwrap();

            let up2 = home.join("b.wav");
            make_wav(&up2, 1, 16_000, 160);
            upload_companion_voice(&id, &up2, "第二次").unwrap();
            // 备份仍是作者原版（不是第一次上传的版本）。
            assert_eq!(std::fs::read(&paths.original_wav).unwrap(), backup);
            assert_eq!(
                std::fs::read_to_string(&paths.original_txt).unwrap(),
                "作者原版转写"
            );
            // 生效的是第二次上传。
            assert_eq!(std::fs::read_to_string(&paths.txt).unwrap(), "第二次");
        });
    }

    #[test]
    fn test_upload_voice_without_existing_voice_creates_pack_voice() {
        run_with_temp_home(|home| {
            // 角色包不带 voice/（或 GIF/Live2D）→ 首传不建备份，直接生成 voice/。
            let src = home.join("novoice");
            make_character_source(&src, "无声角色");
            let id = import_character_from_dir(&src).unwrap().0.id;
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            assert!(companion_voice_in(&model).is_none());

            let up = home.join("new.wav");
            make_wav(&up, 1, 16_000, 160);
            upload_companion_voice(&id, &up, "首次上传").unwrap();
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            let (voice, source) = companion_voice_in(&model).unwrap();
            assert_eq!(voice.text, "首次上传");
            assert_eq!(source, CompanionVoiceSource::Pack);
            assert!(!has_original_voice(&model), "无原版可备份");
        });
    }

    #[test]
    fn test_upload_voice_rejects_corrupt_and_empty_and_unknown() {
        run_with_temp_home(|home| {
            let id = import_pack_with_voice(home, "芙宁娜");
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            let paths = voice_paths(Path::new(&model.model_dir));
            let wav_before = std::fs::read(&paths.wav).unwrap();

            // 损坏 wav → Err，原文件完好且无备份产生。
            let bad = home.join("bad.wav");
            std::fs::write(&bad, b"RIFFxxxxWAVEjunk").unwrap();
            assert!(upload_companion_voice(&id, &bad, "转写").is_err());
            assert_eq!(std::fs::read(&paths.wav).unwrap(), wav_before);
            assert!(!paths.original_wav.exists());

            // 空转写 → Err。
            let ok = home.join("ok.wav");
            make_wav(&ok, 1, 16_000, 160);
            assert!(upload_companion_voice(&id, &ok, "   ").is_err());
            // 未知 id → Err。
            assert!(upload_companion_voice("companion-nope", &ok, "转写").is_err());
            assert_eq!(std::fs::read(&paths.wav).unwrap(), wav_before);
        });
    }

    #[test]
    fn test_restore_voice_roundtrip_and_edge_cases() {
        run_with_temp_home(|home| {
            let id = import_pack_with_voice(home, "芙宁娜");
            // 无备份时恢复 → Err。
            assert!(restore_companion_voice(&id).is_err());

            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            let paths = voice_paths(Path::new(&model.model_dir));
            let author_wav = std::fs::read(&paths.wav).unwrap();
            let author_txt = std::fs::read_to_string(&paths.txt).unwrap();

            let up = home.join("custom.wav");
            make_wav(&up, 1, 16_000, 160);
            upload_companion_voice(&id, &up, "自定义").unwrap();

            // 恢复 → 与作者原版字节一致、备份删除、flag false。
            restore_companion_voice(&id).unwrap();
            assert_eq!(std::fs::read(&paths.wav).unwrap(), author_wav);
            assert_eq!(std::fs::read_to_string(&paths.txt).unwrap(), author_txt);
            assert!(!paths.original_wav.exists());
            assert!(!paths.original_txt.exists());
            let model = load_library_fast().unwrap();
            let model = model.models.into_iter().find(|m| m.id == id).unwrap();
            assert!(!has_original_voice(&model));
            assert!(
                restore_companion_voice(&id).is_err(),
                "备份已删不可重复恢复"
            );

            // 手动删生效 wav 后恢复（若有备份）可修复——再造一次覆盖后手动删。
            upload_companion_voice(&id, &up, "再覆盖").unwrap();
            std::fs::remove_file(&paths.wav).unwrap();
            restore_companion_voice(&id).unwrap();
            assert_eq!(std::fs::read(&paths.wav).unwrap(), author_wav);
        });
    }

    #[test]
    fn test_upload_voice_invalidates_welcome_fingerprint() {
        run_with_temp_home(|home| {
            let id = import_pack_with_voice(home, "芙宁娜");
            crate::companion::set_active(&id).unwrap(); // 当前生效音色（角色自带）参与指纹。
            let fingerprint = || {
                let mut cfg = crate::voice::config::resolve(
                    None,
                    &crate::voice::config::CliOverrides::default(),
                )
                .unwrap();
                crate::voice::config::apply_companion_overrides(&mut cfg);
                crate::companion_welcome::clip_fingerprint(&cfg)
            };
            let fp_before = fingerprint();
            let up = home.join("custom.wav");
            // 帧数与原版不同（len 必变）：mtime 秒级精度下同秒替换也可能撞指纹。
            make_wav(&up, 1, 16_000, 320);
            upload_companion_voice(&id, &up, "新转写").unwrap();
            let fp_after = fingerprint();
            assert_ne!(
                fp_before, fp_after,
                "音色改写必须使欢迎语指纹失效（自动重生成链）"
            );
        });
    }
}
