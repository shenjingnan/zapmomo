//! ZapMomo 桌面应用（Tauri 2）。
//!
//! 复用根 crate `zapmomo` 的 KWS / 音频 / 配置逻辑：
//! - 通过 Tauri command 暴露设备列表、KWS 配置、开始/停止监听；
//! - 监听循环跑在独立 `std::thread`，检测到唤醒词经 `TauriReaction`
//!   以 `kws-detected` 事件推给前端；结束（正常/出错/手动停止）发 `kws-stopped`。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use tauri::TitleBarStyle;
#[cfg(target_os = "macos")]
use tauri::menu::PredefinedMenuItem;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, Manager, PhysicalPosition, PhysicalSize, State,
    WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_global_shortcut::GlobalShortcutExt;
use zapmomo::asr::config::AsrParamsPatch;
use zapmomo::asr::{AsrReaction, AsrResult};
use zapmomo::companion_click_through::{
    CompanionPointerPolicy, EXIT_MARGIN_PX, HitRect, cursor_hit, desired_ignore_cursor_events,
    next_hold, resolve_smart_click_through,
};
use zapmomo::config::settings::{
    self, AsrSettings, BubbleSettings, ChatboxSettings, CompanionDragMode, CompanionWindowLayer,
    CompanionWindowPosition, KwsSettings, Live2dSettings, LlmSettings, TtsSettings,
};
use zapmomo::datetime::iso_timestamp_now;
use zapmomo::kws::{KwsResult, Reaction, ReactionOutcome};
use zapmomo::llm::types::{ChatMessage, ChatRole, GenParams, InputItem, LlmParamsPatch};
use zapmomo::llm::{LlmEngine, LlmEvent};
use zapmomo::model_library;
use zapmomo::model_library::{
    InstallState as LibInstallState, LibraryModel, RuntimeAction as LibRuntimeAction,
    SetCurrentResult, SystemResources, registry::ModelType as LibModelType,
    storage::StorageInfoView,
};
use zapmomo::performance::{
    DeviceEvent, MouseSimulator, PerformanceScene, PerformanceSource, Rect, Rng, StopSignal,
    TypingSimulator, run_source,
};
use zapmomo::tts::config::TtsParamsPatch;
use zapmomo::voice::VoiceSession;
use zapmomo::voice::config::CliOverrides as VoiceCliOverrides;
use zapmomo::voice::events::VoiceEvent;
use zapmomo::voice::records;
use zapmomo::voice::state::SessionState as VoicePhase;

// 角色窗口的 macOS 非激活面板：点击/拖动不激活应用、不抢前台焦点，
// 使其表现为纯桌面摆件（参考 BongoCat 的 `tauri-nspanel` 方案）。
#[cfg(target_os = "macos")]
tauri_nspanel::tauri_panel! {
    panel!(CompanionPanel {
        config: {
            is_floating_panel: true,
            // 摆件无需键盘输入，彻底不抢焦点。
            can_become_key_window: false,
            // 关键：永不成为 main window，点击不会把焦点从上一个窗口抢过来。
            can_become_main_window: false,
        }
    })
}

// 文字输入条的 macOS 非激活面板：与角色窗口同为 nonactivating panel，聚焦输入框
// 不需要激活整个 App（Spotlight 式），因此「显隐快捷键呼出输入条并聚焦」不会把
// 本应用其它可见窗口（如设置窗）一并带到最前。可以成为 key window 以接收键盘与
// 中文 IME 输入，但永不成为 main window。
// 宏展开含 use 声明，须包在独立模块内避免与上方 CompanionPanel 冲突。
#[cfg(target_os = "macos")]
mod chatbox_panel {
    // 宏生成代码需调用 WebviewWindow::app_handle()（Manager trait 方法）
    use tauri::Manager as _;

    tauri_nspanel::tauri_panel! {
        panel!(ChatboxPanel {
            config: {
                is_floating_panel: true,
                can_become_key_window: true,
                can_become_main_window: false,
            }
        })
    }
}
#[cfg(target_os = "macos")]
use chatbox_panel::ChatboxPanel;

// 语音回复气泡窗口的 macOS 非激活面板：纯展示 + 拖动，无需键盘输入，
// 与角色窗口一样彻底不抢焦点（can_become_key_window: false）。
// 宏展开含 use 声明，须包在独立模块内避免与上方 panel 冲突。
#[cfg(target_os = "macos")]
mod bubble_panel {
    // 宏生成代码需调用 WebviewWindow::app_handle()（Manager trait 方法）
    use tauri::Manager as _;

    tauri_nspanel::tauri_panel! {
        panel!(BubblePanel {
            config: {
                is_floating_panel: true,
                can_become_key_window: false,
                can_become_main_window: false,
            }
        })
    }
}
#[cfg(target_os = "macos")]
use bubble_panel::BubblePanel;

/// 监听线程状态：共享停止标志 + 线程句柄 + 运行时实际模型目录（RuntimeActual）。
struct ListenState {
    running: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 当前会话真正使用的模型目录（启动监听时固化；停止/线程退出时清空）
    active_model_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl ListenState {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            active_model_dir: Arc::new(Mutex::new(None)),
        }
    }

    fn is_listening(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn active_model_dir(&self) -> Option<PathBuf> {
        self.active_model_dir.lock().ok().and_then(|g| g.clone())
    }
}

/// RAII：进入监听时置 `active_model_dir`，无论正常/错误/panic 退出监听线程都会清空。
struct ActiveModelGuard {
    target: Arc<Mutex<Option<PathBuf>>>,
}

impl ActiveModelGuard {
    fn set(target: &Arc<Mutex<Option<PathBuf>>>, path: PathBuf) -> Self {
        *target.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
        Self {
            target: target.clone(),
        }
    }
}

impl Drop for ActiveModelGuard {
    fn drop(&mut self) {
        *self.target.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// 语音会话线程状态：共享停止标志 + 线程句柄（仿 `ListenState`）。
struct VoiceSessionState {
    running: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 当前会话的打断标志：会话线程创建 session 后写入，线程退出时清空。
    /// 全局快捷键「打断播报」置位 → 会话循环 `do_barge_in`（停生成/合成/播放，回 Armed）。
    barge_in: Mutex<Option<Arc<AtomicBool>>>,
    /// 当前会话的 TTS 热切换邮箱：会话线程创建前写入，线程退出时清空（与 barge_in
    /// 同款登记模式）。`set_current_model` TTS 事务臂把新引擎塞入邮箱，会话每轮
    /// 循环开头取走并句间换入合成线程（`zapmomo::voice::TtsSwap`）。
    tts_swap: Mutex<Option<zapmomo::voice::TtsSwapSlot>>,
    /// 文字输入通道（输入条窗口）：会话线程启动时写入，退出时清空。
    /// `send_voice_text` 命令经此把用户打字内容送进会话编排循环。
    text_tx: Mutex<Option<std::sync::mpsc::Sender<String>>>,
}

impl VoiceSessionState {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            barge_in: Mutex::new(None),
            tts_swap: Mutex::new(None),
            text_tx: Mutex::new(None),
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

// ---- 语音会话事件载荷（emit 给前端）----

#[derive(Clone, Serialize)]
struct VoiceSessionStatePayload {
    running: bool,
    state: VoicePhase,
}

#[derive(Clone, Serialize)]
struct VoiceWakePayload {
    keyword: String,
}

#[derive(Clone, Serialize)]
struct VoiceTranscriptPayload {
    text: String,
    is_final: bool,
}

#[derive(Clone, Serialize)]
struct VoiceTokenPayload {
    delta: String,
}

#[derive(Clone, Serialize)]
struct VoiceReplyPayload {
    sentence: String,
}

#[derive(Clone, Serialize)]
struct VoicePlayPayload {
    sentence: String,
}

#[derive(Clone, Serialize)]
struct VoiceReplyFinishedPayload {
    reason: String,
    /// 该轮完整可见回复（`None` = 空回复），供前端提交对话记录
    text: Option<String>,
}

#[derive(Clone, Serialize)]
struct VoiceErrorPayload {
    message: String,
}

#[derive(Clone, Serialize)]
struct VoiceStoppedPayload {
    error: Option<String>,
}

/// 模型下载状态：防重入标志。
struct DownloadState {
    in_progress: Arc<AtomicBool>,
}

impl Default for DownloadState {
    fn default() -> Self {
        Self {
            in_progress: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// 下载进度事件载荷（推给前端）。
#[derive(Clone, Serialize)]
struct DownloadProgressPayload {
    stage: String,
    percent: f64,
    message: String,
}

/// 退出作用域（含 panic / 命令取消）时复位下载标志。
struct ResetOnDrop(Arc<AtomicBool>);

impl Drop for ResetOnDrop {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 把唤醒词检测结果通过 Tauri 事件发给前端。
struct TauriReaction {
    app: AppHandle,
}

impl Reaction for TauriReaction {
    fn on_keyword(&mut self, result: &KwsResult) -> ReactionOutcome {
        let _ = self.app.emit("kws-detected", result);
        ReactionOutcome::Continue
    }
}

/// 监听结束事件载荷（正常停止时 `error` 为 `None`）。
#[derive(Clone, Serialize)]
struct ListenStopped {
    error: Option<String>,
}

#[derive(Serialize)]
struct AppInfo {
    version: String,
    product_name: String,
}

#[tauri::command]
fn get_app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        product_name: env!("CARGO_PKG_NAME").to_string(),
    }
}

/// 列出可用麦克风输入设备。
#[tauri::command]
fn list_devices() -> Vec<String> {
    zapmomo::audio::list_input_devices()
}

/// 请求 macOS 麦克风授权（触发系统授权弹窗）。返回是否已授权。
///
/// macOS 未授权时输入设备被系统隐藏、枚举为空，需先经此授权恢复；
/// 调试模式下每次重新编译授权会失效，前端在设备列表为空时引导用户点击。
#[tauri::command]
fn request_mic_permission() -> Result<bool, String> {
    zapmomo::audio::request_mic_permission()
}

/// GUI 展示用的 KWS 配置信息。
#[derive(Serialize)]
struct KwsConfigInfo {
    enabled: bool,
    custom_keywords: String,
    model_dir: String,
    provider: String,
    num_threads: i32,
    sample_rate: i32,
    chunk_size: usize,
    keywords_score: f32,
    keywords_threshold: f32,
    debug: bool,
    keywords: Vec<String>,
    models_present: bool,
    model_downloading: bool,
    settings_path: String,
}

/// `set_kws_params` 载荷：可调整的 KWS 引擎/运行参数（snake_case 直传，缺省项不修改）。
#[derive(Debug, Clone, Default, Deserialize)]
struct KwsParamsPatch {
    keywords_threshold: Option<f32>,
    keywords_score: Option<f32>,
    chunk_size: Option<usize>,
    num_threads: Option<i32>,
    debug: Option<bool>,
}

impl KwsParamsPatch {
    /// 先整体校验（任一越界立即 Err），再逐项写入 `KwsSettings`，保证出错时不部分修改。
    fn apply_to(&self, kws: &mut KwsSettings) -> Result<(), String> {
        if let Some(v) = self.keywords_threshold
            && !(0.0..=1.0).contains(&v)
        {
            return Err(format!("灵敏度/阈值需在 0~1，当前 {v}"));
        }
        if let Some(v) = self.keywords_score
            && !(0.1..=10.0).contains(&v)
        {
            return Err(format!("关键词加权需在 0.1~10，当前 {v}"));
        }
        if let Some(v) = self.chunk_size
            && !(400..=16_000).contains(&v)
        {
            return Err(format!("采样块大小需在 400~16000（@16k），当前 {v}"));
        }
        if let Some(v) = self.num_threads
            && !(1..=32).contains(&v)
        {
            return Err(format!("线程数需在 1~32，当前 {v}"));
        }

        if let Some(v) = self.keywords_threshold {
            kws.keywords_threshold = Some(v);
        }
        if let Some(v) = self.keywords_score {
            kws.keywords_score = Some(v);
        }
        if let Some(v) = self.chunk_size {
            kws.chunk_size = Some(v);
        }
        if let Some(v) = self.num_threads {
            kws.num_threads = Some(v);
        }
        if let Some(v) = self.debug {
            kws.debug = Some(v);
        }
        Ok(())
    }
}

/// 读取合并后的 KWS 配置（settings.toml + 默认值），并给出模型是否就绪。
#[tauri::command]
fn get_kws_config(state: State<'_, DownloadState>) -> Result<KwsConfigInfo, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let kws_settings = settings.as_ref().and_then(|s| s.kws.clone());
    let cfg = zapmomo::kws::config::resolve(kws_settings.as_ref(), None)?;

    let files = [
        &cfg.encoder,
        &cfg.decoder,
        &cfg.joiner,
        &cfg.tokens,
        &cfg.keywords_file,
    ];
    let models_present = files.iter().all(|p| p.is_file());
    let keywords =
        zapmomo::kws::config::parse_keywords_file(&cfg.keywords_file).unwrap_or_default();
    tracing::info!(
        "get_kws_config: settings.kws.enabled={:?} resolve.enabled={} models_present={} settings_path={}",
        kws_settings.as_ref().and_then(|k| k.enabled),
        cfg.enabled,
        models_present,
        zapmomo::config::settings::get_settings_path().display()
    );

    Ok(KwsConfigInfo {
        enabled: cfg.enabled,
        custom_keywords: kws_settings
            .as_ref()
            .and_then(|s| s.custom_keywords.clone())
            .unwrap_or_default(),
        model_dir: cfg.model_dir.display().to_string(),
        provider: cfg.provider.clone(),
        num_threads: cfg.num_threads,
        sample_rate: cfg.sample_rate,
        chunk_size: cfg.chunk_size,
        keywords_score: cfg.keywords_score,
        keywords_threshold: cfg.keywords_threshold,
        debug: cfg.debug,
        keywords,
        models_present,
        model_downloading: state.in_progress.load(Ordering::Relaxed),
        settings_path: zapmomo::config::settings::get_settings_path()
            .display()
            .to_string(),
    })
}

/// 开始实时监听唤醒词（command 与启动自动监听共用）。
///
/// 校验模型文件后启动独立线程跑 `run_realtime_with`，检测结果经
/// `kws-detected` 事件发给前端；线程结束发 `kws-stopped`。
fn start_listen_impl(
    app: AppHandle,
    state: &ListenState,
    device: Option<String>,
    keywords: Option<String>,
) -> Result<(), String> {
    if state.is_listening() {
        return Err("已在监听中".to_string());
    }

    let settings = zapmomo::config::settings::load_settings()?;
    let kws_settings = settings.as_ref().and_then(|s| s.kws.clone());
    let cfg = zapmomo::kws::config::resolve(kws_settings.as_ref(), None)?;

    // 同步校验/编码附加关键词（原始中文自动转 ppinyin），避免编码失败时空指针崩溃
    if let Some(k) = keywords.as_deref() {
        zapmomo::kws::token::encode_custom_keywords(k, &cfg.tokens)?;
    }

    // 预检模型文件，失败同步返回清晰错误（避免在后台线程里才报错）
    let files = [
        &cfg.encoder,
        &cfg.decoder,
        &cfg.joiner,
        &cfg.tokens,
        &cfg.keywords_file,
    ];
    if let Some(missing) = files.iter().find(|p| !p.is_file()) {
        return Err(format!(
            "缺少模型文件: {}\n\n请在「配置」面板点击「下载模型」，或运行 `zapmomo kws install-model` 下载模型。",
            missing.display()
        ));
    }

    let running = state.running.clone();
    running.store(true, Ordering::Relaxed);
    // RuntimeActual：记录本次会话使用的模型目录；随线程退出（RAII）自动清空
    let _active_guard = ActiveModelGuard::set(&state.active_model_dir, cfg.model_dir.clone());
    let thread_app = app.clone();
    let handle = std::thread::spawn(move || {
        let _active = _active_guard;
        tracing::info!("KWS listen thread started");
        let mut reaction = TauriReaction { app: thread_app };
        let result = zapmomo::kws::run_realtime_with(
            &cfg,
            device.as_deref(),
            None,
            keywords.as_deref(),
            &mut reaction,
            Some(&running),
        );
        running.store(false, Ordering::Relaxed);
        match &result {
            Ok(()) => tracing::info!("KWS listen thread finished (clean)"),
            Err(e) => tracing::error!("KWS listen thread finished with error: {e}"),
        }
        let payload = ListenStopped {
            error: result.err(),
        };
        let _ = reaction.app.emit("kws-stopped", payload);
    });
    *state.handle.lock().expect("listen handle lock poisoned") = Some(handle);
    // 通知前端监听已启动（含切换设备后的自动重启；启动瞬间前端未订阅时静默丢弃）
    let _ = app.emit("kws-started", ListenStopped { error: None });
    Ok(())
}

/// 开始实时监听唤醒词。 —— Tauri command 外壳，签名与前端契约不变。
#[tauri::command]
fn start_listen(
    app: AppHandle,
    state: State<'_, ListenState>,
    device: Option<String>,
    keywords: Option<String>,
) -> Result<(), String> {
    start_listen_impl(app, state.inner(), device, keywords)
}

/// 停止实时监听的内部实现（`stop_listen` command 与「切换设备重启」共用）。
fn stop_listen_inner(state: &ListenState) -> Result<(), String> {
    if !state.is_listening() {
        return Err("当前没有在监听".to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    let handle = state
        .handle
        .lock()
        .expect("listen handle lock poisoned")
        .take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    // RAII guard 在线程退出时已清空；这里兜底确保一致
    *state
        .active_model_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// 停止实时监听：置停止标志并等待线程退出。
#[tauri::command]
fn stop_listen(state: State<'_, ListenState>) -> Result<(), String> {
    tracing::warn!("stop_listen called (is_listening={})", state.is_listening());
    stop_listen_inner(state.inner())
}

/// 当前是否正在监听。
#[tauri::command]
fn is_listening(state: State<'_, ListenState>) -> bool {
    state.is_listening()
}

/// 下载并安装 KWS 模型（默认安装到 `~/.zapmomo/models/<模型名>`）。
///
/// 防重入；下载在阻塞线程池执行，进度经 `kws-model-download-progress` 事件推给前端。
#[tauri::command]
async fn download_kws_model(app: AppHandle, state: State<'_, DownloadState>) -> Result<(), String> {
    let flag = state.in_progress.clone();
    if flag.swap(true, Ordering::SeqCst) {
        return Err("模型下载已在进行中，请稍候".to_string());
    }
    let dest = zapmomo::kws::model::user_model_dir();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = ResetOnDrop(flag);
        let mut progress = |p: zapmomo::kws::model::DownloadProgress| {
            let stage = match p.stage {
                zapmomo::kws::model::DownloadStage::Downloading => "downloading",
                zapmomo::kws::model::DownloadStage::Verifying => "verifying",
                zapmomo::kws::model::DownloadStage::Extracting => "extracting",
                zapmomo::kws::model::DownloadStage::Done => "done",
            };
            let _ = app.emit(
                "kws-model-download-progress",
                DownloadProgressPayload {
                    stage: stage.to_string(),
                    percent: p.percent,
                    message: p.message,
                },
            );
        };
        zapmomo::kws::model::install_model_to(&dest, false, &mut progress)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("下载任务异常: {e}"))?
}

/// ASR 监听线程状态：共享停止标志 + 线程句柄。
struct AsrListenState {
    running: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 当前会话真正使用的模型目录（RuntimeActual）
    active_model_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl AsrListenState {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            active_model_dir: Arc::new(Mutex::new(None)),
        }
    }

    fn is_listening(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn active_model_dir(&self) -> Option<PathBuf> {
        self.active_model_dir.lock().ok().and_then(|g| g.clone())
    }
}

/// ASR 模型下载状态：防重入标志。
struct AsrDownloadState {
    in_progress: Arc<AtomicBool>,
}

impl Default for AsrDownloadState {
    fn default() -> Self {
        Self {
            in_progress: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// 把语音识别结果通过 Tauri 事件发给前端。
struct TauriAsrReaction {
    app: AppHandle,
}

impl AsrReaction for TauriAsrReaction {
    fn on_result(&mut self, result: &AsrResult) -> ReactionOutcome {
        let _ = self.app.emit("asr-result", result);
        ReactionOutcome::Continue
    }
}

/// 离线听写线程状态：共享停止标志 + 线程句柄（镜像 `AsrListenState`）。
struct AsrDictateState {
    running: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 当前会话真正使用的模型目录（RuntimeActual）
    active_model_dir: Arc<Mutex<Option<PathBuf>>>,
}

impl AsrDictateState {
    fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
            active_model_dir: Arc::new(Mutex::new(None)),
        }
    }

    fn is_dictating(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn active_model_dir(&self) -> Option<PathBuf> {
        self.active_model_dir.lock().ok().and_then(|g| g.clone())
    }
}

/// 把听写结果（每段整句）通过 Tauri 事件发给前端。
struct TauriAsrDictateReaction {
    app: AppHandle,
}

impl AsrReaction for TauriAsrDictateReaction {
    fn on_result(&mut self, result: &AsrResult) -> ReactionOutcome {
        let _ = self.app.emit("asr-dictate-result", result);
        ReactionOutcome::Continue
    }
}

/// GUI 展示用的 ASR 配置信息（含可经 `set_asr_params` 调整的引擎参数）。
#[derive(Serialize)]
struct AsrConfigInfo {
    enabled: bool,
    /// 模型类型（zipformer/paraformer/sensevoice/whisper），前端据此隐藏流式专属参数
    model_type: String,
    /// 推理后端（sherpa/audiocpp），前端据此显示 audio.cpp 标识与隐藏热词参数
    backend: String,
    model_dir: String,
    provider: String,
    num_threads: i32,
    sample_rate: i32,
    chunk_size: usize,
    decoding_method: String,
    enable_endpoint: bool,
    rule1_min_trailing_silence: f32,
    rule2_min_trailing_silence: f32,
    rule3_min_utterance_length: f32,
    blank_penalty: f32,
    hotwords: Option<String>,
    enable_punctuation: bool,
    debug: bool,
    models_present: bool,
    punctuation_present: bool,
    /// Silero VAD 模型是否已就绪（离线听写首次启动会自动下载）
    vad_present: bool,
    model_downloading: bool,
    settings_path: String,
}

/// 读取合并后的 ASR 配置（settings.toml + 默认值），并给出模型是否就绪。
#[tauri::command]
fn get_asr_config(state: State<'_, AsrDownloadState>) -> Result<AsrConfigInfo, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let asr_settings = settings.as_ref().and_then(|s| s.asr.clone());
    let cfg = zapmomo::asr::config::resolve(asr_settings.as_ref(), None)?;

    // 族 + 后端感知：sherpa 按模型类型清单探测；audiocpp 按族表 GGUF 单文件探测
    let models_present = zapmomo::asr::config::models_present(&cfg);
    let punctuation_present = cfg.punctuation_model.is_file();
    tracing::info!(
        "get_asr_config: model_type={} backend={} settings.asr.enabled={:?} resolve.enabled={} models_present={}",
        cfg.model_type.as_str(),
        cfg.backend.as_str(),
        asr_settings.as_ref().and_then(|a| a.enabled),
        cfg.enabled,
        models_present
    );

    Ok(AsrConfigInfo {
        enabled: cfg.enabled,
        model_type: cfg.model_type.as_str().to_string(),
        backend: cfg.backend.as_str().to_string(),
        model_dir: cfg.model_dir.display().to_string(),
        provider: cfg.provider.clone(),
        num_threads: cfg.num_threads,
        sample_rate: cfg.sample_rate,
        chunk_size: cfg.chunk_size,
        decoding_method: cfg.decoding_method.clone(),
        enable_endpoint: cfg.enable_endpoint,
        rule1_min_trailing_silence: cfg.rule1_min_trailing_silence,
        rule2_min_trailing_silence: cfg.rule2_min_trailing_silence,
        rule3_min_utterance_length: cfg.rule3_min_utterance_length,
        blank_penalty: cfg.blank_penalty,
        hotwords: cfg.hotwords.clone(),
        enable_punctuation: cfg.enable_punctuation,
        debug: cfg.debug,
        models_present,
        punctuation_present,
        vad_present: zapmomo::asr::dictate::vad_model_present(),
        model_downloading: state.in_progress.load(Ordering::Relaxed),
        settings_path: zapmomo::config::settings::get_settings_path()
            .display()
            .to_string(),
    })
}

/// 一键离线转写的结果（snake_case 直传前端，与 AsrConfigInfo 同款）。
#[derive(Serialize)]
struct TranscribeResult {
    text: String,
    model_type: String,
    model_dir: String,
}

/// 一键离线转写 wav 文件（流式 zipformer / SenseVoice / Whisper 均可用）。
///
/// 族感知：`asr::transcribe_wav` 按 `model_type` 分发到在线/离线引擎；
/// `wav_path` 为 None 时转写模型自带的 `test_wavs/` 示例音频（离线「测试识别」）；
/// 阻塞线程池执行避免卡 UI。
#[tauri::command]
async fn transcribe_audio(wav_path: Option<String>) -> Result<TranscribeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = zapmomo::config::settings::load_settings()?;
        let asr_settings = settings.as_ref().and_then(|s| s.asr.clone());
        let cfg = zapmomo::asr::config::resolve(asr_settings.as_ref(), None)?;
        let wav_path = wav_path
            .map(std::path::PathBuf::from)
            .or_else(|| zapmomo::asr::default_test_wav(&cfg.model_dir))
            .ok_or_else(|| "未指定音频路径，且模型目录没有 test_wavs/*.wav 示例音频".to_string())?;
        let text = zapmomo::asr::transcribe_wav(&cfg, &wav_path)?;
        Ok(TranscribeResult {
            text,
            model_type: cfg.model_type.as_str().to_string(),
            model_dir: cfg.model_dir.display().to_string(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 开始实时语音识别的内部实现（`start_asr_listen` command 与「切换设备重启」共用）。
///
/// 校验模型文件后启动独立线程跑 `run_realtime_with`，识别结果经
/// `asr-result` 事件发给前端；线程结束发 `asr-stopped`，启动成功发 `asr-started`。
fn start_asr_listen_impl(
    app: AppHandle,
    state: &AsrListenState,
    device: Option<String>,
) -> Result<(), String> {
    if state.is_listening() {
        return Err("已在识别中".to_string());
    }

    let settings = zapmomo::config::settings::load_settings()?;
    let asr_settings = settings.as_ref().and_then(|s| s.asr.clone());
    let cfg = zapmomo::asr::config::resolve(asr_settings.as_ref(), None)?;

    // 离线族（SenseVoice/Whisper/Qwen3-ASR）不支持实时识别：前端已禁用开关，这里双保险拦截
    if !cfg.model_type.is_streaming() {
        return Err(format!(
            "当前模型类型 {} 不支持实时识别。请切换回流式（zipformer/paraformer）模型，或使用「转写文件」/「免提听写」离线转写。",
            cfg.model_type.as_str()
        ));
    }

    // 预检模型文件（族感知：zipformer 四件 / paraformer 三件），
    // 失败同步返回清晰错误（避免在后台线程里才报错）
    let preflight = collect_asr_preflight_files(&cfg)?;
    if let Some((_, missing)) = preflight.iter().find(|(_, p)| !p.is_file()) {
        return Err(format!(
            "缺少模型文件: {}\n\n请在「配置」面板点击「下载模型」，或运行 `zapmomo asr install-model` 下载模型。",
            missing.display()
        ));
    }

    let running = state.running.clone();
    running.store(true, Ordering::Relaxed);
    // RuntimeActual：记录本次识别会话使用的模型目录；随线程退出自动清空
    let _active_guard = ActiveModelGuard::set(&state.active_model_dir, cfg.model_dir.clone());
    let thread_app = app.clone();
    let handle = std::thread::spawn(move || {
        let _active = _active_guard;
        tracing::info!("ASR listen thread started");
        let mut reaction = TauriAsrReaction { app: thread_app };
        let result = zapmomo::asr::run_realtime_with(
            &cfg,
            device.as_deref(),
            None,
            &mut reaction,
            Some(&running),
        );
        running.store(false, Ordering::Relaxed);
        match &result {
            Ok(()) => tracing::info!("ASR listen thread finished (clean)"),
            Err(e) => tracing::error!("ASR listen thread finished with error: {e}"),
        }
        let payload = ListenStopped {
            error: result.err(),
        };
        let _ = reaction.app.emit("asr-stopped", payload);
    });
    *state
        .handle
        .lock()
        .expect("asr listen handle lock poisoned") = Some(handle);
    // 通知前端识别已启动（含切换设备后的自动重启；启动瞬间前端未订阅时静默丢弃）
    let _ = app.emit("asr-started", ListenStopped { error: None });
    Ok(())
}

/// 开始实时语音识别。 —— Tauri command 外壳，签名与前端契约不变。
#[tauri::command]
fn start_asr_listen(
    app: AppHandle,
    state: State<'_, AsrListenState>,
    device: Option<String>,
) -> Result<(), String> {
    start_asr_listen_impl(app, state.inner(), device)
}

/// 停止实时语音识别的内部实现（command 与「切换设备重启」共用）。
fn stop_asr_listen_inner(state: &AsrListenState) -> Result<(), String> {
    if !state.is_listening() {
        return Err("当前没有在识别".to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    let handle = state
        .handle
        .lock()
        .expect("asr listen handle lock poisoned")
        .take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    *state
        .active_model_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// 停止实时语音识别：置停止标志并等待线程退出。
#[tauri::command]
fn stop_asr_listen(state: State<'_, AsrListenState>) -> Result<(), String> {
    stop_asr_listen_inner(state.inner())
}

/// 当前是否正在识别。
#[tauri::command]
fn is_asr_listening(state: State<'_, AsrListenState>) -> bool {
    state.is_listening()
}

/// 开始离线免提听写的内部实现（command 与「切换设备重启」共用）。
///
/// 守卫：仅在离线模型（SenseVoice/Whisper/Qwen3-ASR）下可用；流式族（zipformer/paraformer）被拒（听写是离线专用）。
/// 线程内先惰性下载 Silero VAD 模型，再跑 `run_dictate`（VAD 分段 → 每段整句转写）。
fn start_asr_dictate_impl(
    app: AppHandle,
    state: &AsrDictateState,
    device: Option<String>,
) -> Result<(), String> {
    if state.is_dictating() {
        return Err("已在听写中".to_string());
    }

    let settings = zapmomo::config::settings::load_settings()?;
    let asr_settings = settings.as_ref().and_then(|s| s.asr.clone());
    let cfg = zapmomo::asr::config::resolve(asr_settings.as_ref(), None)?;

    // 流式模型不支持听写（离线模型专用）：前端已切走开关，这里双保险拦截
    if cfg.model_type.is_streaming() {
        return Err(format!(
            "当前模型类型 {} 不支持免提听写（离线模型专用）。请先切换 SenseVoice/Whisper/Qwen3-ASR 离线模型。",
            cfg.model_type.as_str()
        ));
    }

    // 离线模型文件预检（族感知），避免在后台线程里才报错
    if !zapmomo::asr::config::asr_files_present_for_kind(&cfg.model_dir, cfg.model_type) {
        return Err("离线模型文件不完整，请在「切换模型」中重新下载或选择完整模型。".to_string());
    }

    let running = state.running.clone();
    running.store(true, Ordering::Relaxed);
    // RuntimeActual：记录本次听写使用的模型目录；随线程退出自动清空
    let _active_guard = ActiveModelGuard::set(&state.active_model_dir, cfg.model_dir.clone());
    let thread_app = app.clone();
    let handle = std::thread::spawn(move || {
        let _active = _active_guard;
        tracing::info!("ASR dictate thread started");
        let mut reaction = TauriAsrDictateReaction {
            app: thread_app.clone(),
        };

        // 首次听写自动下载 Silero VAD 模型（~0.6MB，幂等）；失败则停止并报错
        let result = {
            let mut progress = |p: zapmomo::kws::model::DownloadProgress| {
                let stage = match p.stage {
                    zapmomo::kws::model::DownloadStage::Downloading => "downloading",
                    zapmomo::kws::model::DownloadStage::Verifying => "verifying",
                    zapmomo::kws::model::DownloadStage::Extracting => "extracting",
                    zapmomo::kws::model::DownloadStage::Done => "done",
                };
                let _ = thread_app.emit(
                    "asr-vad-download-progress",
                    DownloadProgressPayload {
                        stage: stage.to_string(),
                        percent: p.percent,
                        message: p.message,
                    },
                );
            };
            match zapmomo::asr::dictate::ensure_vad_model(&mut progress) {
                Ok(vad_path) => {
                    let vad_cfg =
                        zapmomo::asr::dictate::DictateConfig::new(vad_path).with_runtime(&cfg);
                    zapmomo::asr::dictate::run_dictate(
                        &cfg,
                        &vad_cfg,
                        device.as_deref(),
                        None,
                        &mut reaction,
                        Some(&running),
                    )
                }
                Err(e) => Err(e),
            }
        };

        running.store(false, Ordering::Relaxed);
        match &result {
            Ok(()) => tracing::info!("ASR dictate thread finished (clean)"),
            Err(e) => tracing::error!("ASR dictate thread finished with error: {e}"),
        }
        let payload = ListenStopped {
            error: result.err(),
        };
        let _ = reaction.app.emit("asr-dictate-stopped", payload);
    });
    *state
        .handle
        .lock()
        .expect("asr dictate handle lock poisoned") = Some(handle);
    // 通知前端听写已启动（含切换设备后的自动重启；启动瞬间前端未订阅时静默丢弃）
    let _ = app.emit("asr-dictate-started", ListenStopped { error: None });
    Ok(())
}

/// 开始离线免提听写。 —— Tauri command 外壳。
#[tauri::command]
fn start_asr_dictate(
    app: AppHandle,
    state: State<'_, AsrDictateState>,
    device: Option<String>,
) -> Result<(), String> {
    start_asr_dictate_impl(app, state.inner(), device)
}

/// 停止离线听写的内部实现（command 与「切换设备重启」共用）。
fn stop_asr_dictate_inner(state: &AsrDictateState) -> Result<(), String> {
    if !state.is_dictating() {
        return Err("当前没有在听写".to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    let handle = state
        .handle
        .lock()
        .expect("asr dictate handle lock poisoned")
        .take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    *state
        .active_model_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
}

/// 停止离线听写：置停止标志并等待线程退出。
#[tauri::command]
fn stop_asr_dictate(state: State<'_, AsrDictateState>) -> Result<(), String> {
    stop_asr_dictate_inner(state.inner())
}

/// 当前是否正在离线听写。
#[tauri::command]
fn is_asr_dictating(state: State<'_, AsrDictateState>) -> bool {
    state.is_dictating()
}

/// 下载并安装 ASR 模型（默认安装到 `~/.zapmomo/models/<模型名>`）。
///
/// 防重入；下载在阻塞线程池执行，进度经 `asr-model-download-progress` 事件推给前端。
#[tauri::command]
async fn download_asr_model(
    app: AppHandle,
    state: State<'_, AsrDownloadState>,
) -> Result<(), String> {
    let flag = state.in_progress.clone();
    if flag.swap(true, Ordering::SeqCst) {
        return Err("模型下载已在进行中，请稍候".to_string());
    }
    let dest = zapmomo::asr::user_model_dir();
    let punct_dest = zapmomo::asr::punctuation_user_model_dir();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = ResetOnDrop(flag);
        let mut progress = |p: zapmomo::asr::DownloadProgress| {
            let stage = match p.stage {
                zapmomo::asr::DownloadStage::Downloading => "downloading",
                zapmomo::asr::DownloadStage::Verifying => "verifying",
                zapmomo::asr::DownloadStage::Extracting => "extracting",
                zapmomo::asr::DownloadStage::Done => "done",
            };
            let _ = app.emit(
                "asr-model-download-progress",
                DownloadProgressPayload {
                    stage: stage.to_string(),
                    percent: p.percent,
                    message: p.message,
                },
            );
        };
        zapmomo::asr::install_model_to(&dest, false, &mut progress).map_err(|e| e.to_string())?;
        // 顺带安装标点模型（自动开启）；失败仅警告，不阻断 ASR 下载成功。
        if let Err(e) =
            zapmomo::asr::install_punctuation_model_to(&punct_dest, false, &mut progress)
        {
            tracing::warn!("标点模型安装失败（ASR 仍可用，仅无标点）: {e}");
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("下载任务异常: {e}"))?
}

/// TTS 合成线程状态：共享 busy 标志 + 线程句柄。
struct TtsSynthesizeState {
    busy: Arc<AtomicBool>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl TtsSynthesizeState {
    fn new() -> Self {
        Self {
            busy: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
        }
    }

    fn is_synthesizing(&self) -> bool {
        self.busy.load(Ordering::Relaxed)
    }
}

/// TTS 模型下载状态：防重入标志。
struct TtsDownloadState {
    in_progress: Arc<AtomicBool>,
}

impl Default for TtsDownloadState {
    fn default() -> Self {
        Self {
            in_progress: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// GUI 展示用的 TTS 配置信息。
#[derive(Serialize)]
struct TtsConfigInfo {
    /// 模型类型（zipvoice/omnivoice/...），前端据此切换音色语义
    model_type: String,
    /// 推理后端（sherpa/audiocpp），前端据此显示引擎徽标
    backend: String,
    model_dir: String,
    provider: String,
    num_threads: i32,
    enabled: bool,
    models_present: bool,
    model_downloading: bool,
    settings_path: String,
    /// 扩散解码步数（质量/速度权衡），可经 `set_tts_params` 修改
    num_steps: i32,
    /// 默认语速，可经 `set_tts_params` 修改
    speed: f32,
    /// 调试输出，可经 `set_tts_params` 修改
    debug: bool,
    /// 默认音色 id（`None` = 用内置 leijun），可经 `set_tts_voice` 修改
    voice: Option<String>,
}

/// 合成结果事件载荷（推给前端播放）。
#[derive(Clone, Serialize)]
struct TtsResult {
    path: String,
    duration: f32,
    sample_rate: i32,
}

/// 读取合并后的 TTS 配置（settings.toml + 默认值），并给出模型是否就绪。
#[tauri::command]
fn get_tts_config(state: State<'_, TtsDownloadState>) -> Result<TtsConfigInfo, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let tts_settings = settings.as_ref().and_then(|s| s.tts.clone());
    let cfg = zapmomo::tts::config::resolve(tts_settings.as_ref(), None)?;

    let models_present = zapmomo::tts::config::models_present(&cfg);

    Ok(TtsConfigInfo {
        model_type: cfg.model_type.as_str().to_string(),
        backend: cfg.backend.as_str().to_string(),
        model_dir: cfg.model_dir.display().to_string(),
        provider: cfg.provider.clone(),
        num_threads: cfg.num_threads,
        enabled: cfg.enabled,
        models_present,
        model_downloading: state.in_progress.load(Ordering::Relaxed),
        settings_path: zapmomo::config::settings::get_settings_path()
            .display()
            .to_string(),
        num_steps: cfg.num_steps,
        speed: cfg.speed,
        debug: cfg.debug,
        voice: cfg.voice.clone(),
    })
}

/// 在后台线程内合成文本，期间发 `tts-progress`，完成后发 `tts-result`。
fn synthesize_inner(
    app: &AppHandle,
    cfg: &zapmomo::tts::config::ResolvedTtsConfig,
    text: &str,
    speed: f32,
    voice: &zapmomo::tts::TtsVoiceParams,
) -> Result<(), String> {
    let engine = zapmomo::tts::TtsEngine::new(cfg.clone())?;
    let out_dir = zapmomo::config::settings::get_tts_output_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建输出目录失败: {e}"))?;
    // 放行 asset 协议 scope，前端 <audio> 才能通过 asset:// 播放生成的 wav。
    let _ = app.asset_protocol_scope().allow_directory(&out_dir, true);
    let out_path = zapmomo::tts::default_output_path();

    let progress_app = app.clone();
    let sample_count =
        engine.synthesize_to_wav_with_progress(text, speed, voice, &out_path, move |p| {
            let _ = progress_app.emit(
                "tts-progress",
                zapmomo::tts::reaction::TtsProgress { percent: p },
            );
            true
        })?;

    let sample_rate = engine.sample_rate();
    let duration = sample_count as f32 / sample_rate as f32;
    let _ = app.emit(
        "tts-result",
        TtsResult {
            path: out_path.display().to_string(),
            duration,
            sample_rate,
        },
    );
    Ok(())
}

/// 列出可用音色：克隆族返回参考音色（模型包内置 + 用户自定义音色库；
/// omnivoice/voxcpm2 无内置仅自定义库）；非克隆模型返回空列表。
#[tauri::command]
fn list_tts_voices() -> Result<Vec<zapmomo::tts::TtsVoice>, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let tts_settings = settings.as_ref().and_then(|s| s.tts.clone());
    let cfg = zapmomo::tts::config::resolve(tts_settings.as_ref(), None)?;
    if !cfg.model_type.uses_reference_audio() {
        return Ok(Vec::new());
    }
    let mut voices = zapmomo::tts::voice::list_builtin_voices(&cfg.model_dir);
    voices.extend(zapmomo::tts::voice_store::list_custom_voices());
    Ok(voices)
}

/// 保存一个自定义音色：把源 wav 拷贝到音色库并登记（命名 + 参考转写文本）。
#[tauri::command]
fn save_tts_voice(
    name: String,
    source_wav_path: String,
    reference_text: String,
) -> Result<zapmomo::tts::TtsVoice, String> {
    zapmomo::tts::voice_store::save_voice(
        &name,
        std::path::Path::new(&source_wav_path),
        &reference_text,
    )
}

/// 删除一个自定义音色（清单 + wav 文件）。
#[tauri::command]
fn delete_tts_voice(id: String) -> Result<(), String> {
    zapmomo::tts::voice_store::delete_voice(&id)
}

/// 录制 N 秒麦克风并保存为 16k wav，返回 wav 路径（供后续保存为音色）。
#[tauri::command]
async fn record_tts_voice(seconds: u32, device: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        zapmomo::audio::record_voice(seconds, device.as_deref()).map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| format!("录音任务异常: {e}"))?
}

/// 用 ASR 离线转写参考音频，返回带标点的转写文本（供自定义音色自动填充）。
///
/// 依赖 ASR 模型（含标点模型）已下载；转写在阻塞线程池执行，避免卡住 UI。
#[tauri::command]
async fn transcribe_reference_audio(wav_path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let settings = zapmomo::config::settings::load_settings()?;
        let asr_settings = settings.as_ref().and_then(|s| s.asr.clone());
        let cfg = zapmomo::asr::config::resolve(asr_settings.as_ref(), None)?;
        zapmomo::asr::transcribe_wav(&cfg, Path::new(&wav_path))
    })
    .await
    .map_err(|e| format!("转写任务异常: {e}"))?
}

/// 把文本合成为语音并写入 wav（后台线程执行）。
///
/// 校验模型文件后启动独立线程合成，进度经 `tts-progress` 事件推给前端；
/// 完成后发 `tts-result`（含 wav 路径），线程末发 `tts-stopped`。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn synthesize_tts(
    app: AppHandle,
    state: State<'_, TtsSynthesizeState>,
    text: String,
    speed: Option<f32>,
    sid: Option<i32>,
    voice: Option<String>,
    reference_wav: Option<String>,
    reference_text: Option<String>,
) -> Result<(), String> {
    if state.is_synthesizing() {
        return Err("正在合成中".to_string());
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("文本不能为空".to_string());
    }

    // 试听与真实语音会话同规则：markdown/emoji 清洗后再送引擎（CLI `tts run`
    // 不洗，保留引擎裸行为调试）。清洗为空报错而非静默回退——试听语义下用户
    // 应知道没有可朗读内容。注意 reference_text 不清洗（克隆音色转写须与 wav
    // 逐字对应）。
    let text = zapmomo::voice::sanitizer::sanitize_for_tts(&text);
    if text.is_empty() {
        return Err("清洗后没有可朗读内容（可能全是 Markdown 符号或 emoji）。".to_string());
    }

    let settings = zapmomo::config::settings::load_settings()?;
    let tts_settings = settings.as_ref().and_then(|s| s.tts.clone());
    let cfg = zapmomo::tts::config::resolve(tts_settings.as_ref(), None)?;

    // 启用门控：关闭时直接返回错误，前端据此禁用合成。
    if !cfg.enabled {
        return Err("语音合成已禁用，请在「模型与能力」中开启语音合成。".to_string());
    }

    // 预检模型文件（backend 感知：sherpa 按模型类型清单、audiocpp 按固定两文件），
    // 失败同步返回清晰错误（避免在后台线程里才报错）
    zapmomo::tts::config::preflight(&cfg).map_err(|e| {
        format!(
            "{e}\n\n请在「配置」面板点击「选择模型」，或运行 `zapmomo tts install-model` 下载模型。"
        )
    })?;

    // 合成音色参数统一解析（克隆 > sid > audiocpp 具名，见 zapmomo::tts::voice）。
    // 用户显式参数（音色/自定义参考音频）优先；都为空时回落 active 角色包的克隆音色
    // （设置页试听全局音色不应被角色包劫持）。在后台线程外解析，尽早报错。
    let character = (voice.is_none() && reference_wav.is_none())
        .then(zapmomo::companion::active_character_voice)
        .flatten();
    let custom_wav = reference_wav
        .map(std::path::PathBuf::from)
        .or_else(|| character.as_ref().map(|v| v.wav.clone()));
    let reference_text = reference_text.or_else(|| character.map(|v| v.text));
    let voice_params = zapmomo::tts::voice::resolve_voice_params(
        &cfg,
        voice.as_deref(),
        sid,
        custom_wav.as_deref(),
        reference_text.as_deref(),
    )?;

    let speed = speed.unwrap_or(cfg.speed);

    let busy = state.busy.clone();
    busy.store(true, Ordering::Relaxed);
    let thread_app = app.clone();
    let handle = std::thread::spawn(move || {
        tracing::info!("TTS synthesize thread started");
        let result = synthesize_inner(&thread_app, &cfg, &text, speed, &voice_params);
        busy.store(false, Ordering::Relaxed);
        match &result {
            Ok(()) => tracing::info!("TTS synthesize thread finished (clean)"),
            Err(e) => tracing::error!("TTS synthesize thread finished with error: {e}"),
        }
        let payload = ListenStopped {
            error: result.err(),
        };
        let _ = thread_app.emit("tts-stopped", payload);
    });
    *state.handle.lock().expect("tts handle lock poisoned") = Some(handle);
    Ok(())
}

/// 停止 TTS 合成/播放的内部实现（command 与全局快捷键打断共用）。
fn stop_tts_inner(state: &TtsSynthesizeState) -> Result<(), String> {
    if !state.is_synthesizing() {
        return Err("当前没有在合成".to_string());
    }
    state.busy.store(false, Ordering::Relaxed);
    let handle = state
        .handle
        .lock()
        .expect("tts handle lock poisoned")
        .take();
    if let Some(handle) = handle {
        let _ = handle.join();
    }
    Ok(())
}

/// 停止正在进行的合成（等待线程退出）。
#[tauri::command]
fn stop_tts(state: State<'_, TtsSynthesizeState>) -> Result<(), String> {
    stop_tts_inner(state.inner())
}

/// 当前是否正在合成。
#[tauri::command]
fn is_tts_synthesizing(state: State<'_, TtsSynthesizeState>) -> bool {
    state.is_synthesizing()
}

/// 下载并安装 TTS 模型（主包 + 声码器，默认 `~/.zapmomo/models/<模型名>`）。
///
/// 防重入；下载在阻塞线程池执行，进度经 `tts-model-download-progress` 事件推给前端。
#[tauri::command]
async fn download_tts_model(
    app: AppHandle,
    state: State<'_, TtsDownloadState>,
) -> Result<(), String> {
    let flag = state.in_progress.clone();
    if flag.swap(true, Ordering::SeqCst) {
        return Err("模型下载已在进行中，请稍候".to_string());
    }
    let dest = zapmomo::tts::user_model_dir();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = ResetOnDrop(flag);
        let mut progress = |p: zapmomo::tts::DownloadProgress| {
            let stage = match p.stage {
                zapmomo::tts::DownloadStage::Downloading => "downloading",
                zapmomo::tts::DownloadStage::Verifying => "verifying",
                zapmomo::tts::DownloadStage::Extracting => "extracting",
                zapmomo::tts::DownloadStage::Done => "done",
            };
            let _ = app.emit(
                "tts-model-download-progress",
                DownloadProgressPayload {
                    stage: stage.to_string(),
                    percent: p.percent,
                    message: p.message,
                },
            );
        };
        zapmomo::tts::install_model_to(&dest, false, &mut progress).map_err(|e| e.to_string())?;
        zapmomo::tts::install_vocoder_to(&dest, false, &mut progress).map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("下载任务异常: {e}"))?
}

/// LLM 引擎状态：懒创建的 worker 线程引擎。
struct LlmState {
    engine: Arc<Mutex<Option<Arc<LlmEngine>>>>,
}

impl LlmState {
    fn new() -> Self {
        Self {
            engine: Arc::new(Mutex::new(None)),
        }
    }
}

/// 共享 LLM 引擎是否正在生成（voice / GUI 任一在生成）。切换/卸载保护据此判断：
/// 仅当 LLM 真正在工作时才阻止，空闲（待唤醒）时允许切换（voice 会从共享槽感知新引擎）。
fn llm_engine_is_generating(llm: &LlmState) -> bool {
    llm.engine
        .lock()
        .ok()
        .and_then(|e| e.as_ref().map(|e| e.is_generating()))
        .unwrap_or(false)
}

/// GUI 展示用的 LLM 配置信息。
#[derive(Serialize)]
struct LlmConfigInfo {
    enabled: bool,
    provider: String,
    ready: bool,
    settings_path: String,
    system_prompt: String,
    params: GenParams,
    base_url: Option<String>,
    /// 完整 API Key（本机桌面应用，settings.toml 本身明文存储；
    /// 前端默认 password 圆点展示，用户点小眼睛才显式明文）。
    api_key: Option<String>,
    model: Option<String>,
    /// 是否启用思考（已 resolve 缺省推断；仅 anthropic provider 生效）
    thinking: bool,
    /// 思考力度（thinking 关闭时保留原值但运行时忽略）
    reasoning_effort: Option<String>,
}

/// 加载状态事件载荷。
#[derive(Clone, Serialize)]
struct LlmStatusPayload {
    ready: bool,
}

/// 读取合并后的 LLM 配置。
fn llm_resolved_config() -> Result<zapmomo::llm::config::ResolvedLlmConfig, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let llm_settings = settings.as_ref().and_then(|s| s.llm.clone());
    zapmomo::llm::config::resolve(llm_settings.as_ref())
}

/// 把 LLM 引擎事件转发为 Tauri 事件，直到 `Finished`/`Error`（`stop_on_status` 时 `Status` 也终止）。
/// 持续把 LLM 引擎事件转发为 Tauri 事件，直到 `Error` / 引擎被释放（`Disconnected`；
/// `stop_on_status` 时 `Status` 也终止）。**`Finished` 不退出**——同一引擎每次生成
/// （GUI 对话 / voice 会话）都会继续产生 Token/Finished，单个转发线程持续服务，
/// 否则第二次生成的事件无人转发，前端会一直停在「生成中」。
/// 只持事件流 `rx`，不持引擎 Arc——引擎被替换后（重新连接 / load）无引用即
/// drop，旧 forward 线程随 `Disconnected` 退出，避免旧模型内存泄漏。
fn forward_llm_events(
    app: AppHandle,
    rx: std::sync::mpsc::Receiver<LlmEvent>,
    stop_on_status: bool,
) {
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(10)) {
            Ok(LlmEvent::Token(delta)) => {
                let _ = app.emit("llm-token", delta);
            }
            Ok(LlmEvent::Finished(reason)) => {
                let _ = app.emit("llm-finished", reason);
                // 不 break：等待下一次生成的事件
            }
            Ok(LlmEvent::Error(e)) => {
                let _ = app.emit("llm-error", e);
                break;
            }
            Ok(LlmEvent::Status { ready }) => {
                let _ = app.emit("llm-status", LlmStatusPayload { ready });
                if stop_on_status {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // recv_timeout 本身已等待，无需额外 sleep
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// 读取 LLM 配置信息（远程连接配置 / 是否就绪）。
#[tauri::command]
fn get_llm_config(state: State<'_, LlmState>) -> Result<LlmConfigInfo, String> {
    let cfg = llm_resolved_config()?;
    let ready = state
        .engine
        .lock()
        .ok()
        .and_then(|e| e.as_ref().map(|e| e.is_ready()))
        .unwrap_or(false);
    Ok(LlmConfigInfo {
        enabled: cfg.enabled,
        provider: cfg.provider,
        ready,
        settings_path: zapmomo::config::settings::get_settings_path()
            .display()
            .to_string(),
        system_prompt: cfg.system_prompt,
        params: cfg.params,
        base_url: cfg.base_url,
        api_key: cfg.api_key,
        model: cfg.model,
        thinking: cfg.thinking,
        reasoning_effort: cfg.reasoning_effort,
    })
}

/// 创建/替换远程 LLM 引擎的核心逻辑。
///
/// 远程 provider 无本地模型加载，只需校验 base_url / model 配置后创建引擎实例。
/// 结果经 `llm-status`/`llm-error` 事件返回。
fn load_llm_impl(app: AppHandle, state: &LlmState) -> Result<(), String> {
    // LLM 正在生成时禁止替换（避免破坏 voice / GUI 的当前生成）。
    if app.state::<VoiceSessionState>().is_running() && llm_engine_is_generating(state) {
        return Err("语音会话正在使用 LLM 生成回复，请稍候再连接。".to_string());
    }
    let cfg = llm_resolved_config()?;
    if !cfg.enabled {
        return Err("LLM 功能未启用，请先在设置中启用。".to_string());
    }
    if cfg.base_url.as_deref().unwrap_or("").trim().is_empty() {
        return Err("未配置 API 地址（base_url），请先填写。".to_string());
    }
    if cfg.model.as_deref().unwrap_or("").trim().is_empty() {
        return Err("未配置模型名（model），请先填写。".to_string());
    }
    let engine = Arc::new(zapmomo::llm::LlmEngine::new(cfg).map_err(|e| e.to_string())?);
    engine.load().map_err(|e| e.to_string())?;
    *state.engine.lock().expect("llm lock poisoned") = Some(engine.clone());
    // 持续转发事件（voice 会话与 chat_llm 共用同一引擎，都由这一个 forward 转发，避免多线程重复 emit）
    std::thread::spawn(move || forward_llm_events(app, engine.subscribe(), false));
    Ok(())
}

/// 连接远程 LLM（异步：结果经 `llm-status`/`llm-error` 事件返回）。
#[tauri::command]
fn load_llm_model(app: AppHandle, state: State<'_, LlmState>) -> Result<(), String> {
    load_llm_impl(app, state.inner())
}

/// 卸载 LLM 模型并释放内存。仅当 LLM 正在生成时拒绝（语音会话空闲时可卸载，voice 会感知引擎变化）。
#[tauri::command]
fn unload_llm_model(app: AppHandle, state: State<'_, LlmState>) -> Result<(), String> {
    if app.state::<VoiceSessionState>().is_running() && llm_engine_is_generating(&state) {
        return Err("语音会话正在使用 LLM 生成回复，请稍候再卸载。".to_string());
    }
    let engine = state.engine.lock().expect("llm lock poisoned").take();
    if let Some(engine) = engine {
        engine.unload().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 发起一次流式对话（token 经 `llm-token`，结束经 `llm-finished`）。
///
/// 事件由统一的持续 forward（引擎就绪时 spawn）转发，此处不再额外 spawn。
#[tauri::command]
fn chat_llm(state: State<'_, LlmState>, text: String) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("文本不能为空".to_string());
    }
    let cfg = llm_resolved_config()?;
    let engine = state
        .engine
        .lock()
        .expect("llm lock poisoned")
        .clone()
        .ok_or("模型未连接，请先点击「连接」".to_string())?;
    if !engine.is_ready() {
        return Err("模型尚未就绪，请稍候".to_string());
    }
    let input = vec![InputItem::Message(ChatMessage::new(ChatRole::User, text))];
    engine
        .generate(input, cfg.params)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 取消当前生成。
#[tauri::command]
fn stop_llm(state: State<'_, LlmState>) -> Result<(), String> {
    if let Some(engine) = state.engine.lock().expect("llm lock poisoned").as_ref() {
        engine.cancel();
    }
    Ok(())
}

/// 模型是否已加载。
#[tauri::command]
fn is_llm_ready(state: State<'_, LlmState>) -> bool {
    state
        .engine
        .lock()
        .ok()
        .and_then(|e| e.as_ref().map(|e| e.is_ready()))
        .unwrap_or(false)
}

// ---- 语音会话（KWS→ASR→LLM→TTS 全链路）----

/// 把 `VoiceEvent` 转发为 Tauri 事件（`Started/BargeIn/Stopped` 是 CLI 噪音，忽略；
/// 终态由会话线程包装统一发 `voice-session-stopped`）。
fn make_voice_emit(app: AppHandle) -> Box<dyn Fn(VoiceEvent) + Send> {
    Box::new(move |ev| {
        // 镜像写入 tracing 日志（~/.zapmomo/logs/app.log），Tauri 模式下也能离线回溯语音会话
        zapmomo::voice::events::log_voice_event(&ev);
        match ev {
            VoiceEvent::Started
            | VoiceEvent::BargeIn
            | VoiceEvent::Stopped { .. }
            | VoiceEvent::FollowUp => {}
            VoiceEvent::State { state } => {
                let _ = app.emit(
                    "voice-session-state",
                    VoiceSessionStatePayload {
                        running: state != VoicePhase::Idle,
                        state,
                    },
                );
            }
            VoiceEvent::Wake { keyword } => {
                let _ = app.emit("voice-session-wake", VoiceWakePayload { keyword });
            }
            VoiceEvent::Transcript { text, is_final } => {
                // 最终用户句：持久化到对话记录（~/.zapmomo/conversations.json）
                if is_final {
                    records::append_record(records::ConversationRecord {
                        role: records::RecordRole::User,
                        text: text.clone(),
                        at: iso_timestamp_now(),
                    });
                }
                let _ = app.emit(
                    "voice-session-transcript",
                    VoiceTranscriptPayload { text, is_final },
                );
            }
            VoiceEvent::Token { delta } => {
                let _ = app.emit("voice-session-token", VoiceTokenPayload { delta });
            }
            VoiceEvent::ReplySentence { sentence } => {
                let _ = app.emit("voice-session-reply", VoiceReplyPayload { sentence });
            }
            VoiceEvent::PlaySentence { sentence } => {
                let _ = app.emit("voice-session-play", VoicePlayPayload { sentence });
            }
            VoiceEvent::ReplyFinished { reason, text } => {
                // 非空回复：持久化桌宠记录（空回复不落盘，避免空行）
                if let Some(text) = &text
                    && !text.is_empty()
                {
                    records::append_record(records::ConversationRecord {
                        role: records::RecordRole::Assistant,
                        text: text.clone(),
                        at: iso_timestamp_now(),
                    });
                }
                let _ = app.emit(
                    "voice-session-reply-finished",
                    VoiceReplyFinishedPayload { reason, text },
                );
            }
            VoiceEvent::Error { message, .. } => {
                let _ = app.emit("voice-session-error", VoiceErrorPayload { message });
            }
        }
    })
}

/// 启动语音会话：解析配置 → 确保 LlmState 唯一引擎存在 → spawn 会话线程
/// （线程内构造 `VoiceSession`，规避非 Send；注入共享 `Arc<LlmEngine>`，只加载一份模型）。
///
/// 会话构造/加载失败经 `voice-session-stopped{error}` 异步通知前端（启动静默降级）。
fn start_voice_session_impl(app: AppHandle, state: &VoiceSessionState) -> Result<(), String> {
    if state.is_running() {
        return Err("语音会话已在运行中".to_string());
    }
    let settings = zapmomo::config::settings::load_settings()?;
    let mut cfg =
        zapmomo::voice::config::resolve(settings.as_ref(), &VoiceCliOverrides::default())?;
    // active 角色包覆盖：人设（character.md）完全替代全局 system prompt；
    // 克隆模型（ZipVoice/OmniVoice）下注入角色音色。
    zapmomo::voice::config::apply_companion_overrides(&mut cfg);
    // 语音互动需 KWS 与 ASR 同时启用（持久化开关）：未启用则拒绝（自动/手动一致拦截）。
    let kws_enabled =
        zapmomo::kws::config::resolve(settings.as_ref().and_then(|s| s.kws.as_ref()), None)
            .map(|c| c.enabled)
            .unwrap_or(false);
    let asr_enabled =
        zapmomo::asr::config::resolve(settings.as_ref().and_then(|s| s.asr.as_ref()), None)
            .map(|c| c.enabled)
            .unwrap_or(false);
    if !(kws_enabled && asr_enabled) {
        return Err(
            "语音互动需要同时启用「唤醒词」(KWS) 与「语音识别」(ASR)。请在模型与能力页开启后重试。"
                .to_string(),
        );
    }
    // 同步预检模型文件：缺模型及时返回错误（也让 setup 的「voice 启动成功 → 跳过
    // LLM auto_load」判定可靠——voice 实际具备运行条件才返回 Ok）。
    preflight_voice_models(&cfg)?;

    // 统一 LLM 引擎：确保 `LlmState` 持有引擎（voice 与 GUI 共享，只加载一份）。
    // 未创建则创建并存入；加载延迟到 voice `run()` 内的 `load_blocking`。
    // voice 会话持「共享引擎槽」引用而非引擎 Arc 克隆——运行时引擎被外部切换
    // （set_current_model / load）时，voice 在每轮编排循环开头感知并重新绑定新引擎。
    let llm_state = app.state::<LlmState>();
    {
        let mut guard = llm_state.engine.lock().expect("llm lock poisoned");
        if guard.is_none() {
            let e =
                Arc::new(zapmomo::llm::LlmEngine::new(cfg.llm.clone()).map_err(|e| e.to_string())?);
            *guard = Some(e.clone());
            // 新引擎就绪后 spawn 持续 forward（GUI LLM 状态反映共享引擎）
            let app = app.clone();
            let e_for_fwd = e.clone();
            std::thread::spawn(move || forward_llm_events(app, e_for_fwd.subscribe(), false));
        }
    }

    let running = state.running.clone();
    running.store(true, Ordering::Relaxed);
    let emit = make_voice_emit(app.clone());
    let shared_llm_slot = llm_state.engine.clone();
    // TTS 热切换邮箱：宿主（set_current_model 写方）与会话各持一份 Arc
    let tts_swap_slot: zapmomo::voice::TtsSwapSlot = Arc::new(Mutex::new(None));
    *app.state::<VoiceSessionState>()
        .tts_swap
        .lock()
        .expect("voice tts_swap lock poisoned") = Some(tts_swap_slot.clone());
    // 文字输入通道：输入条窗口的 send_voice_text 命令 → 会话编排循环 poll_text_input
    let (text_tx, text_rx) = std::sync::mpsc::channel::<String>();
    *app.state::<VoiceSessionState>()
        .text_tx
        .lock()
        .expect("voice text_tx lock poisoned") = Some(text_tx);
    let handle = std::thread::spawn(move || {
        let mut session = match VoiceSession::new_with_parts(
            cfg,
            emit,
            running.clone(),
            Some(shared_llm_slot),
            Some(tts_swap_slot),
            Some(text_rx),
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("语音会话创建失败: {e}");
                running.store(false, Ordering::Relaxed);
                // 构造失败即线程退出：清空已登记的宿主通道，避免 send_voice_text /
                // TTS 事务臂拿到指向死接收端的句柄（残留会导致「会话已停止」误报）
                let voice = app.state::<VoiceSessionState>();
                *voice.text_tx.lock().expect("voice text_tx lock poisoned") = None;
                *voice.tts_swap.lock().expect("voice tts_swap lock poisoned") = None;
                let _ = app.emit(
                    "voice-session-stopped",
                    VoiceStoppedPayload { error: Some(e) },
                );
                return;
            }
        };
        // 暴露打断标志给宿主（全局快捷键「打断播报」置位用）
        *app.state::<VoiceSessionState>()
            .barge_in
            .lock()
            .expect("voice barge_in lock poisoned") = Some(session.barge_in_flag());
        let result = session.run();
        running.store(false, Ordering::Relaxed);
        *app.state::<VoiceSessionState>()
            .barge_in
            .lock()
            .expect("voice barge_in lock poisoned") = None;
        // 清空热切换邮箱（写方此后走「只写 selection」路径；残留 pending 引擎随 drop 释放）
        *app.state::<VoiceSessionState>()
            .tts_swap
            .lock()
            .expect("voice tts_swap lock poisoned") = None;
        // 清空文字输入通道（此后 send_voice_text 报「会话未运行」）
        *app.state::<VoiceSessionState>()
            .text_tx
            .lock()
            .expect("voice text_tx lock poisoned") = None;
        match &result {
            Ok(()) => tracing::info!("语音会话结束"),
            Err(e) => tracing::error!("语音会话异常: {e}"),
        }
        let _ = app.emit(
            "voice-session-stopped",
            VoiceStoppedPayload {
                error: result.err(),
            },
        );
    });
    *state.handle.lock().expect("voice handle lock poisoned") = Some(handle);
    Ok(())
}

/// 按模型族返回语音会话 ASR 必需文件（(标签, 解析后路径)）。
///
/// 不用 `asr_files_present_for_kind`（按 model_dir 探测）：preflight 需校验解析后的
/// 绝对路径（settings 可显式指定模型目录外的文件）。SenseVoice 主模型在 `cfg.model`
/// （Option）需 unwrap，resolve 已保证解析。
fn collect_asr_preflight_files(
    cfg: &zapmomo::asr::config::ResolvedAsrConfig,
) -> Result<Vec<(&'static str, &std::path::Path)>, String> {
    use zapmomo::asr::config::AsrModelKind;
    let files: Vec<(&'static str, &std::path::Path)> = match cfg.model_type {
        AsrModelKind::Zipformer => vec![
            ("ASR encoder", &cfg.encoder),
            ("ASR decoder", &cfg.decoder),
            ("ASR joiner", &cfg.joiner),
            ("ASR tokens", &cfg.tokens),
        ],
        AsrModelKind::Paraformer => vec![
            ("ASR encoder", &cfg.encoder),
            ("ASR decoder", &cfg.decoder),
            ("ASR tokens", &cfg.tokens),
        ],
        AsrModelKind::SenseVoice => {
            let model = cfg
                .model
                .as_deref()
                .ok_or_else(|| "SenseVoice 模型未解析出主模型文件".to_string())?;
            vec![("ASR model", model), ("ASR tokens", &cfg.tokens)]
        }
        AsrModelKind::Whisper => vec![
            ("ASR encoder", &cfg.encoder),
            ("ASR decoder", &cfg.decoder),
            ("ASR tokens", &cfg.tokens),
        ],
        AsrModelKind::Qwen3Asr => {
            let conv = cfg
                .model
                .as_deref()
                .ok_or_else(|| "Qwen3-ASR 模型未解析出 conv_frontend 文件".to_string())?;
            // tokenizer 是目录（非文件），不进 is_file 循环，单独校验
            if !cfg.tokens.is_dir() {
                return Err(format!("缺少 tokenizer 目录: {}", cfg.tokens.display()));
            }
            vec![
                ("ASR conv_frontend", conv),
                ("ASR encoder", &cfg.encoder),
                ("ASR decoder", &cfg.decoder),
            ]
        }
    };
    Ok(files)
}

/// 按 TTS `model_type` 收集预检文件清单（族感知，对齐 `collect_asr_preflight_files`）。
///
/// zipvoice 五件套；未收录的 kind（kitten/supertonic）返回空清单，交由引擎构造时报错。
fn collect_tts_preflight_files(
    cfg: &zapmomo::tts::config::ResolvedTtsConfig,
) -> Result<Vec<(&'static str, &std::path::Path)>, String> {
    use zapmomo::tts::config::TtsModelKind;
    Ok(match cfg.model_type {
        TtsModelKind::Zipvoice => vec![
            ("TTS encoder", cfg.encoder.as_path()),
            ("TTS decoder", cfg.decoder.as_path()),
            ("TTS vocoder", cfg.vocoder.as_path()),
            ("TTS tokens", cfg.tokens.as_path()),
            ("TTS lexicon", cfg.lexicon.as_path()),
        ],
        _ => vec![],
    })
}

/// 预检语音会话所需模型文件（KWS / ASR / TTS / LLM）。缺任一返回带安装提示的错误。
///
/// ASR 按 backend 分派：sherpa 走 `model_type` 族感知逐文件收集（zipformer 四件套 /
/// paraformer encoder+decoder+tokens / SenseVoice model+tokens / Whisper
/// encoder+decoder+tokens / Qwen3-ASR conv_frontend+encoder+decoder，tokenizer 目录
/// 单独校验）；audiocpp 走 `asr::config::preflight` 按族清单校验（单 GGUF 包，不查
/// sherpa 的 ONNX 清单，否则 Qwen3Asr 会误报缺 conv_frontend）。
/// TTS 按 backend 分派：sherpa 走逐文件收集；audiocpp 走
/// `tts::config::preflight`（按族清单，不查 sherpa 清单）。
fn preflight_voice_models(
    cfg: &zapmomo::voice::config::ResolvedSessionConfig,
) -> Result<(), String> {
    let mut files: Vec<(&'static str, &std::path::Path)> = vec![
        ("KWS encoder", cfg.kws.encoder.as_path()),
        ("KWS decoder", cfg.kws.decoder.as_path()),
        ("KWS joiner", cfg.kws.joiner.as_path()),
        ("KWS tokens", cfg.kws.tokens.as_path()),
        ("KWS keywords", cfg.kws.keywords_file.as_path()),
    ];
    match cfg.tts.backend {
        zapmomo::tts::config::TtsBackendKind::Audiocpp => {
            zapmomo::tts::config::preflight(&cfg.tts)
                .map_err(|e| format!("{e}\n（语音会话 TTS 预检失败）"))?;
        }
        zapmomo::tts::config::TtsBackendKind::Sherpa => {
            files.extend(collect_tts_preflight_files(&cfg.tts)?);
        }
    }
    match cfg.asr.backend {
        zapmomo::asr::config::AsrBackendKind::Audiocpp => {
            // audiocpp：单 GGUF 包，`preflight` 按族清单校验（缺失时带 registry 安装提示）
            zapmomo::asr::config::preflight(&cfg.asr)
                .map_err(|e| format!("{e}\n（语音会话 ASR 预检失败）"))?;
        }
        zapmomo::asr::config::AsrBackendKind::Sherpa => {
            files.extend(collect_asr_preflight_files(&cfg.asr)?);
        }
    }
    for (name, path) in files {
        if !path.is_file() {
            return Err(format!("缺少模型文件 {name}: {}", path.display()));
        }
    }
    if cfg.tts.model_type.requires_data_dir() && !cfg.tts.data_dir.is_dir() {
        return Err(format!("缺少 TTS 数据目录: {}", cfg.tts.data_dir.display()));
    }
    if !cfg.llm.enabled {
        return Err("语音会话需要启用 LLM，请先在设置中启用。".to_string());
    }
    if cfg.llm.base_url.as_deref().unwrap_or("").trim().is_empty() {
        return Err("语音会话需要配置 LLM API 地址（base_url），请先填写。".to_string());
    }
    if cfg.llm.model.as_deref().unwrap_or("").trim().is_empty() {
        return Err("语音会话需要配置 LLM 模型名（model），请先填写。".to_string());
    }
    Ok(())
}

/// 启动语音会话（进入待唤醒 Armed）。
#[tauri::command]
fn start_voice_session(app: AppHandle, state: State<'_, VoiceSessionState>) -> Result<(), String> {
    start_voice_session_impl(app, state.inner())
}

/// 停止语音会话的内部实现（command 与「切换设备重启」共用）。
fn stop_voice_session_inner(state: &VoiceSessionState) -> Result<(), String> {
    if !state.is_running() {
        return Err("语音会话未在运行中".to_string());
    }
    state.running.store(false, Ordering::Relaxed);
    if let Some(handle) = state
        .handle
        .lock()
        .expect("voice handle lock poisoned")
        .take()
    {
        let _ = handle.join();
    }
    // 会话线程 panic 时可能残留打断标志，这里兜底清空
    *state.barge_in.lock().expect("voice barge_in lock poisoned") = None;
    Ok(())
}

/// 停止语音会话（置停止标志并等待会话线程退出）。
#[tauri::command]
fn stop_voice_session(state: State<'_, VoiceSessionState>) -> Result<(), String> {
    stop_voice_session_inner(state.inner())
}

/// 语音会话是否在运行中。
#[tauri::command]
fn is_voice_session_running(state: State<'_, VoiceSessionState>) -> bool {
    state.is_running()
}

/// 发送文字消息（输入条窗口）：经 mpsc 送进会话编排循环，与 ASR 最终文本等价
/// 走 LLM → TTS → 落盘的完整对话链路。会话未运行时拒绝（不隐式启动/开麦克风）。
#[tauri::command]
fn send_voice_text(state: State<'_, VoiceSessionState>, text: String) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(()); // 空消息静默忽略（前端已拦截，这里兜底）
    }
    let tx = state
        .text_tx
        .lock()
        .expect("voice text_tx lock poisoned")
        .clone()
        .ok_or("语音互动未运行：请先在「对话记录」页开启语音互动，再发送文字消息".to_string())?;
    tx.send(text).map_err(|_| "语音会话已停止".to_string())
}

// ---- dsh 桥（deepseek-harness 任务事件 → 桌宠说话）----

/// dsh 桥状态：共享停止标志 + 线程句柄 + 实际监听端口（RuntimeActual）。
///
/// running/port 指向「当前一代」桥的标志：每次 start 整体替换为全新 Arc（fresh），
/// 服务线程持有克隆。stop 超时分离的旧线程迟到退出时只作用于自己那一代——
/// 不会读到新桥的 true 而复活，也不会把新桥的 running/port 清掉。
struct DshBridgeState {
    /// 当前代停止标志（false = 应退出）；start 时整体替换
    running: Mutex<Arc<AtomicBool>>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// 当前代实际监听端口（0 = 未运行/未就绪）；start 时整体替换
    port: Mutex<Arc<AtomicU16>>,
    /// 最近一次运行的错误（线程 epilogue 写入、start 清空；供前端轮询）
    last_error: Mutex<Option<String>>,
    /// 懒构建的播报器（TTS 模型未就绪时为 None，下次事件重试构建；失败不缓存）
    announcer: Mutex<Option<std::sync::Arc<zapmomo::dsh::announce::Announcer>>>,
    /// 懒构建的 LLM 播报 worker（进程级单例；桥重启不重建）
    llm_worker: Mutex<Option<std::sync::Arc<DshLlmWorker>>>,
}

impl DshBridgeState {
    fn new() -> Self {
        Self {
            running: Mutex::new(Arc::new(AtomicBool::new(false))),
            handle: Mutex::new(None),
            port: Mutex::new(Arc::new(AtomicU16::new(0))),
            last_error: Mutex::new(None),
            announcer: Mutex::new(None),
            llm_worker: Mutex::new(None),
        }
    }

    fn is_running(&self) -> bool {
        self.current_running().load(Ordering::Relaxed)
    }

    /// 当前代停止标志（克隆后读取，锁即刻释放）。
    fn current_running(&self) -> Arc<AtomicBool> {
        self.running
            .lock()
            .expect("dsh running lock poisoned")
            .clone()
    }

    /// 给定标志是否仍是「当前代」（线程 epilogue 写共享状态前的守卫：
    /// 分离的旧线程迟到退出时 start 已换新 Arc，此时不写 last_error、不发终态事件）。
    fn is_current_generation(&self, running: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.current_running(), running)
    }

    /// 当前实际监听端口（0 = 未运行/未就绪）。
    fn current_port(&self) -> u16 {
        self.port
            .lock()
            .expect("dsh port lock poisoned")
            .load(Ordering::Relaxed)
    }
}

/// `dsh-speak` 事件载荷（气泡台词 + 原始事件）。
#[derive(Clone, Serialize)]
struct DshSpeakPayload {
    text: String,
    event: zapmomo::dsh::event::DshEvent,
}

/// `dsh-bridge-status` 事件载荷。
#[derive(Clone, Serialize)]
struct DshBridgeStatusPayload {
    running: bool,
    port: Option<u16>,
    error: Option<String>,
}

/// dsh 事件处理管线：节流 → 投递给 LLM 播报 worker（覆盖式单槽）。
///
/// 桥线程只投递不等待：LLM 生成秒级耗时，原地等待会阻塞 serve 收下一条事件
/// （sink 在桥线程内联执行）；文案生成、气泡 emit、TTS 播报、落盘都在
/// dsh-llm worker 内完成。
fn handle_dsh_event(
    app: &AppHandle,
    throttle: &zapmomo::dsh::EventThrottle,
    event: zapmomo::dsh::event::DshEvent,
) {
    if !throttle.allow(&event) {
        tracing::debug!(
            "dsh 事件被节流丢弃: kind={} session={}",
            event.kind(),
            event.session_id()
        );
        return;
    }
    dsh_llm_worker(app).submit(event);
}

/// dsh LLM 播报 worker：覆盖式单槽 + 条件变量，串行执行「文案生成 → 气泡/TTS/落盘」。
///
/// - 桥线程只投递不等待（见 [`handle_dsh_event`]）
/// - 覆盖式：上一条未处理完时新事件替换旧事件——最新任务状态最值得播报
///   （同节流护栏口径：风暴场景只留最新）
/// - 线程常驻进程生命周期：空闲时阻塞在 Condvar 无 CPU 消耗，无独立资源需释放
struct DshLlmWorker {
    slot: Arc<WorkerSlot>,
}

struct WorkerSlot {
    event: Mutex<Option<zapmomo::dsh::event::DshEvent>>,
    signal: std::sync::Condvar,
}

impl DshLlmWorker {
    fn spawn(app: AppHandle) -> Self {
        let slot = Arc::new(WorkerSlot {
            event: Mutex::new(None),
            signal: std::sync::Condvar::new(),
        });
        let worker_slot = slot.clone();
        // 命名线程便于日志定位（同 dsh-announce / llm-worker 惯例）
        std::thread::Builder::new()
            .name("dsh-llm".to_string())
            .spawn(move || {
                loop {
                    let event = {
                        let mut guard = worker_slot
                            .event
                            .lock()
                            .expect("dsh-llm slot lock poisoned");
                        loop {
                            match guard.take() {
                                Some(ev) => break ev,
                                // 虚假唤醒安全：槽空继续等
                                None => {
                                    guard = worker_slot
                                        .signal
                                        .wait(guard)
                                        .expect("dsh-llm slot lock poisoned");
                                }
                            }
                        }
                    };
                    narrate_event(&app, event);
                }
            })
            .expect("spawn dsh-llm 线程失败");
        Self { slot }
    }

    /// 投递事件：槽中未处理事件被本条替换（只保留最新状态）。
    fn submit(&self, event: zapmomo::dsh::event::DshEvent) {
        let mut guard = self.slot.event.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            tracing::debug!("dsh LLM 播报忙，未处理事件被最新事件替换");
        }
        *guard = Some(event);
        self.slot.signal.notify_one();
    }
}

/// 取（或懒构建）LLM 播报 worker（进程级单例；桥重启不重建）。
fn dsh_llm_worker(app: &AppHandle) -> Arc<DshLlmWorker> {
    let state = app.state::<DshBridgeState>();
    let mut slot = state.llm_worker.lock().unwrap_or_else(|e| e.into_inner());
    slot.get_or_insert_with(|| Arc::new(DshLlmWorker::spawn(app.clone())))
        .clone()
}

/// 决策并产出播报文本：LLM 文案或模板台词（气泡 / TTS / 落盘共用同一份）。
///
/// 走 LLM 的条件见 `narrate::should_use_llm`（dsh/LLM 开关 + 引擎就绪 + 空闲）；
/// 生成发起失败 / 出错 / 超时 / 清洗后无文本一律回退模板台词（气泡永不缺席）。
fn narrate_text(app: &AppHandle, event: &zapmomo::dsh::event::DshEvent) -> String {
    // 事件时实时读设置，开关即时生效
    let settings = zapmomo::config::settings::load_settings().unwrap_or_default();
    let dsh_cfg = zapmomo::dsh::config::resolve(settings.as_ref().and_then(|s| s.dsh.as_ref()));
    let llm_cfg =
        match zapmomo::llm::config::resolve(settings.as_ref().and_then(|s| s.llm.as_ref())) {
            Ok(c) => c,
            // 配置解析异常视为 LLM 不可用，模板台词兜底
            Err(e) => {
                tracing::warn!("dsh LLM 配置解析失败，回退模板台词: {e}");
                return zapmomo::dsh::lines::pick_line(event, zapmomo::dsh::lines::next_roll());
            }
        };
    let engine = app
        .state::<LlmState>()
        .engine
        .lock()
        .expect("llm lock poisoned")
        .clone();
    let engine_ready = engine.as_ref().is_some_and(|e| e.is_ready());
    let engine_generating = engine.as_ref().is_some_and(|e| e.is_generating());
    if !zapmomo::dsh::narrate::should_use_llm(
        dsh_cfg.llm_enabled,
        llm_cfg.enabled,
        engine_ready,
        engine_generating,
    ) {
        return zapmomo::dsh::lines::pick_line(event, zapmomo::dsh::lines::next_roll());
    }
    let engine = engine.expect("should_use_llm 通过后引擎必然存在");
    zapmomo::dsh::narrate::generate_narration(
        &engine,
        event,
        &llm_cfg.params,
        zapmomo::dsh::narrate::NARRATE_TIMEOUT,
    )
    .unwrap_or_else(|| {
        tracing::info!("dsh LLM 播报未产出文本，回退模板台词");
        zapmomo::dsh::lines::pick_line(event, zapmomo::dsh::lines::next_roll())
    })
}

/// 单事件播报（在 dsh-llm worker 线程串行执行）：
/// LLM 文案（不可用/失败降级模板）→ `dsh-speak` 气泡 → TTS 播报 → 对话记录落盘。
fn narrate_event(app: &AppHandle, event: zapmomo::dsh::event::DshEvent) {
    let text = narrate_text(app, &event);
    tracing::info!("dsh 事件播报: kind={} text={text}", event.kind());
    let _ = app.emit(
        "dsh-speak",
        DshSpeakPayload {
            text: text.clone(),
            event,
        },
    );

    let settings = zapmomo::config::settings::load_settings().unwrap_or_default();
    let cfg = zapmomo::dsh::config::resolve(settings.as_ref().and_then(|s| s.dsh.as_ref()));
    // 语音播报：voice 会话运行中不出声（不打断对话）；TTS 未就绪只出气泡
    if cfg.voice_enabled
        && !app.state::<VoiceSessionState>().is_running()
        && let Some(announcer) = dsh_announcer(&app.state::<DshBridgeState>())
    {
        announcer.announce(&text);
    }
    // 落盘到对话记录（与语音会话同库，前端「对话记录」页可见）；
    // 空文本不落盘（与 voice ReplyFinished 的守卫一致，防御性）
    if cfg.record_to_history && !text.is_empty() {
        records::append_record(records::ConversationRecord {
            role: records::RecordRole::Assistant,
            text,
            at: iso_timestamp_now(),
        });
    }
}

/// 取（或懒构建）播报器：TTS 未就绪返回 None（只出气泡），失败不缓存。
fn dsh_announcer(
    state: &DshBridgeState,
) -> Option<std::sync::Arc<zapmomo::dsh::announce::Announcer>> {
    let mut slot = state.announcer.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = slot.as_ref() {
        return Some(a.clone());
    }
    match zapmomo::dsh::announce::Announcer::try_new() {
        Ok(a) => {
            let a = std::sync::Arc::new(a);
            *slot = Some(a.clone());
            Some(a)
        }
        Err(e) => {
            // warn 而非 debug：默认 info 日志下让运维能看到「为什么只有气泡没声音」
            // （合成失败路径在 worker 内已是 warn，两处对齐）
            tracing::warn!("dsh 播报器不可用（本次只出气泡，TTS 就绪后自动恢复）: {e}");
            None
        }
    }
}

/// 启动 dsh 桥：解析配置 → spawn 服务线程（绑 loopback、写发现文件、事件走管线）。
fn start_dsh_bridge_impl(app: AppHandle, state: &DshBridgeState) -> Result<(), String> {
    if state.is_running() {
        return Err("dsh 桥已在运行".to_string());
    }
    let settings = zapmomo::config::settings::load_settings()?;
    let cfg = zapmomo::dsh::config::resolve(settings.as_ref().and_then(|s| s.dsh.as_ref()));
    if !cfg.enabled {
        return Err("dsh 桥未启用".to_string());
    }
    // 清陈旧发现文件（上次退出未清理的残留）
    zapmomo::dsh::remove_discovery();

    // fresh Arc：每次启动换全新一代标志（线程持有克隆）——stop 超时分离的旧线程
    // 迟到退出只作用于自己那一代，不会复活（旧 running 恒 false）也不会污染新桥
    let running = Arc::new(AtomicBool::new(true));
    *state.running.lock().expect("dsh running lock poisoned") = running.clone();
    let port_flag = Arc::new(AtomicU16::new(0));
    *state.port.lock().expect("dsh port lock poisoned") = port_flag.clone();
    *state
        .last_error
        .lock()
        .expect("dsh last_error lock poisoned") = None;
    // 每次启动重新探测播报器（清空懒构建缓存：TTS 模型下载后立即生效，无需等重启）
    *state.announcer.lock().expect("dsh announcer lock poisoned") = None;
    let thread_app = app;
    let handle = std::thread::spawn(move || {
        tracing::info!("dsh bridge thread started");
        let token = zapmomo::dsh::generate_token();
        let token_for_file = token.clone();
        let port_for_ready = port_flag.clone();
        let app_for_ready = thread_app.clone();
        let mut on_ready = move |port: u16| {
            port_for_ready.store(port, Ordering::Relaxed);
            if let Err(e) = zapmomo::dsh::write_discovery(&zapmomo::dsh::DiscoveryInfo {
                port,
                token: token_for_file.clone(),
            }) {
                tracing::warn!("dsh 桥发现文件写入失败: {e}");
            }
            let _ = app_for_ready.emit(
                "dsh-bridge-status",
                DshBridgeStatusPayload {
                    running: true,
                    port: Some(port),
                    error: None,
                },
            );
        };
        let throttle = zapmomo::dsh::EventThrottle::new(std::time::Duration::from_secs(3));
        let app_for_sink = thread_app.clone();
        let mut sink = move |event: zapmomo::dsh::event::DshEvent| {
            handle_dsh_event(&app_for_sink, &throttle, event);
        };
        let result = zapmomo::dsh::serve(cfg.port, &token, &mut sink, &running, &mut on_ready);
        port_flag.store(0, Ordering::Relaxed);
        // token 条件清理：本线程若是迟到的分离线程，发现文件可能已属于重启后的
        // 新桥——只有 token 一致（仍是自己的文件）才删
        zapmomo::dsh::remove_discovery_if_token(&token);
        running.store(false, Ordering::Relaxed);
        match &result {
            Ok(()) => tracing::info!("dsh bridge thread finished (clean)"),
            Err(e) => tracing::error!("dsh bridge thread finished with error: {e}"),
        }
        // 本线程仍是当前代才写 last_error / 发终态事件（分离旧线程迟到退出
        // 不污染新桥的轮询状态与前端事件）
        let bridge_state = thread_app.state::<DshBridgeState>();
        if bridge_state.is_current_generation(&running) {
            let err = result.err();
            *bridge_state
                .last_error
                .lock()
                .expect("dsh last_error lock poisoned") = err.clone();
            let _ = thread_app.emit(
                "dsh-bridge-status",
                DshBridgeStatusPayload {
                    running: false,
                    port: None,
                    error: err,
                },
            );
        }
    });
    *state
        .handle
        .lock()
        .expect("dsh bridge handle lock poisoned") = Some(handle);
    Ok(())
}

/// 停止 dsh 桥：置停止标志后有界等待线程退出。
///
/// serve 的停止保证覆盖「请求间」检查；停滞客户端可能把线程卡在单次请求的
/// body 读取/响应抽干里（tiny_http 无 socket 读超时），此时超时后**分离线程**
/// （不 join，不阻塞调用方；线程随客户端断开或进程退出自然终结），running
/// 状态位仍保持一致。分离线程迟到退出只作用于自己那一代标志（fresh Arc）；
/// 发现文件若因此未由 epilogue 清理，下次启动会清陈旧残留。
fn stop_dsh_bridge_inner(state: &DshBridgeState) -> Result<(), String> {
    if !state.is_running() {
        return Err("dsh 桥未在运行".to_string());
    }
    state.current_running().store(false, Ordering::Relaxed);
    let handle = state
        .handle
        .lock()
        .expect("dsh bridge handle lock poisoned")
        .take();
    if let Some(handle) = handle {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if handle.is_finished() {
            let _ = handle.join();
        } else {
            tracing::warn!("dsh 桥线程未在 2s 内退出（可能被停滞客户端阻塞），已分离");
            // JoinHandle 无 Drop impl，forget 与 drop 同为分离语义；显式 forget 表达意图
            std::mem::forget(handle);
        }
    }
    Ok(())
}

/// GUI 展示用的 dsh 桥配置信息。
#[derive(Serialize)]
struct DshConfigInfo {
    enabled: bool,
    port: u16,
    voice_enabled: bool,
    llm_enabled: bool,
    record_to_history: bool,
    running: bool,
    /// 实际监听端口（RuntimeActual；None = 未就绪）
    actual_port: Option<u16>,
    /// 最近一次桥线程错误（启动失败/退出异常；None = 正常），供设置页展示
    error: Option<String>,
    discovery_path: String,
}

#[tauri::command]
fn get_dsh_config(state: State<'_, DshBridgeState>) -> Result<DshConfigInfo, String> {
    let settings = zapmomo::config::settings::load_settings()?;
    let cfg = zapmomo::dsh::config::resolve(settings.as_ref().and_then(|s| s.dsh.as_ref()));
    let actual = state.current_port();
    Ok(DshConfigInfo {
        enabled: cfg.enabled,
        port: cfg.port,
        voice_enabled: cfg.voice_enabled,
        llm_enabled: cfg.llm_enabled,
        record_to_history: cfg.record_to_history,
        running: state.is_running(),
        actual_port: (actual != 0).then_some(actual),
        error: state
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        discovery_path: zapmomo::dsh::discovery_file().display().to_string(),
    })
}

#[tauri::command]
fn set_dsh_enabled(
    app: AppHandle,
    state: State<'_, DshBridgeState>,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = zapmomo::config::settings::load_settings()?.unwrap_or_default();
    settings.dsh.get_or_insert_with(Default::default).enabled = Some(enabled);
    zapmomo::config::settings::save_settings(&settings)?;
    if enabled {
        if state.is_running() {
            // 幂等：目标态已达成直接成功（避免重复开关弹错误提示）
            Ok(())
        } else {
            start_dsh_bridge_impl(app, state.inner())
        }
    } else if state.is_running() {
        stop_dsh_bridge_inner(state.inner())
    } else {
        Ok(())
    }
}

/// `set_dsh_params` 载荷：可调整项（缺省不修改）。
#[derive(Debug, Clone, Default, Deserialize)]
struct DshParamsPatch {
    voice_enabled: Option<bool>,
    llm_enabled: Option<bool>,
    record_to_history: Option<bool>,
    port: Option<u16>,
}

#[tauri::command]
fn set_dsh_params(
    app: AppHandle,
    state: State<'_, DshBridgeState>,
    params: DshParamsPatch,
) -> Result<(), String> {
    if let Some(p) = params.port
        && p != 0
        && p < 1024
    {
        return Err(format!("端口需在 1024~65535 或 0（随机），当前 {p}"));
    }
    let mut settings = zapmomo::config::settings::load_settings()?.unwrap_or_default();
    // 端口与当前配置相同（未配置 ≡ 0 随机）则无需重启桥
    let port_changed = params
        .port
        .is_some_and(|p| settings.dsh.as_ref().and_then(|d| d.port).unwrap_or(0) != p);
    let dsh = settings.dsh.get_or_insert_with(Default::default);
    if let Some(v) = params.voice_enabled {
        dsh.voice_enabled = Some(v);
    }
    if let Some(v) = params.llm_enabled {
        dsh.llm_enabled = Some(v);
    }
    if let Some(v) = params.record_to_history {
        dsh.record_to_history = Some(v);
    }
    if let Some(v) = params.port {
        dsh.port = Some(v);
    }
    zapmomo::config::settings::save_settings(&settings)?;
    // 端口变化需重启桥生效；voice/record 项在事件时实时读取
    if port_changed && state.is_running() {
        stop_dsh_bridge_inner(state.inner())?;
        start_dsh_bridge_impl(app, state.inner())?;
    }
    Ok(())
}

#[tauri::command]
fn get_dsh_bridge_status(state: State<'_, DshBridgeState>) -> DshBridgeStatusPayload {
    let port = state.current_port();
    DshBridgeStatusPayload {
        running: state.is_running(),
        port: (port != 0).then_some(port),
        error: state
            .last_error
            .lock()
            .expect("dsh last_error lock poisoned")
            .clone(),
    }
}

/// 测试播报：灌一条假事件进管线（设置页按钮全链路验收，不用 curl）。
#[tauri::command]
fn test_dsh_announce(app: AppHandle) -> Result<(), String> {
    // 独立零窗口节流器：测试不受 3s 节流限制
    let throttle = zapmomo::dsh::EventThrottle::new(std::time::Duration::ZERO);
    handle_dsh_event(
        &app,
        &throttle,
        zapmomo::dsh::event::DshEvent::TaskFinished {
            session_id: "zapmomo-test".to_string(),
            title: Some("桌宠测试播报".to_string()),
            reason: Some("completed".to_string()),
        },
    );
    Ok(())
}

/// 读取持久化的对话记录（`~/.zapmomo/conversations.json`），供前端「对话记录」页载入。
#[tauri::command]
fn get_conversation_records() -> Vec<records::ConversationRecord> {
    records::load_records()
}

/// 清空持久化的对话记录。
#[tauri::command]
fn clear_conversation_records() -> Result<(), String> {
    records::clear_records()
}

/// 持久化远程 LLM 连接配置（base_url / api_key / model），写入 `[llm]`。
///
/// `None` 字段保持原有配置不变；`api_key` 为空串时清空。保存后需重新连接生效。
#[tauri::command]
fn set_llm_connection(
    base_url: String,
    api_key: Option<String>,
    model: String,
) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let llm = settings.llm.get_or_insert_with(LlmSettings::default);
    // provider 仅在未设置时默认 "openai"；已配置的（如 "anthropic"）保留，
    // 避免设置页保存连接时把其他 provider 重置回 OpenAI 兼容
    if llm.provider.is_none() {
        llm.provider = Some("openai".to_string());
    }
    if !base_url.trim().is_empty() {
        llm.base_url = Some(base_url.trim().to_string());
    }
    match api_key {
        Some(k) if !k.trim().is_empty() => llm.api_key = Some(k.trim().to_string()),
        Some(_) => llm.api_key = None,
        None => {}
    }
    if !model.trim().is_empty() {
        llm.model = Some(model.trim().to_string());
    }
    settings::save_settings(&settings)?;
    Ok(())
}

/// 批量持久化 LLM 采样参数，写入 `[llm]`。
///
/// 载荷为 `{ params: { temperature, top_p, ... } }`（snake_case 直传）；
/// `None` 字段保持原有配置不变。值先整体校验、再写入，出错时不部分修改。
#[tauri::command]
fn set_llm_params(params: LlmParamsPatch) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let llm = settings.llm.get_or_insert_with(LlmSettings::default);
    params.apply_to(llm)?;
    settings::save_settings(&settings)?;
    Ok(())
}

/// 持久化角色 system prompt，写入 `[llm].system_prompt`。
///
/// 空串会覆盖内置默认（模型收到空 system prompt）；改动需重新连接 provider 才生效。
#[tauri::command]
fn set_llm_system_prompt(prompt: String) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let llm = settings.llm.get_or_insert_with(LlmSettings::default);
    llm.system_prompt = Some(prompt);
    settings::save_settings(&settings)?;
    Ok(())
}

/// 持久化「是否启用语音合成」，写入 `[tts].enabled`（缺省 true）。
#[tauri::command]
fn set_tts_enabled(enabled: bool) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let tts = settings.tts.get_or_insert_with(TtsSettings::default);
    tts.enabled = Some(enabled);
    settings::save_settings(&settings)?;
    Ok(())
}

/// 批量持久化 TTS 合成参数（扩散步数/默认语速/线程/调试），写入 `[tts]`。
///
/// 载荷为 `{ params: { num_steps, speed, ... } }`（snake_case 直传）；
/// `None` 字段保持原有配置不变。值先整体校验、再写入，出错时不部分修改。
/// 引擎在每次合成时新建，因此保存后下一次合成即生效，无需重启。
#[tauri::command]
fn set_tts_params(params: TtsParamsPatch) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let tts = settings.tts.get_or_insert_with(TtsSettings::default);
    params.apply_to(tts)?;
    settings::save_settings(&settings)
}

/// 设定默认音色（写入 `[tts].voice`；`None` 恢复内置默认 leijun）。
///
/// 所有不显式指定音色的合成（测试语音 / 语音会话）都会用该默认音色，
/// 经 `resolve_reference` 回退生效。保存后下一次合成即生效，无需重启。
#[tauri::command]
fn set_tts_voice(voice: Option<String>) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let tts = settings.tts.get_or_insert_with(TtsSettings::default);
    tts.voice = voice;
    settings::save_settings(&settings)
}

/// 切换 TTS 推理后端（写入 `[tts].backend`）。高级/测试入口：常规入口是模型库
/// 「设为当前」（`set_selected_model` 按 registry runtime 同步写入）。
///
/// 切后端时同步重置 `model_type` 交回 resolve 目录探测（旧 kind 属于另一后端的
/// 模型），并在切回 sherpa 时复位 backend 覆盖。保存后下一次合成即生效。
#[tauri::command]
fn set_tts_backend(backend: String) -> Result<(), String> {
    let kind = zapmomo::tts::config::TtsBackendKind::parse_str(&backend)
        .ok_or_else(|| format!("未知 TTS 后端: {backend}（支持 sherpa / audiocpp）"))?;
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let tts = settings.tts.get_or_insert_with(TtsSettings::default);
    tts.backend = Some(kind.as_str().to_string());
    // 旧 model_type 属于另一后端的模型，交回 resolve 按目录探测
    tts.model_type = None;
    settings::save_settings(&settings)
}

/// 持久化「启用 KWS」开关，写入 `[kws].enabled`（缺省 false）。
/// 开关只持久化偏好；立即开始/停止监听由前端调用 `start_listen` / `stop_listen`，
/// 下次启动自动监听由 `.setup()` 判断 `[kws].enabled` 触发。
#[tauri::command]
fn set_kws_enabled(enabled: bool) -> Result<(), String> {
    tracing::info!("set_kws_enabled 命令被调用: enabled={enabled}");
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let kws = settings.kws.get_or_insert_with(KwsSettings::default);
    kws.enabled = Some(enabled);
    settings::save_settings(&settings)?;
    tracing::info!(
        "set_kws_enabled 已保存，[kws].enabled={:?}",
        settings.kws.as_ref().and_then(|k| k.enabled)
    );
    Ok(())
}

/// 持久化 ASR 启用状态，写入 `[asr].enabled`（语音会话「能识别」的前提）。
#[tauri::command]
fn set_asr_enabled(enabled: bool) -> Result<(), String> {
    tracing::info!("set_asr_enabled 命令被调用: enabled={enabled}");
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let asr = settings.asr.get_or_insert_with(AsrSettings::default);
    asr.enabled = Some(enabled);
    settings::save_settings(&settings)?;
    tracing::info!(
        "set_asr_enabled 已保存，[asr].enabled={:?}",
        settings.asr.as_ref().and_then(|a| a.enabled)
    );
    Ok(())
}

/// 持久化会话级自定义唤醒词，写入 `[kws].custom_keywords`（空串 → None = 模型内置）。
#[tauri::command]
fn set_kws_custom_keywords(keywords: String) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let kws = settings.kws.get_or_insert_with(KwsSettings::default);
    kws.custom_keywords = if keywords.trim().is_empty() {
        None
    } else {
        Some(keywords.trim().to_string())
    };
    settings::save_settings(&settings)
}

/// 持久化 KWS 引擎/运行参数（灵敏度/加权/块大小/线程/调试），写入 `[kws]`。
/// 引擎参数在启动监听时固化：修改后需重启监听才生效（由前端在保存后若在监听则重启）。
#[tauri::command]
fn set_kws_params(params: KwsParamsPatch) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let kws = settings.kws.get_or_insert_with(KwsSettings::default);
    params.apply_to(kws)?;
    settings::save_settings(&settings)
}

/// 持久化 ASR 引擎/运行参数（线程/块大小/断句/热词/标点/调试），写入 `[asr]`。
/// 引擎参数在启动识别时固化：修改后需重启识别才生效（由前端在保存后若在识别则重启）。
#[tauri::command]
fn set_asr_params(params: AsrParamsPatch) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let asr = settings.asr.get_or_insert_with(AsrSettings::default);
    params.apply_to(asr)?;
    settings::save_settings(&settings)
}

/// 读取全局默认麦克风输入设备名（空串 = 系统默认），KWS / ASR 共用。
#[tauri::command]
fn get_microphone() -> Result<String, String> {
    Ok(settings::load_settings()?
        .and_then(|s| s.microphone)
        .unwrap_or_default())
}

/// 设置并持久化全局默认麦克风（空串 → None = 系统默认）。
///
/// 若 KWS / ASR / 语音会话正在监听，用新设备自动重启对应监听，使切换立即生效；
/// 重启失败（如新设备不可用）返回错误，已停止的监听保持停止。
#[tauri::command]
fn set_microphone(
    app: AppHandle,
    listen: State<'_, ListenState>,
    asr_listen: State<'_, AsrListenState>,
    asr_dictate: State<'_, AsrDictateState>,
    voice: State<'_, VoiceSessionState>,
    mic: String,
) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    settings.microphone = if mic.trim().is_empty() {
        None
    } else {
        Some(mic.trim().to_string())
    };
    settings::save_settings(&settings)?;

    let new_mic = settings.microphone.clone();

    // KWS 监听运行中 → 用新设备重启（custom_keywords 从持久化配置读取）。
    if listen.is_listening() {
        stop_listen_inner(listen.inner())?;
        let kw = settings
            .kws
            .as_ref()
            .and_then(|k| k.custom_keywords.clone());
        start_listen_impl(app.clone(), listen.inner(), new_mic.clone(), kw)?;
    }
    // ASR 监听运行中 → 用新设备重启。
    if asr_listen.is_listening() {
        stop_asr_listen_inner(asr_listen.inner())?;
        start_asr_listen_impl(app.clone(), asr_listen.inner(), new_mic.clone())?;
    }
    // 离线听写运行中 → 用新设备重启。
    if asr_dictate.is_dictating() {
        stop_asr_dictate_inner(asr_dictate.inner())?;
        start_asr_dictate_impl(app.clone(), asr_dictate.inner(), new_mic.clone())?;
    }
    // 语音会话运行中 → 用新设备重启（会话内部 KWS/ASR 自持，重新加载新麦克风）。
    if voice.is_running() {
        stop_voice_session_inner(voice.inner())?;
        start_voice_session_impl(app.clone(), voice.inner())?;
    }
    Ok(())
}

/// 前端展示用的 BongoCat 道具资源信息（非 BongoCat 模型为 `None`）。
#[derive(Clone, Serialize)]
struct PerformancePropsView {
    /// 键盘背景图绝对路径（`resources/background.png`）。
    background: Option<String>,
    /// 按键贴图清单（爪子按在某键上的预渲染图）。
    keys: Vec<PerformanceKeyView>,
}

#[derive(Clone, Serialize)]
struct PerformanceKeyView {
    /// 键名（如 `KeyA`、`CapsLock`）。
    key: String,
    /// 贴图绝对路径。
    path: String,
    /// 所属的手：`"left"` / `"right"`。
    hand: String,
}

/// 把探测到的 BongoCat 道具资源转成前端可用的视图（绝对路径字符串）。
fn props_view(props: &zapmomo::live2d::config::BongoCatProps) -> PerformancePropsView {
    PerformancePropsView {
        background: props.background.as_ref().map(|p| p.display().to_string()),
        keys: props
            .keys
            .iter()
            .map(|k| PerformanceKeyView {
                key: k.key.clone(),
                path: k.path.display().to_string(),
                hand: match k.hand {
                    zapmomo::live2d::config::Hand::Left => "left",
                    zapmomo::live2d::config::Hand::Right => "right",
                }
                .to_string(),
            })
            .collect(),
    }
}

/// 探测 active 伙伴是否带 BongoCat 道具资源（相对模型清单所在目录）。
fn detect_active_bongocat_props() -> Option<PerformancePropsView> {
    let lib = zapmomo::companion::load_library_fast().ok()?;
    let active = zapmomo::companion::active_model(&lib)?;
    let model_dir = Path::new(&active.model_file)
        .parent()
        .unwrap_or(Path::new(&active.model_dir));
    zapmomo::live2d::config::detect_bongocat(model_dir).map(|p| props_view(&p))
}

/// GUI 展示用的 Live2D 配置信息。
#[derive(Serialize)]
struct Live2dConfigInfo {
    model_dir: Option<String>,
    model_file: Option<String>,
    format: Option<String>,
    models_present: bool,
    window_scale: Option<f64>,
    window_opacity: Option<f64>,
    click_through: Option<bool>,
    smart_click_through: Option<bool>,
    window_layer: Option<CompanionWindowLayer>,
    locked: Option<bool>,
    drag_mode: Option<CompanionDragMode>,
    settings_path: String,
    /// BongoCat 道具资源（非 BongoCat 模型为 `null`）。
    props: Option<PerformancePropsView>,
}

/// 读取 Live2D 配置，并在模型目录存在时重新放行 asset 协议 scope。
///
/// asset 协议 scope 不跨进程持久，因此每次启动/读取都要重新
/// `allow_directory`，否则 WebView 无法加载模型文件。
///
/// 模型路径优先从伙伴库 active 读取（库是唯一 Source of Truth，且 GIF 伙伴
/// 无 `.model3.json`，`resolve()` 扫描不到）；库无 active 时回退旧版
/// `settings.model_dir` 解析（后台旧版迁移完成前的窗口期桌宠仍可显示）。
#[tauri::command]
fn get_live2d_config(app: AppHandle) -> Result<Live2dConfigInfo, String> {
    let settings = settings::load_settings()?;
    let live2d_settings = settings.as_ref().and_then(|s| s.live2d.clone());

    let lib = zapmomo::companion::load_library_fast()?;
    let active =
        zapmomo::companion::active_model(&lib).filter(|m| zapmomo::companion::quick_valid(m));
    // active 伙伴的私有布局（尺寸/位置）；None = 未单独配置，回退全局默认。
    let active_layout = active.as_ref().and_then(|m| m.layout.clone());
    let (model_dir, model_file, format, models_present) = match active {
        Some(m) => (
            Some(m.model_dir.clone()),
            Some(m.model_file.clone()),
            Some(m.format.clone()),
            true,
        ),
        None => {
            let cfg = zapmomo::live2d::config::resolve(live2d_settings.as_ref())?;
            let present = cfg.model_file.as_ref().is_some_and(|f| f.is_file());
            (
                Some(cfg.model_dir.display().to_string()),
                cfg.model_file.map(|p| p.display().to_string()),
                cfg.format.map(|f| f.to_str().to_string()),
                present,
            )
        }
    };
    if models_present && let Some(dir) = &model_dir {
        let _ = app.asset_protocol_scope().allow_directory(dir, true);
    }

    // 有效缩放：active 伙伴私有 layout 优先，全局 [live2d].window_scale 兜底。
    let window_scale = active_layout
        .as_ref()
        .and_then(|l| l.scale)
        .or_else(|| live2d_settings.as_ref().and_then(|l| l.window_scale));
    let window_opacity = live2d_settings.as_ref().and_then(|l| l.window_opacity);
    let click_through = live2d_settings.as_ref().and_then(|l| l.click_through);
    let smart_click_through = live2d_settings.as_ref().and_then(|l| l.smart_click_through);
    let window_layer = live2d_settings.as_ref().and_then(|l| l.window_layer);
    let locked = live2d_settings.as_ref().and_then(|l| l.locked);
    let drag_mode = live2d_settings.as_ref().and_then(|l| l.drag_mode);

    Ok(Live2dConfigInfo {
        model_dir,
        model_file,
        format,
        models_present,
        window_scale,
        window_opacity,
        click_through,
        smart_click_through,
        window_layer,
        locked,
        drag_mode,
        settings_path: settings::get_settings_path().display().to_string(),
        props: detect_active_bongocat_props(),
    })
}

/// `live2d-model-changed` 事件载荷（切换到某伙伴 / 清屏）。
/// 字段为 `Option`：清屏时均为 `None`。
#[derive(Clone, Serialize)]
struct Live2dModelInfo {
    model_dir: Option<String>,
    model_file: Option<String>,
    format: Option<String>,
    /// BongoCat 道具资源（非 BongoCat 模型为 `null`）。
    props: Option<PerformancePropsView>,
    /// 该伙伴的私有缩放；`None` = 未单独配置，角色窗口沿用当前状态。
    window_scale: Option<f64>,
    /// 该伙伴的私有窗口位置（已做多屏可达校验，落屏外时降级为 `None` 沿用当前位置）。
    window_position: Option<CompanionWindowPosition>,
}

// ---------------------------------------------------------------------------
// 伙伴库（Companion Library）命令
// ---------------------------------------------------------------------------

/// 前端展示用的伙伴信息（snake_case，与 `Live2dConfigInfo` 一致）。
#[derive(Serialize)]
struct CompanionView {
    id: String,
    name: String,
    source_path: Option<String>,
    model_dir: String,
    model_file: String,
    format: String,
    imported_at: String,
    /// 快速有效判定：托管目录与清单文件是否都还在磁盘上。
    valid: bool,
    /// 探测到的封面图绝对路径（best-effort；无封面图为 null，前端用占位图标）。
    cover_image: Option<String>,
    /// 角色包是否带人设（character.md 非空；非角色包恒 false）。
    has_persona: bool,
    /// 角色包是否带音色克隆参考（voice/reference.wav + reference.txt 成对）。
    has_voice: bool,
}

#[derive(Serialize)]
struct CompanionLibraryView {
    models: Vec<CompanionView>,
    active_model_id: Option<String>,
}

#[derive(Serialize)]
struct ImportCompanionResult {
    library: CompanionLibraryView,
    model_id: String,
    already_imported: bool,
}

fn build_view(lib: &zapmomo::companion::CompanionLibrary) -> CompanionLibraryView {
    CompanionLibraryView {
        models: lib
            .models
            .iter()
            .map(|m| CompanionView {
                id: m.id.clone(),
                name: m.name.clone(),
                source_path: m.source_path.clone(),
                model_dir: m.model_dir.clone(),
                model_file: m.model_file.clone(),
                format: m.format.clone(),
                imported_at: m.imported_at.clone(),
                valid: zapmomo::companion::quick_valid(m),
                cover_image: zapmomo::live2d::config::find_cover_image(Path::new(&m.model_dir))
                    .map(|p| p.display().to_string()),
                has_persona: zapmomo::companion::has_persona(m),
                has_voice: zapmomo::companion::has_character_voice(m),
            })
            .collect(),
        active_model_id: lib.active_model_id.clone(),
    }
}

/// 把 `settings.toml [live2d].model_dir` 同步成伙伴库 active（**幂等**：值相同则
/// 不写不 emit，避免每次 `list_companions` 都触发桌宠重载）。
///
/// 唯一逻辑 Source of Truth 是 `CompanionLibrary.active_model_id`；settings 里的
/// `model_dir` 只是兼容 `CompanionRoot` / `get_live2d_config` / `live2d-model-changed`
/// 的 derived runtime cache，最终一致由本函数负责。
fn reconcile_active(
    app: &AppHandle,
    active: Option<&zapmomo::companion::CompanionModel>,
) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);

    let desired: Option<String> = active.map(|m| m.model_dir.clone());
    if live2d.model_dir == desired {
        return Ok(());
    }

    live2d.model_dir = desired;
    settings::save_settings(&settings)?;

    match active {
        Some(model) => {
            app.asset_protocol_scope()
                .allow_directory(Path::new(&model.model_dir), true)
                .map_err(|e| format!("无法放行模型目录: {e}"))?;
            // BongoCat 道具探测相对模型清单所在目录（毫秒级小 IO，与 validate 同量级）。
            let model_dir = Path::new(&model.model_file)
                .parent()
                .unwrap_or(Path::new(&model.model_dir));
            let props = zapmomo::live2d::config::detect_bongocat(model_dir).map(|p| props_view(&p));
            let info = Live2dModelInfo {
                model_dir: Some(model.model_dir.clone()),
                model_file: Some(model.model_file.clone()),
                format: Some(model.format.clone()),
                props,
                // 伙伴私有布局随切换事件下发：角色窗口据此恢复该伙伴的尺寸/位置；
                // 位置落屏外（多屏布局已变化）时降级 None，前端沿用当前位置。
                window_scale: model.layout.as_ref().and_then(|l| l.scale),
                window_position: model
                    .layout
                    .as_ref()
                    .and_then(|l| l.position.clone())
                    .filter(|p| position_on_any_monitor(app, p.x as f64, p.y as f64)),
            };
            // 通知常驻角色窗口即时重载新模型（同进程事件，跨窗口同步）。
            let _ = app.emit("live2d-model-changed", &info);
        }
        None => {
            // 清屏：桌宠收到空 model_file 后清除当前模型。
            let info = Live2dModelInfo {
                model_dir: None,
                model_file: None,
                format: None,
                props: None,
                window_scale: None,
                window_position: None,
            };
            let _ = app.emit("live2d-model-changed", &info);
        }
    }

    // active 实际变更 → 人设/音色快照需重建：运行中的语音会话 stop+start
    // （set_microphone 同款模式；重启同时清 history，人设语义干净）。
    // 失败仅告警：人设/音色是增强，不应让「切换伙伴」本身失败。
    let voice = app.state::<VoiceSessionState>();
    if voice.is_running()
        && let Err(e) = stop_voice_session_inner(voice.inner())
            .and_then(|()| start_voice_session_impl(app.clone(), voice.inner()))
    {
        tracing::warn!("切换伙伴后重启语音会话失败（人设/音色将在下次启动会话时生效）: {e}");
    }
    Ok(())
}

/// 启动阶段同步 reconcile（毫秒级，不迁移）：让 settings 与伙伴库 active 一致，
/// 使 `CompanionRoot` 挂载时 `get_live2d_config` 就读到正确的当前伙伴。
///
/// **库空时不主动清空 settings**：旧版 `settings.model_dir` 仍由后台迁移继续使用，
/// 避免「后台迁移完成前桌宠闪空」。只有库中解析出 active 时才应用。
fn reconcile_active_at_startup(app: &AppHandle) {
    match zapmomo::companion::load_library_fast() {
        Ok(lib) => {
            let active = zapmomo::companion::active_model(&lib);
            if let Some(model) = active
                && let Err(e) = reconcile_active(app, Some(model))
            {
                tracing::warn!("启动同步伙伴配置失败（将在下次打开伙伴页自愈）: {e}");
            }
        }
        Err(e) => tracing::warn!("读取伙伴库失败（跳过启动同步）: {e}"),
    }
}

/// 后台旧版迁移：库为空且旧 `[live2d].model_dir` 存在时，复制进托管目录并设为 active，
/// 完成后重新 reconcile（桌宠从旧目录无缝切到托管副本）。不阻塞启动。
fn migrate_legacy_in_background(app: AppHandle) {
    tauri::async_runtime::spawn_blocking(
        move || match zapmomo::companion::migrate_legacy_if_empty() {
            Ok(Some(_id)) => {
                if let Ok(lib) = zapmomo::companion::load_library_fast() {
                    let active = zapmomo::companion::active_model(&lib);
                    if let Err(e) = reconcile_active(&app, active) {
                        tracing::warn!("迁移后同步伙伴配置失败: {e}");
                    }
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("旧版模型后台迁移失败（将在下次打开伙伴页重试）: {e}"),
        },
    );
}

/// 后台存量迁移：为已导入伙伴补注册未登记的动作/表情文件（幂等，不阻塞启动；
/// 失败不写标记，下次启动自动重试）。
fn register_motions_in_background() {
    tauri::async_runtime::spawn_blocking(move || {
        match zapmomo::companion::register_motions_for_existing() {
            Ok(n) if n > 0 => tracing::info!("已为 {n} 个伙伴补注册动作/表情文件"),
            Ok(_) => {}
            Err(e) => tracing::warn!("补注册动作/表情迁移失败（下次启动重试）: {e}"),
        }
    });
}

/// 列出伙伴库（含旧版迁移兜底 + sanitize active）。
#[tauri::command]
async fn list_companions(app: AppHandle) -> Result<CompanionLibraryView, String> {
    let lib = tauri::async_runtime::spawn_blocking(zapmomo::companion::load_library)
        .await
        .map_err(|e| e.to_string())??;
    // 放行所有有效托管目录的 asset scope（settings 窗口启动不再全局调 get_live2d_config，
    // 伙伴页预览依赖此处放行；scope 不跨进程持久，每次都要重新放行）。
    for model in &lib.models {
        if zapmomo::companion::quick_valid(model) {
            let _ = app
                .asset_protocol_scope()
                .allow_directory(Path::new(&model.model_dir), true);
        }
    }
    let active = zapmomo::companion::active_model(&lib);
    reconcile_active(&app, active)?;
    Ok(build_view(&lib))
}

/// 导入伙伴（Live2D 模型目录或 GIF 动图文件，复制到应用托管目录并登记进伙伴库）。
///
/// 成功或已导入都会立即放行新模型的 asset scope，保证右侧预览无需再进页面；
/// 若本次导入成为 active（首次导入自动 active）则 reconcile 同步桌宠。
#[tauri::command]
async fn import_companion(app: AppHandle, source: String) -> Result<ImportCompanionResult, String> {
    let source_path = PathBuf::from(source);
    let (model, already_imported) = tauri::async_runtime::spawn_blocking(move || {
        zapmomo::companion::import_source(&source_path)
    })
    .await
    .map_err(|e| e.to_string())??;

    app.asset_protocol_scope()
        .allow_directory(Path::new(&model.model_dir), true)
        .map_err(|e| format!("无法放行模型目录: {e}"))?;

    let lib = zapmomo::companion::load_library_fast()?;
    let became_active = lib.active_model_id.as_deref() == Some(model.id.as_str());
    if became_active {
        stop_performance_inner(&app);
        let active = zapmomo::companion::active_model(&lib);
        reconcile_active(&app, active)?;
    }
    // 新导入条目要出现在托盘「切换伙伴」子菜单（无论是否成为 active）。
    rebuild_tray_menu_threadsafe(&app);

    Ok(ImportCompanionResult {
        library: build_view(&lib),
        model_id: model.id.clone(),
        already_imported,
    })
}

/// 设置「当前使用」伙伴（Library 先持久化成功，再 reconcile 同步 settings 与桌宠）。
#[tauri::command]
async fn set_active_companion(app: AppHandle, id: String) -> Result<CompanionLibraryView, String> {
    stop_performance_inner(&app);
    let lib = tauri::async_runtime::spawn_blocking(move || zapmomo::companion::set_active(&id))
        .await
        .map_err(|e| e.to_string())??;
    let active = zapmomo::companion::active_model(&lib);
    reconcile_active(&app, active)?;
    // 设置页切换后托盘「切换伙伴」勾选要移动。
    rebuild_tray_menu_threadsafe(&app);
    Ok(build_view(&lib))
}

/// 重命名伙伴（只改展示名；不影响 active / 桌宠，reconcile 为幂等 no-op）。
#[tauri::command]
async fn rename_companion(
    app: AppHandle,
    id: String,
    name: String,
) -> Result<CompanionLibraryView, String> {
    let lib = tauri::async_runtime::spawn_blocking(move || zapmomo::companion::rename(&id, &name))
        .await
        .map_err(|e| e.to_string())??;
    let active = zapmomo::companion::active_model(&lib);
    reconcile_active(&app, active)?;
    // 重命名后托盘「切换伙伴」label 要更新。
    rebuild_tray_menu_threadsafe(&app);
    Ok(build_view(&lib))
}

/// 移除伙伴：删除托管目录与库条目；若删的是 active，自动落到第一个有效伙伴或清空，
/// 并 reconcile 同步桌宠（切换到新 active 或清屏）。
#[tauri::command]
async fn remove_companion(app: AppHandle, id: String) -> Result<CompanionLibraryView, String> {
    // 删除的若是 active，先停表演（reconcile 清屏/切换前让 stopped 先发出）。
    stop_performance_inner(&app);
    let lib = tauri::async_runtime::spawn_blocking(move || zapmomo::companion::remove(&id))
        .await
        .map_err(|e| e.to_string())??;
    let active = zapmomo::companion::active_model(&lib);
    reconcile_active(&app, active)?;
    // 条目减少 / active 落位或清屏后托盘子菜单要刷新。
    rebuild_tray_menu_threadsafe(&app);
    Ok(build_view(&lib))
}

/// 保存前端从 Live2D 渲染画布截取的 PNG 封面（写入托管目录 `cover.png`）。
#[tauri::command]
async fn save_cover_image(id: String, png: Vec<u8>) -> Result<CompanionLibraryView, String> {
    tauri::async_runtime::spawn_blocking(move || zapmomo::companion::save_cover(&id, &png))
        .await
        .map_err(|e| e.to_string())??;
    let lib = zapmomo::companion::load_library_fast()?;
    Ok(build_view(&lib))
}

/// 持久化角色窗口位置（逻辑像素），供下次启动恢复。
///
/// 由前端在用户手动拖动窗口后（debounce）调用。位置是**伙伴私有**配置：
/// 写入 `library.json` 中 active 伙伴的 `layout.position`；无 active 伙伴时
/// （空库窗口期）回退写 `settings.toml [live2d.window_position]` 全局默认。
#[tauri::command]
fn save_companion_position(x: i32, y: i32) -> Result<(), String> {
    if let Some(id) = active_companion_id() {
        return zapmomo::companion::save_layout_position(&id, x, y);
    }
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.window_position = Some(CompanionWindowPosition { x, y });
    settings::save_settings(&settings)
}

/// 持久化文字输入条窗口位置（逻辑像素），供下次启动恢复。
///
/// 由前端在用户手动拖动窗口后（debounce）调用，写入 `~/.zapmomo/settings.toml`
/// 的 `[chatbox.window_position]` 段。
#[tauri::command]
fn save_chatbox_position(x: i32, y: i32) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let chatbox = settings
        .chatbox
        .get_or_insert_with(ChatboxSettings::default);
    chatbox.window_position = Some(CompanionWindowPosition { x, y });
    settings::save_settings(&settings)
}

/// 持久化语音回复气泡窗口位置（逻辑像素），供下次启动恢复。
///
/// 由前端在用户手动拖动窗口后（debounce）调用，写入 `~/.zapmomo/settings.toml`
/// 的 `[bubble.window_position]` 段。
#[tauri::command]
fn save_bubble_position(x: i32, y: i32) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let bubble = settings.bubble.get_or_insert_with(BubbleSettings::default);
    bubble.window_position = Some(CompanionWindowPosition { x, y });
    settings::save_settings(&settings)
}

/// （临时调试）气泡窗口前端状态日志：排查「气泡无法拖动」——确认点穿切换与
/// 拖动事件是否到达。验收通过后删除。
#[tauri::command]
fn bubble_debug_log(message: String) {
    tracing::info!(target: "bubble_debug", "{message}");
}

/// 隐藏文字输入条窗口并持久化开关（前端 Esc 关闭时调用，保持菜单勾选态一致）。
#[tauri::command]
fn hide_chatbox(app: AppHandle) {
    set_chatbox_visible(&app, false, false);
}

/// 显示/隐藏文字输入条窗口并持久化开关状态（托盘/右键菜单勾选共用）。
///
/// `focus` 仅显示时生效（显示后可直接打字）。macOS 上 chatbox 是非激活面板，
/// 显示与聚焦走 NSPanel 专用 API（`orderFrontRegardless` + `makeKeyWindow`）：
/// 聚焦输入不激活应用，不会把本应用其它可见窗口（如设置窗）一并带到最前；
/// 其它平台为普通窗口，聚焦会激活应用。
fn set_chatbox_visible(app: &AppHandle, visible: bool, focus: bool) {
    if let Some(window) = app.get_webview_window("chatbox") {
        // macOS：tao 的 window.show()/set_focus() 底层是 makeKeyAndOrderFront /
        // activateIgnoringOtherApps——后者无条件激活整个 App，AppKit 激活时会把本应用
        // 全部可见窗口（如一直开着的设置窗）整体带到最前，显隐快捷键因此表现为
        // 「连带弹出主界面」。NonactivatingPanel mask 只抑制用户点击面板时的隐式
        // 激活，管不住显式 activate 调用。改走 panel 的 orderFrontRegardless +
        // makeKeyWindow：输入条成为 key window 可直接打字，但 App 不激活（Spotlight 式）。
        #[cfg(target_os = "macos")]
        {
            use tauri_nspanel::ManagerExt;
            match app.get_webview_panel("chatbox") {
                Ok(panel) => {
                    if visible {
                        if focus {
                            panel.show_and_make_key();
                        } else {
                            panel.show();
                        }
                    } else {
                        panel.hide();
                    }
                }
                // panel 注册前的时序兜底（正常运行不可达）：退回 tauri 原路径
                Err(_) => {
                    let _ = if visible {
                        window.show()
                    } else {
                        window.hide()
                    };
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = if visible && focus {
                window.show().and_then(|()| window.set_focus())
            } else if visible {
                window.show()
            } else {
                window.hide()
            };
        }
    }
    if let Ok(mut settings) = settings::load_settings() {
        let settings = settings.get_or_insert_with(Default::default);
        let chatbox = settings
            .chatbox
            .get_or_insert_with(ChatboxSettings::default);
        chatbox.visible = Some(visible);
        let _ = settings::save_settings(settings);
    }
    rebuild_tray_menu(app);
}

/// 保存角色窗口缩放比例并通知角色窗口（内部实现，供 command 与原生菜单事件共用）。
///
/// 缩放是**伙伴私有**配置：写入 `library.json` 中 active 伙伴的 `layout.scale`；
/// 无 active 伙伴时回退写 `settings.toml [live2d.window_scale]` 全局默认。
fn apply_companion_scale(app: &AppHandle, scale: f64) -> Result<(), String> {
    if let Some(id) = active_companion_id() {
        zapmomo::companion::save_layout_scale(&id, scale)?;
    } else {
        let mut settings = settings::load_settings()?.unwrap_or_default();
        let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
        live2d.window_scale = Some(scale);
        settings::save_settings(&settings)?;
    }
    let _ = app.emit("companion-scale-changed", scale);
    rebuild_tray_menu(app);
    Ok(())
}

/// 保存角色窗口透明度并通知角色窗口（内部实现，供 command 与原生菜单事件共用）。
fn apply_companion_opacity(app: &AppHandle, opacity: f64) -> Result<(), String> {
    let opacity = clamp_opacity(opacity);
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.window_opacity = Some(opacity);
    settings::save_settings(&settings)?;
    let _ = app.emit("companion-opacity-changed", opacity);
    rebuild_tray_menu(app);
    Ok(())
}

/// 智能穿透轮询周期（≈30Hz）：每 tick 一次 OS 光标直读 + 线程本地计算。
const POINTER_TICK: std::time::Duration = std::time::Duration::from_millis(33);
/// 拖动保护期：窗口移动后保持可交互的时长（拖动中切穿透会打断 startDragging
/// 的系统拖动循环，是此类实现翻车头号原因）。
const DRAG_HOLD: std::time::Duration = std::time::Duration::from_millis(600);
/// 右键菜单保护期：原生菜单无关闭回调，定时兜底（期间强制可交互）。
const MENU_HOLD: std::time::Duration = std::time::Duration::from_millis(4000);

/// 角色窗口指针穿透状态：智能穿透轮询与单一写点共享的全部输入缓存。
///
/// policy 由各 `apply_companion_*` 与启动路径刷新（读 settings）；region 由前端经
/// `set_companion_hit_region` 上报；origin/size/scale 由窗口事件缓存（轮询每 tick
/// 仅做 OS 光标直读与一次 `is_visible`，不走 dispatcher getter 往返）；
/// ignore_written 是已写入系统的值，写点据此跳过冗余系统调用。
struct CompanionPointerState {
    policy: Mutex<CompanionPointerPolicy>,
    /// 前端上报的角色可交互矩形（窗口内逻辑像素）；`None` = 前端未就绪
    /// （启动/加载中/加载失败 → fail-open 判可交互）；`Some([])` = 清屏（穿透）。
    region: Mutex<Option<Vec<HitRect>>>,
    /// 窗口外框左上角（全局物理像素；`Moved` 事件缓存）。
    origin: Mutex<Option<PhysicalPosition<f64>>>,
    /// 窗口外框尺寸（物理像素；`Resized`/`ScaleFactorChanged` 事件缓存）。
    size: Mutex<Option<PhysicalSize<f64>>>,
    /// 窗口所在屏缩放（`ScaleFactorChanged` 事件缓存）。
    scale: Mutex<f64>,
    /// 最后一次窗口移动时刻（`DRAG_HOLD` 内视为拖动中，保护期持续顺延）。
    last_move_at: Mutex<Option<std::time::Instant>>,
    /// 保护期截止时刻（拖动/右键菜单期间强制可交互）。
    hold_until: Mutex<Option<std::time::Instant>>,
    /// 已写入系统的 ignore_cursor_events 值；写点据此跳过冗余系统调用
    /// （Windows GWL_EXSTYLE 改写 / macOS 属性重排都不便宜）。
    ignore_written: AtomicBool,
}

impl CompanionPointerState {
    fn new() -> Self {
        Self {
            policy: Mutex::new(CompanionPointerPolicy {
                visible: true,
                layer: CompanionWindowLayer::Front,
                force_click_through: false,
                smart_enabled: true,
            }),
            region: Mutex::new(None),
            origin: Mutex::new(None),
            size: Mutex::new(None),
            scale: Mutex::new(1.0),
            last_move_at: Mutex::new(None),
            hold_until: Mutex::new(None),
            ignore_written: AtomicBool::new(false),
        }
    }
}

/// 从 settings 刷新穿透策略快照（`visible` 不在此列：写点处现查窗口实况）。
fn refresh_companion_pointer_policy(app: &AppHandle) {
    let (force, smart) = match settings::load_settings() {
        Ok(Some(s)) => (
            resolve_click_through(s.live2d.as_ref()),
            resolve_smart_click_through(s.live2d.as_ref()),
        ),
        _ => (false, true),
    };
    let state = app.state::<CompanionPointerState>();
    *state.policy.lock().unwrap_or_else(|e| e.into_inner()) = CompanionPointerPolicy {
        visible: true,
        layer: current_companion_layer(),
        force_click_through: force,
        smart_enabled: smart,
    };
}

/// 单一权威写点：按策略快照 + 光标命中 + 保护期重算穿透并应用（值不变跳过系统调用）。
///
/// 全平台统一走 tauri `set_ignore_cursor_events`——macOS 上与 tauri-nspanel 的
/// `setIgnoresMouseEvents:` 打到同一 NSWindow/NSPanel selector，companion 转面板后
/// 依然有效（历史上两条路径已在同一窗口双用）。这是唯一调用该 API 的地方：
/// 手动穿透、层级切换（含 Windows 置底点穿）、智能穿透轮询全部经此收敛，
/// 消除多点写入竞争（含层级切换静默清掉手动穿透的既有 bug）。
fn sync_companion_ignore_cursor_events_hit(app: &AppHandle, cursor_hit: bool, holding: bool) {
    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    let state = app.state::<CompanionPointerState>();
    let mut policy = *state.policy.lock().unwrap_or_else(|e| e.into_inner());
    policy.visible = window.is_visible().unwrap_or(false);
    let desired = desired_ignore_cursor_events(policy, cursor_hit, holding);
    if state.ignore_written.swap(desired, Ordering::Relaxed) != desired {
        let _ = window.set_ignore_cursor_events(desired);
    }
}

/// 无光标上下文的 push 路径（开关/层级/显隐）共用的 sync：
/// fail-open 判命中（可交互），轮询 tick 至多 33ms 后按实况纠偏。
fn sync_companion_ignore_cursor_events(app: &AppHandle) {
    sync_companion_ignore_cursor_events_hit(app, true, false);
}

/// 智能穿透轮询单 tick：读光标 → 窗口内逻辑坐标 → 命中判定 → hold 推进 → 单一写点。
///
/// smart 关闭时空转早退（写点由 push 路径负责）；`is_visible` 是每 tick 唯一的
/// dispatcher getter，`cursor_position` 为 OS 直读（无主线程往返），其余输入全部
/// 来自事件缓存。不直接写 `set_ignore_cursor_events` 之外的窗口属性。
fn companion_pointer_tick(app: &AppHandle) {
    let state = app.state::<CompanionPointerState>();
    if !state
        .policy
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .smart_enabled
    {
        return;
    }
    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let Ok(cursor) = app.cursor_position() else {
        return;
    };
    // 几何缓存未就绪（启动极早期首个 tick）→ 跳过，下一 tick 再判。
    let origin = *state.origin.lock().unwrap_or_else(|e| e.into_inner());
    let size = *state.size.lock().unwrap_or_else(|e| e.into_inner());
    let scale = *state.scale.lock().unwrap_or_else(|e| e.into_inner());
    let (Some(origin), Some(size)) = (origin, size) else {
        return;
    };
    let local_x = cursor.x - origin.x;
    let local_y = cursor.y - origin.y;
    // 窗口盒 clamp：光标明确在窗外（±离开阈值的物理量）直接判未命中。防御混合 DPI
    // 显示器下 cursor_position（tao 按主屏 scale 折算）与 outer_position（按所在屏
    // scale）的偏差，避免光标在另一块屏时误判命中。
    let margin = EXIT_MARGIN_PX * scale;
    let in_window = local_x >= -margin
        && local_y >= -margin
        && local_x <= size.width + margin
        && local_y <= size.height + margin;
    let current_ignore = state.ignore_written.load(Ordering::Relaxed);
    let region = state
        .region
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let hit = in_window
        && cursor_hit(
            region.as_deref(),
            local_x / scale,
            local_y / scale,
            current_ignore,
        );
    // hold 推进：窗口刚移动过（拖动中）顺延保护期；右键菜单保护期未过期保持。
    let now = std::time::Instant::now();
    let moved = state
        .last_move_at
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|t| now.duration_since(t) <= DRAG_HOLD);
    let holding = {
        let mut hold = state.hold_until.lock().unwrap_or_else(|e| e.into_inner());
        *hold = next_hold(*hold, now, moved, DRAG_HOLD);
        hold.is_some()
    };
    let mut policy = *state.policy.lock().unwrap_or_else(|e| e.into_inner());
    policy.visible = visible;
    let desired = desired_ignore_cursor_events(policy, hit, holding);
    if state.ignore_written.swap(desired, Ordering::Relaxed) != desired {
        let _ = window.set_ignore_cursor_events(desired);
    }
}

/// 启动智能穿透轮询线程（app 存活期内常驻；smart 关闭时 tick 空转早退；
/// 进程退出随进程终止，无需停止信号）。
fn start_companion_pointer_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(POINTER_TICK);
            companion_pointer_tick(&app);
        }
    });
}

/// 主动初始化几何缓存（origin/size/scale）。
///
/// tao 不保证窗口创建后派发首次 Moved/Resized；不初始化则轮询 tick 因缓存为
/// `None` 永久跳过，智能穿透静默失效。此后增量由 `on_window_event` 维护。
fn init_companion_pointer_geometry(app: &AppHandle) {
    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    let state = app.state::<CompanionPointerState>();
    if let Ok(p) = window.outer_position() {
        *state.origin.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(PhysicalPosition::new(f64::from(p.x), f64::from(p.y)));
    }
    if let Ok(s) = window.outer_size() {
        *state.size.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(PhysicalSize::new(f64::from(s.width), f64::from(s.height)));
    }
    if let Ok(f) = window.scale_factor() {
        *state.scale.lock().unwrap_or_else(|e| e.into_inner()) = f;
    }
}

/// 保存并应用角色窗口**强制穿透**（内部实现，供 command 与原生菜单事件共用）。
///
/// 开启后 companion 窗口对所有鼠标事件透明（拖动/滚轮缩放/右键菜单随之失效），
/// 关闭入口只剩设置页与托盘菜单；优先级最高，覆盖智能穿透的光标判定。
/// 穿透的实际写点统一在 `sync_companion_ignore_cursor_events`（单一权威写点）。
/// 不发事件：穿透不影响角色窗口渲染，其前端无消费者。
fn apply_companion_click_through(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.click_through = Some(enabled);
    settings::save_settings(&settings)?;
    refresh_companion_pointer_policy(app);
    sync_companion_ignore_cursor_events(app);
    rebuild_tray_menu(app);
    Ok(())
}

/// 保存并应用角色窗口智能穿透（内部实现，供 command 与原生菜单事件共用）。
///
/// 开启后按光标位置动态切换穿透：光标落在角色不透明区域上才接收鼠标（决策逻辑见
/// `zapmomo::companion_click_through`）。与强制穿透（`click_through`）叠加时后者优先。
fn apply_companion_smart_click_through(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.smart_click_through = Some(enabled);
    settings::save_settings(&settings)?;
    refresh_companion_pointer_policy(app);
    sync_companion_ignore_cursor_events(app);
    let _ = app.emit("companion-smart-click-through-changed", enabled);
    rebuild_tray_menu(app);
    Ok(())
}

/// 保存角色窗口显示层级并即时应用（内部实现，供 command 与原生菜单事件共用）。
///
/// z-order 由 `apply_companion_layer_platform` 平台实现调整；点穿不再由平台实现
/// 直写（历史 bug：macOS 前置分支硬编码清除穿透，会静默丢掉已开启的手动穿透），
/// 统一收敛到单一写点 `sync_companion_ignore_cursor_events`。
fn apply_companion_layer(app: &AppHandle, layer: CompanionWindowLayer) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.window_layer = Some(layer);
    settings::save_settings(&settings)?;
    apply_companion_layer_platform(app, layer);
    refresh_companion_pointer_policy(app);
    sync_companion_ignore_cursor_events(app);
    let _ = app.emit("companion-layer-changed", layer);
    rebuild_tray_menu(app);
    Ok(())
}

/// 保存并应用角色窗口位置锁定（内部实现，供 command 与原生菜单事件共用）。
///
/// 锁定仅拦截前端拖动（CompanionRoot 的 mousedown → startDragging），滚轮缩放与
/// 右键菜单保留（右键菜单是解锁入口）。与 click_through 不同：拖动拦截在前端
/// CompanionRoot，必须 emit `companion-locked-changed` 实时同步；无平台 API 调用。
fn apply_companion_locked(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.locked = Some(enabled);
    settings::save_settings(&settings)?;
    let _ = app.emit("companion-locked-changed", enabled);
    rebuild_tray_menu(app);
    Ok(())
}

/// 保存并应用角色窗口拖拽模式（内部实现，供 command 调用）。
///
/// modifier 模式仅收紧前端拖动条件（CompanionRoot 的 mousedown → startDragging
/// 需按住 cmd/Ctrl），滚轮缩放与右键菜单不受影响；与 locked 正交（locked 优先，
/// 完全禁止拖动）。拖拽模式不进右键/托盘菜单，无需 rebuild_tray_menu。
fn apply_companion_drag_mode(app: &AppHandle, mode: CompanionDragMode) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    let live2d = settings.live2d.get_or_insert_with(Live2dSettings::default);
    live2d.drag_mode = Some(mode);
    settings::save_settings(&settings)?;
    let _ = app.emit("companion-drag-mode-changed", mode);
    Ok(())
}

// ---------------------------------------------------------------------------
// 表演（BongoCat 兼容模拟表演）运行时
//
// 事件流与 BongoCat `device-changed` 逐字节同构，仅事件由模拟 PerformanceSource
// 产生——**绝不监听真实键鼠**。控制事件（performance-started/stopped）定向
// `companion` 窗口，避免 60-120 msg/s 的鼠标事件灌进设置窗口。
// ---------------------------------------------------------------------------

/// 当前表演状态（`None` = 未表演）。static Mutex 供主线程菜单与异步命令共用。
static PERFORMANCE: Mutex<Option<PerformanceState>> = Mutex::new(None);

/// TTS 热切换写方代际（`set_current_model` 事务臂递增；连续切换防旧覆盖新 + 日志追溯）。
static TTS_SWAP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct PerformanceState {
    /// 每个活动通道一个停止信号（Both 场景有两个 worker）。
    stops: Vec<StopSignal>,
    scene: PerformanceScene,
}

/// 主显示器物理像素活动区域（取不到时回退 1920×1080）。
fn primary_monitor_rect(app: &AppHandle) -> Rect {
    let fallback = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };
    match app.primary_monitor() {
        Ok(Some(m)) => {
            let s = m.size();
            let p = m.position();
            Rect {
                x: p.x as f64,
                y: p.y as f64,
                width: s.width as f64,
                height: s.height as f64,
            }
        }
        _ => fallback,
    }
}

/// 停止当前表演（若有）并发出 `performance-stopped`；不做托盘重建（调用方负责）。
fn stop_performance_inner(app: &AppHandle) {
    let mut guard = PERFORMANCE.lock().unwrap();
    if let Some(state) = guard.take() {
        for stop in &state.stops {
            stop.stop(); // 立即唤醒 worker；被打断的在途事件不发出
        }
        let _ = app.emit_to(
            "companion",
            "performance-stopped",
            serde_json::json!({ "scene": state.scene.as_str() }),
        );
        tracing::info!("已停止表演：{}", state.scene.as_str());
    }
}

/// 启动一个模拟器 worker 线程（发 `device-changed` 到 companion 窗口），
/// 停止信号登记进 `stops`。Both 场景会调用两次（typing + mouse 各一个线程）。
fn spawn_performance_worker(
    mut source: Box<dyn PerformanceSource>,
    app: &AppHandle,
    stops: &mut Vec<StopSignal>,
) {
    let stop = StopSignal::new();
    stops.push(stop.clone());
    let emit_app = app.clone();
    let mut rng = Rng::from_entropy();
    std::thread::spawn(move || {
        let mut emit = |ev: &DeviceEvent| {
            let _ = emit_app.emit_to("companion", "device-changed", ev);
        };
        run_source(source.as_mut(), &mut rng, &stop, &mut emit);
    });
}

/// 启动表演（供 command 与菜单事件共用）。
fn start_performance_impl(app: &AppHandle, scene: PerformanceScene) -> Result<(), String> {
    let props = detect_active_bongocat_props()
        .ok_or_else(|| "当前伙伴不支持表演（需要 BongoCat 格式模型）".to_string())?;

    // 锁内先停旧表演（旧 worker 静默退出、不再发 stopped，避免新旧线程清理竞态）。
    let mut guard = PERFORMANCE.lock().unwrap();
    if let Some(old) = guard.take() {
        for stop in &old.stops {
            stop.stop();
        }
    }

    // 键池来自模型实际贴图键名（无贴图的键不会出现在事件流）。
    let keys: Vec<String> = props.keys.iter().map(|k| k.key.clone()).collect();
    let area = primary_monitor_rect(app);
    let mut stops = Vec::new();

    // 按场景启动一个或多个通道 worker（Both = 键鼠同动，两个线程并发）。
    if scene.has_typing() {
        spawn_performance_worker(
            Box::new(TypingSimulator::new(keys.clone())),
            app,
            &mut stops,
        );
    }
    if scene.has_mouse() {
        spawn_performance_worker(Box::new(MouseSimulator::new(area)), app, &mut stops);
    }

    // 先发 started（消费者先于第一个 device 事件就绪）。带鼠标通道时下发 play_area。
    let started_payload = if scene.has_mouse() {
        serde_json::json!({
            "scene": scene.as_str(),
            "play_area": { "x": area.x, "y": area.y, "width": area.width, "height": area.height },
        })
    } else {
        serde_json::json!({ "scene": scene.as_str() })
    };
    let _ = app.emit_to("companion", "performance-started", &started_payload);

    *guard = Some(PerformanceState { stops, scene });
    tracing::info!("已开始表演：{}", scene.as_str());
    Ok(())
}

/// 开始表演（场景名 "typing" / "mouse"；供设置页/未来 AI 编排调用）。
#[tauri::command]
fn start_performance(app: AppHandle, scene: String) -> Result<(), String> {
    let scene = match scene.as_str() {
        "typing" => PerformanceScene::Typing,
        "mouse" => PerformanceScene::Mouse,
        "both" => PerformanceScene::Both,
        other => return Err(format!("未知表演场景: {other}")),
    };
    start_performance_impl(&app, scene)?;
    rebuild_tray_menu_threadsafe(&app);
    Ok(())
}

/// 停止表演（幂等）。
#[tauri::command]
fn stop_performance(app: AppHandle) -> Result<(), String> {
    stop_performance_inner(&app);
    rebuild_tray_menu_threadsafe(&app);
    Ok(())
}

/// 查询当前是否在表演及场景（dev HMR 重载后前端重同步用）。
#[tauri::command]
fn is_performing() -> Option<String> {
    PERFORMANCE
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.scene.as_str().to_string())
}

/// 主线程停止表演（handle_menu / 菜单切换伙伴 / 隐藏窗口用）。
fn stop_performance_sync(app: &AppHandle) {
    stop_performance_inner(app);
    rebuild_tray_menu(app);
}

/// 读取持久化的角色窗口显示层级（缺省置顶）。
fn current_companion_layer() -> CompanionWindowLayer {
    settings::load_settings()
        .ok()
        .flatten()
        .and_then(|s| s.live2d)
        .and_then(|l| l.window_layer)
        .unwrap_or_default()
}

/// 菜单切换当前伙伴：`set_active` 持久化 → `reconcile_active` 同步桌宠 → 重建托盘勾选态。
///
/// 与 `apply_companion_*` 家族对齐（供 handle_menu 主线程调用），但菜单上下文没有 UI
/// 可承载错误，故不返回 `Result`，失败只 `tracing::warn`。`set_active` 内部的
/// `validate_managed_model` 深校验是毫秒级小 IO（读 model3.json + 查文件存在性），
/// 与 handle_menu 里现有同步 load/save settings 同量级。
fn apply_active_companion(app: &AppHandle, id: &str) {
    // 切换伙伴前先停表演（stopped 先于 model-changed 发出，同线程 emit 保序）。
    stop_performance_inner(app);
    match zapmomo::companion::set_active(id) {
        Ok(lib) => {
            let active = zapmomo::companion::active_model(&lib);
            if let Err(e) = reconcile_active(app, active) {
                tracing::warn!(
                    "菜单切换伙伴后同步桌宠失败（active 已持久化，重启或打开伙伴页自愈）: {e}"
                );
            }
            rebuild_tray_menu(app);
        }
        Err(e) => tracing::warn!("菜单切换伙伴失败（active 未变更）: {e}"),
    }
}

/// macOS 置底层级：-1 = 普通应用窗口(0)之下、桌面图标(-2147483623)与壁纸(-2147483624)之上。
/// 模型作为背景装饰浮在桌面上：绝不被壁纸图片盖住（壁纸级同层不可靠），桌面图标仍可点击。
#[cfg(target_os = "macos")]
const MACOS_COMPANION_BACK_LEVEL: i64 = -1;

/// 气泡/输入条面板的 macOS 层级：Floating(4) 之上 1 级，建窗时常置、不随角色层级切换。
/// 角色前置 = Floating(4)、置底 = -1（见 `apply_companion_layer_platform`），层级 5
/// 保证聊天气泡与文字输入条恒高于角色，角色任何层级下都不会遮挡它们。
/// （Tauri `always_on_top` 只设 NSFloatingWindowLevel=3，低于角色前置的 4，
/// 曾导致角色前置时角色反而盖住气泡/输入条、无法操作。）
#[cfg(target_os = "macos")]
const MACOS_OVERLAY_PANEL_LEVEL: i64 = 5;

/// 按层级即时调整角色窗口的 z-order（平台相关；启动与运行时共用）。
///
/// 只管 z-order：点穿统一由 `sync_companion_ignore_cursor_events` 单一写点处理
/// （tauri `set_ignore_cursor_events` 与面板的 setIgnoresMouseEvents 同 selector，
/// 不必在此直写；直写曾静默清掉已开启的手动穿透）。
/// macOS 走 NSPanel set_level，不混用 tauri 的 set_always_on_top（它会先把 level
/// 置回 0，与壁纸级冲突）。注意：不要在这里切换 NSPanel 的 floating 属性——
/// 运行时切换会破坏存活 WKWebView 的渲染（模型消失）。
#[cfg(target_os = "macos")]
fn apply_companion_layer_platform(app: &AppHandle, layer: CompanionWindowLayer) {
    use tauri_nspanel::{ManagerExt, PanelLevel};

    let Ok(panel) = app.get_webview_panel("companion") else {
        return;
    };
    match layer {
        // 浮层（4），与建窗时的 always_on_top 行为一致。
        CompanionWindowLayer::Front => panel.set_level(i64::from(PanelLevel::Floating)),
        // 图标之上：-1 在普通窗口之下、桌面图标与壁纸之上。不切换 NSPanel floating 属性——
        // z-order 完全由 set_level 控制（浮层属性不参与层级），运行时切换 floating 会破坏存活 WebView 渲染。
        CompanionWindowLayer::Back => panel.set_level(MACOS_COMPANION_BACK_LEVEL),
    }
}

/// 把气泡/输入条窗口重断到 TOPMOST band 顶部（Windows）。
///
/// topmost band 内无层级之分，Z 序由插入顺序决定：角色置顶后再断一次气泡/输入条
/// 的 always_on_top（SetWindowPos HWND_TOPMOST 即移到 band 顶），保证二者恒在角色
/// 之上；角色置底时已退出 topmost band，本调用无副作用（仅重复置顶）。
#[cfg(windows)]
fn raise_overlay_windows(app: &AppHandle) {
    for label in ["bubble", "chatbox"] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_always_on_top(true);
        }
    }
}

/// 按层级即时调整角色窗口的 z-order（平台相关；启动与运行时共用）。
///
/// 只管 z-order：点穿（含置底 WS_EX_TRANSPARENT）统一由
/// `sync_companion_ignore_cursor_events` 单一写点处理，避免与手动/智能穿透双写竞争。
#[cfg(windows)]
fn apply_companion_layer_platform(app: &AppHandle, layer: CompanionWindowLayer) {
    use windows::Win32::UI::WindowsAndMessaging::{
        HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };

    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    let front = matches!(layer, CompanionWindowLayer::Front);
    let _ = window.set_always_on_top(front); // 置顶 = WS_EX_TOPMOST；置底 = 清除
    if !front {
        let Ok(hwnd) = window.hwnd() else { return };
        unsafe {
            // 一次性把窗口沉到 Z 序底部；hide/show 后由 toggle_companion_window 按 layer 重放。
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    raise_overlay_windows(app);
}

/// 按层级即时调整角色窗口的 z-order 与鼠标穿透（平台相关；启动与运行时共用）。
///
/// Linux：置底能力暂未实现（X11 `_NET_WM_STATE_BELOW` 与 Wayland 支持为 future work），
/// 仅保证可编译与持久化，运行时不做窗口层级调整。
#[cfg(target_os = "linux")]
fn apply_companion_layer_platform(_app: &AppHandle, layer: CompanionWindowLayer) {
    tracing::debug!("角色窗口层级切换在 Linux 上暂不支持置底（{layer:?}）");
}

/// 把原生菜单项 id 解析为缩放比例。
fn scale_from_id(id: &str) -> Option<f64> {
    match id {
        "scale_25" => Some(0.25),
        "scale_50" => Some(0.5),
        "scale_70" => Some(0.7),
        "scale_100" => Some(1.0),
        "scale_150" => Some(1.5),
        "scale_200" => Some(2.0),
        _ => None,
    }
}

/// 透明度合法范围（含边界）。
const OPACITY_MIN: f64 = 0.2;
const OPACITY_MAX: f64 = 1.0;

/// 把透明度 clamp 到 `[OPACITY_MIN, OPACITY_MAX]`。
fn clamp_opacity(v: f64) -> f64 {
    v.clamp(OPACITY_MIN, OPACITY_MAX)
}

/// 把原生菜单项 id 解析为透明度。
fn opacity_from_id(id: &str) -> Option<f64> {
    match id {
        "opacity_100" => Some(1.0),
        "opacity_80" => Some(0.8),
        "opacity_60" => Some(0.6),
        "opacity_40" => Some(0.4),
        "opacity_20" => Some(0.2),
        _ => None,
    }
}

/// 把原生菜单项 id 解析为显示层级。
fn layer_from_id(id: &str) -> Option<CompanionWindowLayer> {
    match id {
        "layer_front" => Some(CompanionWindowLayer::Front),
        "layer_back" => Some(CompanionWindowLayer::Back),
        _ => None,
    }
}

/// 「切换伙伴」菜单项 id 前缀：`companion_set_<伙伴id>`。
const COMPANION_SET_PREFIX: &str = "companion_set_";

/// 把原生菜单项 id 解析为伙伴 id（`companion_set_<id>` → `<id>`）。
///
/// 空后缀与其余命名空间（`scale_*` / `open_settings` / 占位项 `no_companions` 等）
/// 返回 `None`。
fn companion_id_from_menu_id(id: &str) -> Option<&str> {
    id.strip_prefix(COMPANION_SET_PREFIX)
        .filter(|rest| !rest.is_empty())
}

/// 「切换伙伴」菜单项描述（纯数据，便于单测；由构建函数转成 CheckMenuItem）。
struct CompanionMenuEntry {
    id: String,
    label: String,
    /// 无效伙伴（托管目录/清单被外部删除）禁用，避免点击后才在深层校验失败。
    enabled: bool,
    /// 当前 active 打勾。
    checked: bool,
}

/// 由库快照生成「切换伙伴」菜单项描述（不触碰菜单 API，纯函数）。
///
/// `valid` 用 `quick_valid`（仅探测目录/清单存在性，毫秒级）；无效项 label 追加
/// 「（不可用）」并禁用，仍保留在列表中告知用户该伙伴待重新导入。
fn companion_menu_entries(
    models: &[zapmomo::companion::CompanionModel],
    active_id: Option<&str>,
) -> Vec<CompanionMenuEntry> {
    models
        .iter()
        .map(|m| {
            let valid = zapmomo::companion::quick_valid(m);
            CompanionMenuEntry {
                id: format!("{COMPANION_SET_PREFIX}{}", m.id),
                label: if valid {
                    m.name.clone()
                } else {
                    format!("{}（不可用）", m.name)
                },
                enabled: valid,
                checked: active_id == Some(m.id.as_str()),
            }
        })
        .collect()
}

/// 设置并持久化角色窗口缩放比例（1.0 = 100%）。
///
/// 由设置面板（或角色窗口自身）调用：写入 `~/.zapmomo/settings.toml` 的
/// `[live2d.window_scale]` 段，并通过 `companion-scale-changed` 事件通知角色窗口
/// （角色窗口持有真实模型宽高比，负责把比例换算成绝对尺寸并 `setSize`）。
#[tauri::command]
fn set_companion_scale(app: AppHandle, scale: f64) -> Result<(), String> {
    apply_companion_scale(&app, scale)
}

/// 设置并持久化角色窗口透明度（1.0 = 不透明，范围 0.2~1.0）。
///
/// 由设置面板调用：写入 `~/.zapmomo/settings.toml` 的 `[live2d].window_opacity`，
/// 并通过 `companion-opacity-changed` 事件通知角色窗口更新渲染层 opacity。
#[tauri::command]
fn set_companion_opacity(app: AppHandle, opacity: f64) -> Result<(), String> {
    apply_companion_opacity(&app, opacity)
}

/// 设置并持久化角色窗口点击穿透（true = 鼠标事件全部穿透到身后窗口）。
///
/// 由设置面板或原生菜单调用：写入 `~/.zapmomo/settings.toml` 的 `[live2d].click_through`，
/// 并立即对 companion 窗口生效。开启后窗口收不到任何鼠标事件（拖动/缩放/右键菜单失效），
/// 只能从设置页或托盘菜单关闭。语义为**强制穿透**：优先级最高，覆盖智能穿透。
#[tauri::command]
fn set_companion_click_through(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_companion_click_through(&app, enabled)
}

/// 设置并持久化角色窗口智能穿透（光标在角色不透明区域上才接收鼠标，其余穿透）。
///
/// 由设置面板或原生菜单调用：写入 `~/.zapmomo/settings.toml` 的
/// `[live2d].smart_click_through`，并经 `companion-smart-click-through-changed`
/// 事件广播；与强制穿透叠加时后者优先。
#[tauri::command]
fn set_companion_smart_click_through(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_companion_smart_click_through(&app, enabled)
}

/// 更新角色窗口的智能穿透命中区域（前端在模型加载/布局变化后上报，去抖 150ms）。
///
/// `rects` 为窗口内逻辑像素矩形集；`[]` 表示清屏（明确穿透），不调用表示未就绪
/// （fail-open 判可交互）。NaN/负值坐标防御性 clamp 为 0。不在此处 sync：
/// push 路径无光标上下文，交给轮询下一 tick（≤33ms）按实况判定，避免假命中翻转。
#[tauri::command]
fn set_companion_hit_region(app: AppHandle, rects: Vec<HitRect>) -> Result<(), String> {
    let cleaned: Vec<HitRect> = rects
        .into_iter()
        .map(|r| HitRect {
            x: if r.x.is_finite() && r.x > 0.0 {
                r.x
            } else {
                0.0
            },
            y: if r.y.is_finite() && r.y > 0.0 {
                r.y
            } else {
                0.0
            },
            width: if r.width.is_finite() && r.width > 0.0 {
                r.width
            } else {
                0.0
            },
            height: if r.height.is_finite() && r.height > 0.0 {
                r.height
            } else {
                0.0
            },
        })
        .collect();
    let state = app.state::<CompanionPointerState>();
    *state.region.lock().unwrap_or_else(|e| e.into_inner()) = Some(cleaned);
    Ok(())
}

/// 设置并持久化角色窗口显示层级（置顶/置底），并即时应用到角色窗口。
///
/// 由设置面板调用：写入 `~/.zapmomo/settings.toml` 的 `[live2d].window_layer`，
/// 并通过 `companion-layer-changed` 事件通知角色窗口；z-order 与点穿由
/// `apply_companion_layer_platform` 平台实现即时调整。
#[tauri::command]
fn set_companion_layer(app: AppHandle, layer: CompanionWindowLayer) -> Result<(), String> {
    apply_companion_layer(&app, layer)
}

/// 设置并持久化角色窗口位置锁定（true = 禁止拖动窗口）。
///
/// 由设置面板或原生菜单调用：写入 `~/.zapmomo/settings.toml` 的 `[live2d].locked`，
/// 并通过 `companion-locked-changed` 事件通知角色窗口实时拦截拖动；滚轮缩放与
/// 右键菜单不受影响（右键菜单是解锁入口之一）。
#[tauri::command]
fn set_companion_locked(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_companion_locked(&app, enabled)
}

/// 设置并持久化角色窗口拖拽模式（modifier = 需按住 cmd/Ctrl 才能拖动）。
///
/// 由设置面板调用：写入 `~/.zapmomo/settings.toml` 的 `[live2d].drag_mode`，
/// 并通过 `companion-drag-mode-changed` 事件通知角色窗口实时生效。
#[tauri::command]
fn set_companion_drag_mode(app: AppHandle, mode: CompanionDragMode) -> Result<(), String> {
    apply_companion_drag_mode(&app, mode)
}

/// 读取是否在 macOS Dock / Cmd+Tab 中隐藏应用图标（Accessory 模式）。
#[tauri::command]
fn get_hide_dock_icon() -> Result<bool, String> {
    Ok(settings::load_settings()?
        .unwrap_or_default()
        .hide_dock_icon)
}

/// 设置并持久化是否在 macOS Dock / Cmd+Tab 中隐藏应用图标，并立即生效。
///
/// 写入 `~/.zapmomo/settings.toml` 顶层的 `hide_dock_icon` 字段；非 macOS 仅持久化，
/// 不改变激活策略（该设置仅对 macOS 的 Dock / Cmd+Tab 有意义）。
///
/// `app` 仅在 macOS 上用于切换 ActivationPolicy，其它平台未使用，故非 macOS 允许未使用变量。
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
#[tauri::command]
fn set_hide_dock_icon(app: AppHandle, hide: bool) -> Result<(), String> {
    let mut settings = settings::load_settings()?.unwrap_or_default();
    settings.hide_dock_icon = hide;
    settings::save_settings(&settings)?;
    #[cfg(target_os = "macos")]
    {
        let policy = if hide {
            tauri::ActivationPolicy::Accessory
        } else {
            tauri::ActivationPolicy::Regular
        };
        app.set_activation_policy(policy)
            .map_err(|e| format!("切换激活策略失败: {e}"))?;
    }
    Ok(())
}

/// 自启动拉起检测：命令行精确携带 `--autostart`（开启自启动时由插件附加到
/// 系统启动项）。前缀/去杠变体（`--autostart-x`、`autostart`）不命中。
fn is_launched_by_autostart<I>(args: I) -> bool
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    args.into_iter().any(|a| a.as_ref() == "--autostart")
}

/// 自启动菜单项的 (id, 文案)：按当前状态显示相反动作，点击应用固定值（幂等）。
fn autostart_item_labels(enabled: bool) -> (&'static str, &'static str) {
    if enabled {
        ("disable_autostart", "关闭开机自启动")
    } else {
        ("enable_autostart", "开机自启动")
    }
}

/// 读当前开机自启动状态。
///
/// 注意：与 hide_dock_icon 等落盘开关不同，自启动是系统级注册（注册表 Run 键 /
/// LaunchAgent / XDG .desktop），不随应用退出消失，用户可在系统设置外部增删；
/// 系统状态即唯一真值，不在 settings.toml 落盘，读取直查插件（单次本地文件 /
/// 注册表检查，调用点仅 command 与托盘重建，无需缓存）。
fn current_autostart_enabled(app: &AppHandle) -> bool {
    // 函数内 use：避免与 tauri-nspanel 的同名 ManagerExt trait 冲突。
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

/// 设置并生效开机自启动（内部实现，供 command 与原生菜单事件共用）。
///
/// 注册/移除系统启动项后经 `autostart-changed` 事件通知设置页刷新开关，并重建
/// 托盘菜单翻转「开机自启动/关闭开机自启动」文案。
fn apply_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled {
        app.autolaunch()
            .enable()
            .map_err(|e| format!("开启开机自启动失败（写入系统启动项被拒）：{e}"))?;
    } else {
        app.autolaunch()
            .disable()
            .map_err(|e| format!("关闭开机自启动失败（移除系统启动项被拒）：{e}"))?;
    }
    let _ = app.emit("autostart-changed", enabled);
    rebuild_tray_menu(app);
    Ok(())
}

/// 读取是否开启开机自启动（系统注册状态直读）。
#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    Ok(current_autostart_enabled(&app))
}

/// 设置开机自启动（设置页经此 command 间接操作插件，不暴露权限给前端）。
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    apply_autostart(&app, enabled)
}

/// 处理应用菜单、托盘菜单与角色窗口右键菜单事件。
fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "show_settings" | "open_settings" => show_settings_window(app),
        "toggle_companion" => toggle_companion_window(app),
        "hide_companion" => hide_companion_window(app),
        // 文字输入条：勾选态即目标态（点击时 muda 已自动翻转 checked，这里取反读到的
        // 配置值等价于「切到另一态」；set_chatbox_visible 内 rebuild 刷新勾选）
        "toggle_chatbox" => {
            let visible = !current_chatbox_visible();
            // 菜单勾选是用户显式打开输入条：显示后直接聚焦可打字
            set_chatbox_visible(app, visible, true);
        }
        // 退出/重启前清理 dsh 桥发现文件：桥线程随进程终止无 epilogue，
        // 不清理会残留指向死端口的文件（崩溃残留仍由下次启动兜底清理）
        "restart" => {
            zapmomo::dsh::remove_discovery();
            zapmomo::audiocpp::server::shutdown_blocking();
            app.request_restart();
        }
        "quit" => {
            zapmomo::dsh::remove_discovery();
            zapmomo::audiocpp::server::shutdown_blocking();
            app.exit(0);
        }
        // 点击穿透：菜单按当前状态显示「启用/禁用」项，点击后应用固定值（幂等，
        // 不受菜单事件重复派发影响）。
        "enable_click_through" => {
            let _ = apply_companion_click_through(app, true);
        }
        "disable_click_through" => {
            let _ = apply_companion_click_through(app, false);
        }
        // 智能穿透：与强制穿透同理按当前状态显示相反动作项，点击应用固定值（幂等）。
        "enable_smart_click_through" => {
            let _ = apply_companion_smart_click_through(app, true);
        }
        "disable_smart_click_through" => {
            let _ = apply_companion_smart_click_through(app, false);
        }
        // 位置锁定：与点击穿透同理按当前状态显示「锁定/解锁」项，点击应用固定值（幂等）。
        "enable_lock" => {
            let _ = apply_companion_locked(app, true);
        }
        "disable_lock" => {
            let _ = apply_companion_locked(app, false);
        }
        // 开机自启动：同为按当前状态显示相反动作的菜单项，点击应用固定值（幂等）。
        "enable_autostart" => {
            let _ = apply_autostart(app, true);
        }
        "disable_autostart" => {
            let _ = apply_autostart(app, false);
        }
        // 表演动作（模拟键鼠，与真实输入无关）。
        "perform_typing" => {
            if let Err(e) = start_performance_impl(app, PerformanceScene::Typing) {
                tracing::warn!("启动敲键盘表演失败: {e}");
            } else {
                rebuild_tray_menu(app);
            }
        }
        "perform_mouse" => {
            if let Err(e) = start_performance_impl(app, PerformanceScene::Mouse) {
                tracing::warn!("启动玩鼠标表演失败: {e}");
            } else {
                rebuild_tray_menu(app);
            }
        }
        "perform_both" => {
            if let Err(e) = start_performance_impl(app, PerformanceScene::Both) {
                tracing::warn!("启动键鼠同动表演失败: {e}");
            } else {
                rebuild_tray_menu(app);
            }
        }
        "perform_stop" => stop_performance_sync(app),
        _ => {
            if let Some(scale) = scale_from_id(id) {
                let _ = apply_companion_scale(app, scale);
            } else if let Some(opacity) = opacity_from_id(id) {
                let _ = apply_companion_opacity(app, opacity);
            } else if let Some(layer) = layer_from_id(id) {
                let _ = apply_companion_layer(app, layer);
            } else if let Some(companion_id) = companion_id_from_menu_id(id) {
                apply_active_companion(app, companion_id);
            }
        }
    }
}

/// 当前 active 伙伴是否为 BongoCat 格式（探测式，毫秒级）。
fn current_companion_is_bongocat() -> bool {
    detect_active_bongocat_props().is_some()
}

/// 构建「表演」子菜单（敲键盘 / 玩鼠标 / 键鼠同动 / 停止表演）。
///
/// 非 BongoCat 伙伴或表演进行中时前三项禁用；未表演时 stop 禁用。
/// 右键菜单每次弹出重建，勾选/可用态天然最新；托盘菜单需在 start/stop 后重建。
fn build_performance_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let bongo = current_companion_is_bongocat();
    let performing = PERFORMANCE.lock().unwrap().is_some();
    let typing = MenuItem::with_id(
        app,
        "perform_typing",
        "表演：敲键盘",
        bongo && !performing,
        None::<&str>,
    )?;
    let mouse = MenuItem::with_id(
        app,
        "perform_mouse",
        "表演：玩鼠标",
        bongo && !performing,
        None::<&str>,
    )?;
    let both = MenuItem::with_id(
        app,
        "perform_both",
        "表演：键鼠同动",
        bongo && !performing,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(app, "perform_stop", "停止表演", performing, None::<&str>)?;
    Submenu::with_items(app, "表演", true, &[&typing, &mouse, &both, &stop])
}

/// 构建角色窗口的右键菜单（切换伙伴/表演/窗口尺寸/透明度/显示层级子菜单 + 打开设置 / 隐藏角色 / 重启 / 退出）。
fn build_companion_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let companion_submenu = build_companion_switch_submenu(app)?;
    let performance_submenu = build_performance_submenu(app)?;
    let (scale_submenu, opacity_submenu) = build_metric_submenus(app)?;
    let layer_submenu = build_layer_submenu(app)?;
    let click_through = build_click_through_item(app)?;
    let smart_click_through = build_smart_click_through_item(app)?;
    let locked = build_locked_item(app)?;
    let chatbox = build_chatbox_item(app)?;
    let open_settings = MenuItem::with_id(app, "open_settings", "打开设置", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide_companion", "隐藏角色", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &companion_submenu,
            &performance_submenu,
            &scale_submenu,
            &opacity_submenu,
            &layer_submenu,
            &click_through,
            &smart_click_through,
            &locked,
            &chatbox,
            &open_settings,
            &hide,
            &restart,
            &quit,
        ],
    )
}

/// 托盘 id（档位变化后 `tray_by_id` 定位托盘并重建菜单）。
const TRAY_ID: &str = "zapmomo-tray";

/// 构建托盘菜单：显示/隐藏角色、切换伙伴、窗口尺寸/透明度/显示层级、打开设置、重启、退出。
fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let companion_submenu = build_companion_switch_submenu(app)?;
    let performance_submenu = build_performance_submenu(app)?;
    let (tray_scale, tray_opacity) = build_metric_submenus(app)?;
    let tray_layer = build_layer_submenu(app)?;
    let click_through = build_click_through_item(app)?;
    let smart_click_through = build_smart_click_through_item(app)?;
    let locked = build_locked_item(app)?;
    let autostart = build_autostart_item(app)?;
    let chatbox = build_chatbox_item(app)?;
    let toggle_companion =
        MenuItem::with_id(app, "toggle_companion", "显示/隐藏角色", true, None::<&str>)?;
    let open_settings = MenuItem::with_id(app, "open_settings", "打开设置", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    Menu::with_items(
        app,
        &[
            &toggle_companion,
            &companion_submenu,
            &performance_submenu,
            &tray_scale,
            &tray_opacity,
            &tray_layer,
            &click_through,
            &smart_click_through,
            &locked,
            &chatbox,
            &autostart,
            &open_settings,
            &restart,
            &quit,
        ],
    )
}

/// 档位（尺寸/透明度）变化后重建托盘菜单，刷新勾选态。
///
/// 托盘菜单只在启动时构建一次，勾选态是当时的快照；不重建会出现旧档位残留打勾
/// （新档位被点击时自动勾上，快照里的旧档位没人取消）。
fn rebuild_tray_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Ok(menu) = build_tray_menu(app)
    {
        let _ = tray.set_menu(Some(menu));
    }
}

/// 从任意线程安全重建托盘菜单（muda/NSMenu 操作须在主线程）。
///
/// 异步命令（import/set_active/rename/remove_companion）跑在 tokio 运行时线程，
/// 不能直接 `set_menu`；主线程路径（handle_menu / apply_companion_*）继续直接调用
/// `rebuild_tray_menu`，不必多一次线程跳转。应用退出中 `run_on_main_thread` 可能
/// 失败，忽略（与 `rebuild_tray_menu` 容忍托盘缺失同一策略）。
fn rebuild_tray_menu_threadsafe(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray_menu(&handle));
}

/// 读当前窗口缩放与透明度（缩放：伙伴私有 layout 优先、全局兜底、缺省 1.0；透明度全局，缺省 1.0）。
fn current_companion_metrics() -> (f64, f64) {
    let layout = zapmomo::companion::active_layout();
    match settings::load_settings() {
        Ok(Some(s)) => (
            resolve_scale(s.live2d.as_ref(), layout.as_ref()),
            s.live2d
                .as_ref()
                .and_then(|l| l.window_opacity)
                .unwrap_or(1.0),
        ),
        _ => (resolve_scale(None, layout.as_ref()), 1.0),
    }
}

/// 解析点击穿透开关：缺省（未配置 / 旧版配置）视为关闭。
fn resolve_click_through(live2d: Option<&Live2dSettings>) -> bool {
    live2d.and_then(|l| l.click_through).unwrap_or(false)
}

/// 解析有效缩放：active 伙伴私有 layout 优先，全局 `[live2d].window_scale` 兜底，缺省 1.0。
fn resolve_scale(
    live2d: Option<&Live2dSettings>,
    layout: Option<&zapmomo::companion::CompanionLayout>,
) -> f64 {
    layout
        .and_then(|l| l.scale)
        .or_else(|| live2d.and_then(|l| l.window_scale))
        .unwrap_or(1.0)
}

/// 解析有效窗口位置：active 伙伴私有 layout 优先，全局 `[live2d].window_position` 兜底。
fn resolve_position(
    live2d: Option<&Live2dSettings>,
    layout: Option<&zapmomo::companion::CompanionLayout>,
) -> Option<CompanionWindowPosition> {
    layout
        .and_then(|l| l.position.clone())
        .or_else(|| live2d.and_then(|l| l.window_position.clone()))
}

/// 当前 active 伙伴 id（无 active / 读库失败 → None）。
fn active_companion_id() -> Option<String> {
    let lib = zapmomo::companion::load_library_fast().ok()?;
    zapmomo::companion::active_model(&lib).map(|m| m.id.clone())
}

/// 读当前点击穿透开关（读失败或缺省回退 false）。
fn current_companion_click_through() -> bool {
    match settings::load_settings() {
        Ok(Some(s)) => resolve_click_through(s.live2d.as_ref()),
        _ => false,
    }
}

/// 读当前智能穿透开关（读失败或缺省回退 true——智能穿透是新默认行为）。
fn current_companion_smart_click_through() -> bool {
    match settings::load_settings() {
        Ok(Some(s)) => resolve_smart_click_through(s.live2d.as_ref()),
        _ => true,
    }
}

/// 解析文字输入条可见性（未配置 → 默认显示）。
///
/// 「缺省 = 显示」承载首次启动体验：角色出现时输入条一同出现。用户一旦显式
/// 关闭（托盘/右键取消勾选 / Esc），`visible` 持久化为 false，之后启动均尊重。
fn resolve_chatbox_visible(chatbox: Option<&ChatboxSettings>) -> bool {
    chatbox.and_then(|c| c.visible).unwrap_or(true)
}

/// 读当前文字输入条可见性（读失败/无文件视为未配置，与启动缺省一致：显示）。
fn current_chatbox_visible() -> bool {
    match settings::load_settings() {
        Ok(Some(s)) => resolve_chatbox_visible(s.chatbox.as_ref()),
        _ => true,
    }
}

/// 「文字输入条」勾选项（勾选 = 输入条窗口显示中）。托盘与角色右键菜单共用。
fn build_chatbox_item(app: &AppHandle) -> tauri::Result<CheckMenuItem<tauri::Wry>> {
    CheckMenuItem::with_id(
        app,
        "toggle_chatbox",
        "文字输入条",
        true,
        current_chatbox_visible(),
        None::<&str>,
    )
}

/// 解析位置锁定开关：缺省（未配置 / 旧版配置）视为关闭。
fn resolve_locked(live2d: Option<&Live2dSettings>) -> bool {
    live2d.and_then(|l| l.locked).unwrap_or(false)
}

/// 读当前位置锁定开关（读失败或缺省回退 false）。
fn current_companion_locked() -> bool {
    match settings::load_settings() {
        Ok(Some(s)) => resolve_locked(s.live2d.as_ref()),
        _ => false,
    }
}

/// 构建「强制穿透」菜单项（角色右键菜单与托盘菜单共用）。
///
/// 按当前状态显示「启用强制穿透」或「禁用强制穿透」的普通菜单项，点击后
/// `handle_menu` 应用固定值（不取反）。不用 CheckMenuItem：其 checked 自动切换
/// 与取反逻辑叠加，一次点击可能触发正反两个 apply（如 15:21 日志所示），
/// 净效果为零，表现为「点击无效」。
fn build_click_through_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let enabled = current_companion_click_through();
    MenuItem::with_id(
        app,
        if enabled {
            "disable_click_through"
        } else {
            "enable_click_through"
        },
        if enabled {
            "禁用强制穿透"
        } else {
            "启用强制穿透"
        },
        true,
        None::<&str>,
    )
}

/// 构建「智能穿透」菜单项（角色右键菜单与托盘菜单共用；不用 CheckMenuItem 的
/// 理由同 build_click_through_item）。
fn build_smart_click_through_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let enabled = current_companion_smart_click_through();
    MenuItem::with_id(
        app,
        if enabled {
            "disable_smart_click_through"
        } else {
            "enable_smart_click_through"
        },
        if enabled {
            "禁用智能穿透"
        } else {
            "启用智能穿透"
        },
        true,
        None::<&str>,
    )
}

/// 构建「锁定角色」菜单项（角色右键菜单与托盘菜单共用）。
///
/// 与 build_click_through_item 同理不用 CheckMenuItem：其 checked 自动切换与取反
/// 逻辑叠加，一次点击可能触发正反两个 apply，净效果为零（表现为「点击无效」）。
fn build_locked_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let enabled = current_companion_locked();
    MenuItem::with_id(
        app,
        if enabled {
            "disable_lock"
        } else {
            "enable_lock"
        },
        if enabled {
            "解锁角色"
        } else {
            "锁定角色"
        },
        true,
        None::<&str>,
    )
}

/// 构建「开机自启动」菜单项（仅托盘菜单；右键菜单不加——低频设置项，设置页与
/// 托盘两入口已足够）。
///
/// 与 build_locked_item 同理不用 CheckMenuItem：其 checked 自动切换与取反逻辑
/// 叠加，一次点击可能触发正反两个 apply，净效果为零（表现为「点击无效」）。
fn build_autostart_item(app: &AppHandle) -> tauri::Result<MenuItem<tauri::Wry>> {
    let (id, label) = autostart_item_labels(current_autostart_enabled(app));
    MenuItem::with_id(app, id, label, true, None::<&str>)
}

/// 构建「显示层级」子菜单（角色右键菜单与托盘菜单共用）。
///
/// 用 `CheckMenuItem`：构建时读当前 settings 的层级，命中的项打勾。
/// 切换后 `apply_companion_layer` 会 `rebuild_tray_menu` 刷新勾选态；
/// 角色右键菜单每次弹出都重建，天然最新。
fn build_layer_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let cur = current_companion_layer();
    let front = CheckMenuItem::with_id(
        app,
        "layer_front",
        "置顶",
        true,
        cur == CompanionWindowLayer::Front,
        None::<&str>,
    )?;
    let back = CheckMenuItem::with_id(
        app,
        "layer_back",
        "置底",
        true,
        cur == CompanionWindowLayer::Back,
        None::<&str>,
    )?;
    Submenu::with_items(app, "显示层级", true, &[&front, &back])
}

/// 构建「窗口尺寸」「透明度」两个档位子菜单（角色右键菜单与托盘菜单共用）。
///
/// 档位用 `CheckMenuItem`：构建时读当前 settings，命中的档位打勾。
fn build_metric_submenus(
    app: &AppHandle,
) -> tauri::Result<(Submenu<tauri::Wry>, Submenu<tauri::Wry>)> {
    let (cur_scale, cur_opacity) = current_companion_metrics();
    let mk_item = |id: &str, label: &str, cur: f64, v: f64| {
        CheckMenuItem::with_id(app, id, label, true, v == cur, None::<&str>)
    };
    let s25 = mk_item("scale_25", "25%", cur_scale, 0.25)?;
    let s50 = mk_item("scale_50", "50%", cur_scale, 0.5)?;
    let s70 = mk_item("scale_70", "70%", cur_scale, 0.7)?;
    let s100 = mk_item("scale_100", "100%", cur_scale, 1.0)?;
    let s150 = mk_item("scale_150", "150%", cur_scale, 1.5)?;
    let s200 = mk_item("scale_200", "200%", cur_scale, 2.0)?;
    let o20 = mk_item("opacity_20", "20%", cur_opacity, 0.2)?;
    let o40 = mk_item("opacity_40", "40%", cur_opacity, 0.4)?;
    let o60 = mk_item("opacity_60", "60%", cur_opacity, 0.6)?;
    let o80 = mk_item("opacity_80", "80%", cur_opacity, 0.8)?;
    let o100 = mk_item("opacity_100", "100%", cur_opacity, 1.0)?;
    let scale_menu = Submenu::with_items(
        app,
        "窗口尺寸",
        true,
        &[&s25, &s50, &s70, &s100, &s150, &s200],
    )?;
    // 档位顺序与「窗口尺寸」一致：从小到大。
    let opacity_menu = Submenu::with_items(app, "透明度", true, &[&o20, &o40, &o60, &o80, &o100])?;
    Ok((scale_menu, opacity_menu))
}

/// 构建「切换伙伴」子菜单（角色右键菜单与托盘菜单共用）。
///
/// - 空库 / 读库失败：降级为单个禁用项「暂无伙伴」（id 不带 `companion_set_`
///   前缀，永不进入 handle_menu 分支）。
/// - 当前 active 用 `CheckMenuItem` 打勾：右键菜单每次弹出重建、托盘在切换后
///   `rebuild_tray_menu`，勾选态始终最新。
fn build_companion_switch_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let lib = match zapmomo::companion::load_library_fast() {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!("读取伙伴库失败（切换伙伴子菜单降级为占位项）: {e}");
            return build_empty_companion_submenu(app);
        }
    };
    if lib.models.is_empty() {
        return build_empty_companion_submenu(app);
    }
    let entries = companion_menu_entries(&lib.models, lib.active_model_id.as_deref());
    let items: Vec<CheckMenuItem<tauri::Wry>> = entries
        .iter()
        .map(|e| {
            CheckMenuItem::with_id(
                app,
                e.id.clone(),
                e.label.as_str(),
                e.enabled,
                e.checked,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<_>>()?;
    // with_items 收 &[&dyn IsMenuItem]：collect 的 FromIterator 不做逐元素 unsize
    // 强转，须显式 as 成 trait 对象再收集。
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items
        .iter()
        .map(|i| i as &dyn IsMenuItem<tauri::Wry>)
        .collect();
    Submenu::with_items(app, "切换伙伴", true, &refs)
}

/// 空库占位子菜单（单个禁用项；id 无 `companion_set_` 前缀，不会被 handle_menu 处理）。
fn build_empty_companion_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let placeholder = MenuItem::with_id(app, "no_companions", "暂无伙伴", false, None::<&str>)?;
    Submenu::with_items(app, "切换伙伴", true, &[&placeholder])
}

/// 弹出角色窗口右键菜单（由前端在右键时调用，坐标相对窗口左上角，逻辑像素）。
#[tauri::command]
fn show_companion_menu(app: AppHandle, x: f64, y: f64) -> Result<(), String> {
    // 右键菜单保护期：原生菜单无关闭回调，只能定时兜底；期间智能穿透不切换，
    // 防止光标在菜单项上游走时把窗口切穿透。Windows 弹出菜单还会模态阻塞
    // dispatcher（轮询线程停顿无损害），恢复后由 hold 继续保护。
    {
        let state = app.state::<CompanionPointerState>();
        *state.hold_until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(std::time::Instant::now() + MENU_HOLD);
    }
    let menu = build_companion_menu(&app).map_err(|e| e.to_string())?;
    let window = app
        .get_webview_window("companion")
        .ok_or_else(|| "角色窗口不存在".to_string())?;
    window
        .popup_menu_at(&menu, LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// 显示设置窗口并聚焦。
fn show_settings_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 切换常驻角色窗口的显隐（文字输入条联动：显隐状态与角色保持一致）。
fn toggle_companion_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("companion") else {
        return;
    };
    if window.is_visible().unwrap_or(true) {
        stop_performance_sync(app);
        let _ = window.hide();
        set_chatbox_visible(app, false, false);
    } else {
        let _ = window.show();
        // 置底时 show 会把窗口顶到 Z 序顶部：Windows 需压回底部（macOS show 不改 level/点穿）。
        if cfg!(windows) && current_companion_layer() == CompanionWindowLayer::Back {
            apply_companion_layer_platform(app, CompanionWindowLayer::Back);
        }
        // 输入条随角色一起显示并聚焦（show 后可直接打字）。macOS 上走 NSPanel 的
        // orderFrontRegardless + makeKeyWindow（见 set_chatbox_visible）：聚焦不激活
        // App，不会把开着的设置窗连带带到最前。
        set_chatbox_visible(app, true, true);
    }
    // 显隐改变了 visible 输入，经单一写点即时重算穿透（不等轮询下一 tick）。
    sync_companion_ignore_cursor_events(app);
    sync_bubble_visibility(app);
}

/// 气泡窗口显隐与角色窗口保持一致（气泡无独立开关，显隐不持久化）。
///
/// 所有改变角色显隐的路径（快捷键切换 / 右键隐藏 / 单实例恢复 / 启动）都应调用，
/// 否则气泡会成为孤儿窗口（角色已隐藏，气泡还漂在屏幕上）。
fn sync_bubble_visibility(app: &AppHandle) {
    let visible = app
        .get_webview_window("companion")
        .map(|w| w.is_visible().unwrap_or(true))
        .unwrap_or(false);
    if let Some(bubble) = app.get_webview_window("bubble") {
        let _ = if visible {
            bubble.show()
        } else {
            bubble.hide()
        };
    }
}

/// 打开设置窗口（供角色窗口右键菜单调用）。
#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings_window(&app);
}

/// 打断当前回复：voice 会话运行中置位打断标志（会话线程停生成/合成/播放回 Armed）；
/// 同时兜底停独立 TTS 播放与 LLM 生成（voice 未运行但测试播放/生成中的场景）。
fn interrupt_reply(app: &AppHandle) {
    let voice = app.state::<VoiceSessionState>();
    if voice.is_running()
        && let Some(flag) = voice
            .barge_in
            .lock()
            .expect("voice barge_in lock poisoned")
            .clone()
    {
        flag.store(true, Ordering::Relaxed);
    }
    // 「没有在合成」不算错误：打断场景下静默跳过
    let _ = stop_tts_inner(app.state::<TtsSynthesizeState>().inner());
    if let Some(engine) = app
        .state::<LlmState>()
        .engine
        .lock()
        .expect("llm lock poisoned")
        .as_ref()
    {
        engine.cancel();
    }
}

/// 全局快捷键触发分发（复用托盘/菜单同款内部函数）。
fn dispatch_shortcut(app: &AppHandle, action: zapmomo::config::shortcuts::ShortcutAction) {
    use zapmomo::config::shortcuts::ShortcutAction;
    match action {
        ShortcutAction::ToggleCompanion => toggle_companion_window(app),
        ShortcutAction::OpenSettings => show_settings_window(app),
        ShortcutAction::InterruptReply => interrupt_reply(app),
        ShortcutAction::ToggleVoiceSession => {
            // stop 需 join 会话线程（等麦克风轮询退出）、start 有模型预检，
            // 都可能耗时：放后台线程避免阻塞快捷键回调
            let app = app.clone();
            std::thread::spawn(move || {
                let state = app.state::<VoiceSessionState>();
                let result = if state.is_running() {
                    stop_voice_session_inner(state.inner())
                } else {
                    start_voice_session_impl(app.clone(), state.inner())
                };
                if let Err(e) = result {
                    tracing::warn!("切换语音会话失败: {e}");
                }
            });
        }
    }
}

/// 启动时按 `[shortcuts]` 配置注册全局快捷键：单个失败仅告警不阻塞启动
/// （键位可能已被其他软件占用），其余照常注册。
fn register_shortcuts_at_startup(app: &AppHandle) {
    use zapmomo::config::shortcuts::ShortcutAction;
    let shortcuts = settings::load_settings()
        .ok()
        .flatten()
        .and_then(|s| s.shortcuts)
        .unwrap_or_default();
    for action in ShortcutAction::ALL {
        let Some(acc) = shortcuts.get(action).map(str::to_string) else {
            continue;
        };
        let result = app
            .global_shortcut()
            .on_shortcut(acc.as_str(), move |app, _sc, ev| {
                // 插件在按下和松开各回调一次：只响应按下，否则一次按键切换两次
                // （表现为「按住消失、松开又出现」）
                if ev.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                    dispatch_shortcut(app, action);
                }
            });
        match result {
            Ok(()) => tracing::info!("全局快捷键已注册：{} = {}", action.as_str(), acc),
            Err(e) => tracing::warn!(
                "全局快捷键 {} ({}) 注册失败，已跳过: {e}",
                action.as_str(),
                acc
            ),
        }
    }
}

/// 读取用户自定义快捷键（action 标识 → accelerator，仅含已绑定项）。
#[tauri::command]
fn get_shortcuts() -> Result<std::collections::HashMap<String, String>, String> {
    let shortcuts = settings::load_settings()?
        .unwrap_or_default()
        .shortcuts
        .unwrap_or_default();
    let mut map = std::collections::HashMap::new();
    for action in zapmomo::config::shortcuts::ShortcutAction::ALL {
        if let Some(acc) = shortcuts.get(action) {
            map.insert(action.as_str().to_string(), acc.to_string());
        }
    }
    Ok(map)
}

/// 绑定快捷键：校验 → 查重 → **先注册成功再落盘**（键位被系统/其他应用占用时
/// 注册失败，配置保持原值，杜绝「界面已绑定但实际不生效」的假状态）。
#[tauri::command]
fn set_shortcut(app: AppHandle, action: String, accelerator: String) -> Result<(), String> {
    use zapmomo::config::shortcuts::{ShortcutAction, validate_accelerator};
    let action =
        ShortcutAction::from_ident(&action).ok_or_else(|| format!("未知的操作：{action}"))?;
    let accelerator = accelerator.trim().to_string();
    validate_accelerator(&accelerator)?;

    let mut cfg = settings::load_settings()?.unwrap_or_default();
    let shortcuts = cfg.shortcuts.get_or_insert_with(Default::default);
    if let Some(other) = shortcuts.find_conflict(action, &accelerator) {
        return Err(format!("该快捷键已绑定到「{}」", other.label()));
    }
    // 幂等：与当前值相同直接成功
    if shortcuts.get(action) == Some(accelerator.as_str()) {
        return Ok(());
    }
    let old = shortcuts.get(action).map(str::to_string);
    app.global_shortcut()
        .on_shortcut(accelerator.as_str(), move |app, _sc, ev| {
            // 同启动注册路径：只响应按下，避免松开时二次触发
            if ev.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                dispatch_shortcut(app, action);
            }
        })
        .map_err(|e| format!("注册失败，可能已被其他应用占用：{e}"))?;
    // 新键注册成功后才解绑旧键
    if let Some(old) = old
        && let Err(e) = app.global_shortcut().unregister(old.as_str())
    {
        tracing::warn!("解绑旧快捷键 {old} 失败: {e}");
    }
    shortcuts.set(action, Some(accelerator));
    settings::save_settings(&cfg)?;
    Ok(())
}

/// 清除操作的快捷键绑定（解绑 + 配置置空）。
#[tauri::command]
fn clear_shortcut(app: AppHandle, action: String) -> Result<(), String> {
    use zapmomo::config::shortcuts::ShortcutAction;
    let action =
        ShortcutAction::from_ident(&action).ok_or_else(|| format!("未知的操作：{action}"))?;
    let mut cfg = settings::load_settings()?.unwrap_or_default();
    if let Some(shortcuts) = cfg.shortcuts.as_mut() {
        if let Some(cur) = shortcuts.get(action).map(str::to_string)
            && let Err(e) = app.global_shortcut().unregister(cur.as_str())
        {
            tracing::warn!("解绑快捷键 {cur} 失败: {e}");
        }
        shortcuts.set(action, None);
    }
    settings::save_settings(&cfg)?;
    Ok(())
}

/// 隐藏角色窗口（隐藏时停表演，防幽灵表演态）。
fn hide_companion_window(app: &AppHandle) {
    stop_performance_sync(app);
    if let Some(window) = app.get_webview_window("companion") {
        let _ = window.hide();
    }
    // 隐藏改变 visible 输入，经单一写点即时重算穿透（不等轮询下一 tick）。
    sync_companion_ignore_cursor_events(app);
    sync_bubble_visibility(app);
}

/// 隐藏角色窗口（供角色窗口右键菜单调用）。
#[tauri::command]
fn hide_companion(app: AppHandle) {
    hide_companion_window(&app);
}

/// 退出应用（供角色窗口右键菜单调用）。退出前清理 dsh 桥发现文件（防死端口残留）
/// 与 audio.cpp sidecar 进程。
#[tauri::command]
fn quit_app(app: AppHandle) {
    zapmomo::dsh::remove_discovery();
    zapmomo::audiocpp::server::shutdown_blocking();
    app.exit(0);
}

/// 重启应用（退出后自动重新拉起，供设置页按钮调用）。退出前清理 dsh 桥发现文件
/// 与 audio.cpp sidecar 进程。
#[tauri::command]
fn restart_app(app: AppHandle) {
    zapmomo::dsh::remove_discovery();
    zapmomo::audiocpp::server::shutdown_blocking();
    app.request_restart();
}

// ===========================================================================
// 模型库（Model Library）
// ===========================================================================

/// 模型库下载任务状态：单任务 + 可取消 + 记录当前下载的模型 id。
#[derive(Default)]
struct ModelLibraryState {
    in_progress: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    current_id: Arc<Mutex<Option<String>>>,
}

/// 模型库下载进度事件载荷。
#[derive(Clone, Serialize)]
struct ModelLibraryProgressPayload {
    model_id: String,
    stage: String,
    asset: String,
    overall_percent: f64,
    bytes_downloaded: u64,
    total_bytes: u64,
    message: String,
}

/// 下载任务 guard：所有出口（成功/失败/取消/panic）都复位下载标志与 cancel。
struct LibraryDownloadGuard {
    in_progress: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    current_id: Arc<Mutex<Option<String>>>,
}

impl Drop for LibraryDownloadGuard {
    fn drop(&mut self) {
        self.in_progress.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
        *self.current_id.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

fn download_stage_str(stage: zapmomo::kws::model::DownloadStage) -> &'static str {
    use zapmomo::kws::model::DownloadStage::*;
    match stage {
        Downloading => "downloading",
        Verifying => "verifying",
        Extracting => "extracting",
        Done => "done",
    }
}

/// 从模型库列表解析模型（按 `id` 或 `install_id`；Current/Delete 可唯一定位具体安装实例）。
fn resolve_library_model(id: &str) -> Result<LibraryModel, String> {
    model_library::resolve_model(id).ok_or_else(|| format!("未知的模型：{id}"))
}

/// 平台化打开目录（macOS `open` / Linux `xdg-open` / Windows `explorer`）。
fn open_path(p: &Path) -> Result<(), String> {
    let cmd = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(cmd)
        .arg(p)
        .spawn()
        .map_err(|e| format!("打开目录失败：{e}"))?;
    Ok(())
}

/// 模型库列表（含每个模型的安装状态 / current / runtime_status）。
#[tauri::command]
fn list_model_library(
    kws: State<'_, ListenState>,
    asr: State<'_, AsrListenState>,
    asr_dictate: State<'_, AsrDictateState>,
    tts: State<'_, TtsSynthesizeState>,
) -> Result<Vec<LibraryModel>, String> {
    let mut models = model_library::list_models();
    let kws_actual = kws.active_model_dir();
    // 流式识别与离线听写任一在跑 → ASR RuntimeActual 置位（模型库卡片显示 Active）
    let asr_actual = asr
        .active_model_dir()
        .or_else(|| asr_dictate.active_model_dir());
    // TTS 无常驻引擎：actual = 当前 selection（与 current 判定同源，写配置即切换），
    // active = 是否有合成线程在跑。
    // LLM 已改为远程连接：无本地 runtime，llm 相关 actual 恒 None/false。
    let tts_actual = model_library::selection_path(LibModelType::Tts);
    let actuals = model_library::RuntimeActuals {
        kws: kws_actual.as_deref(),
        asr: asr_actual.as_deref(),
        tts: tts_actual.as_deref(),
        tts_active: tts.is_synthesizing(),
        llm: None,
        llm_switching: false,
        llm_switch_target: None,
        llm_load_error_path: None,
    };
    model_library::enrich_runtime_status(&mut models, &actuals);
    Ok(models)
}

/// 系统资源（独立命令，CPU 采样在阻塞线程执行）。
#[tauri::command]
async fn get_system_resources() -> Result<SystemResources, String> {
    tauri::async_runtime::spawn_blocking(model_library::sysinfo::get_system_resources)
        .await
        .map_err(|e| format!("资源检测失败：{e}"))
}

/// 下载并安装模型库中的 registry 模型（单任务，真实进度，可取消）。
#[tauri::command]
async fn download_library_model(
    app: AppHandle,
    state: State<'_, ModelLibraryState>,
    id: String,
) -> Result<(), String> {
    let flag = state.in_progress.clone();
    if flag.swap(true, Ordering::SeqCst) {
        return Err("已有模型下载进行中，请稍候".to_string());
    }
    state.cancel.store(false, Ordering::SeqCst);
    *state.current_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());

    let model = model_library::registry::model_by_id(&id)
        .ok_or_else(|| format!("未知的 Registry 模型：{id}"))?;
    if model.download.is_none() {
        flag.store(false, Ordering::SeqCst);
        *state.current_id.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Err("该模型没有内置下载源".to_string());
    }

    let app = app.clone();
    let cancel = state.cancel.clone();
    let current_id = state.current_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = LibraryDownloadGuard {
            in_progress: flag,
            cancel: cancel.clone(),
            current_id,
        };
        let emit = |stage: &str, percent: f64, msg: &str| {
            let _ = app.emit(
                "model-library-download-progress",
                ModelLibraryProgressPayload {
                    model_id: id.clone(),
                    stage: stage.to_string(),
                    asset: String::new(),
                    overall_percent: percent,
                    bytes_downloaded: 0,
                    total_bytes: 0,
                    message: msg.to_string(),
                },
            );
        };
        emit("preparing", 0.0, "准备下载…");
        let mut progress = |p: zapmomo::kws::model::DownloadProgress| {
            let _ = app.emit(
                "model-library-download-progress",
                ModelLibraryProgressPayload {
                    model_id: id.clone(),
                    stage: download_stage_str(p.stage).to_string(),
                    asset: String::new(),
                    overall_percent: p.percent,
                    bytes_downloaded: p.bytes_downloaded,
                    total_bytes: p.total_bytes,
                    message: p.message,
                },
            );
        };
        let install_cancel = cancel.clone();
        let result =
            model_library::install_managed_model(model, &mut progress, Some(&*install_cancel));
        match result {
            Ok(_) => {
                emit("done", 100.0, "模型安装完成");
                Ok(())
            }
            Err(zapmomo::kws::model::ModelError::Cancelled) => {
                emit("cancelled", 0.0, "已取消下载");
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    })
    .await
    .map_err(|e| format!("下载任务异常：{e}"))?
}

/// 取消当前下载。
#[tauri::command]
fn cancel_model_download(state: State<'_, ModelLibraryState>) -> Result<(), String> {
    if !state.in_progress.load(Ordering::Relaxed) {
        return Err("没有正在进行的下载".to_string());
    }
    state.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 设为当前模型（「使用」）。
///
/// 只写 `model_dir`，**绝不写 enabled / 自动启动能力**。
/// KWS/ASR 监听中提示重启；TTS 会话中热切换（失败整表回滚）。
/// LLM 已改为远程连接，本地模型切换不再支持。
#[tauri::command]
async fn set_current_model(
    app: AppHandle,
    kws: State<'_, ListenState>,
    asr: State<'_, AsrListenState>,
    id: String,
) -> Result<SetCurrentResult, String> {
    let model = resolve_library_model(&id)?;
    if model.install_state != LibInstallState::Installed {
        return Err("该模型未安装或正在下载，无法设为当前模型".to_string());
    }
    let path = PathBuf::from(model.local_path.clone().ok_or("该模型没有可用路径")?);
    let mt = model.model_type;

    // ---- KWS / ASR / TTS：写 selection；KWS/ASR 监听中提示重启，TTS 会话中热切换 ----
    if mt != LibModelType::Llm {
        // TTS 事务快照：热切换构造失败时整表回滚（set_selected_model 会同步清
        // voice/engine_path 等伴生字段，单恢复 model_dir 不够）
        let tts_snapshot = if mt == LibModelType::Tts {
            settings::load_settings()?.and_then(|s| s.tts.clone())
        } else {
            None
        };
        model_library::set_selected_model(mt, &path)?;

        // TTS：dsh 播报常驻引擎缓存失效（下次播报按新配置懒重建）
        if mt == LibModelType::Tts
            && let Some(dsh_state) = app.try_state::<DshBridgeState>()
        {
            *dsh_state
                .announcer
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
        }

        let (action, effective, message) = match mt {
            LibModelType::Kws if kws.is_listening() => (
                LibRuntimeAction::RestartRequired,
                false,
                format!(
                    "已将 {} 设为 KWS 当前模型，将在下次启动监听时生效",
                    model.display_name
                ),
            ),
            LibModelType::Asr if asr.is_listening() => (
                LibRuntimeAction::RestartRequired,
                false,
                format!(
                    "已将 {} 设为 ASR 当前模型，将在下次启动识别时生效",
                    model.display_name
                ),
            ),
            LibModelType::Tts => {
                // 语音会话运行中 → 构造新引擎塞邮箱（句间热切换，下一句起生效，
                // 当前句不打断、会话历史不断）；失败整表回滚（会话无感知继续旧引擎）。
                // 会话未运行 / 邮箱已清（退出竞态）→ 只写 selection（下次合成生效）。
                let voice_state = app.state::<VoiceSessionState>();
                let slot = voice_state.tts_swap.lock().ok().and_then(|g| g.clone());
                match slot {
                    Some(slot) if voice_state.is_running() => {
                        // selection 已写入 → resolve 新配置并预检（快速失败，不浪费引擎构造）
                        let settings = settings::load_settings()?;
                        let tts_settings = settings.as_ref().and_then(|s| s.tts.clone());
                        let cfg = zapmomo::tts::config::resolve(tts_settings.as_ref(), None)
                            .and_then(|cfg| zapmomo::tts::config::preflight(&cfg).map(|_| cfg))
                            .map_err(|e| {
                                // clone：快照还要留给后续引擎构造失败分支
                                let _ = model_library::restore_tts_settings(tts_snapshot.clone());
                                format!("切换失败（已回滚，语音会话继续使用原模型）：{e}")
                            })?;
                        // 重活放阻塞线程（sherpa 加载 1~2s / audiocpp spawn+加载 1~3s）
                        let generation = TTS_SWAP_GEN.fetch_add(1, Ordering::Relaxed) + 1;
                        let built = tauri::async_runtime::spawn_blocking(move || {
                            zapmomo::tts::TtsEngine::new(cfg.clone()).map(|engine| {
                                zapmomo::voice::TtsSwap {
                                    engine,
                                    cfg,
                                    generation,
                                }
                            })
                        })
                        .await;
                        match built {
                            Ok(Ok(swap)) => {
                                // 覆盖语义：连续切换时旧 pending 引擎 drop（释放内存/租约）
                                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(swap);
                                (
                                    LibRuntimeAction::None,
                                    true,
                                    format!(
                                        "已将 {} 设为当前模型，语音会话下一句起生效",
                                        model.display_name
                                    ),
                                )
                            }
                            // 引擎构造失败（模型文件损坏等）
                            Ok(Err(e)) => {
                                let _ = model_library::restore_tts_settings(tts_snapshot);
                                return Err(format!(
                                    "切换失败（已回滚，语音会话继续使用原模型）：{e}"
                                ));
                            }
                            // 阻塞任务 panic（join 失败）：同样整表回滚
                            Err(e) => {
                                let _ = model_library::restore_tts_settings(tts_snapshot);
                                return Err(format!(
                                    "切换失败（已回滚，语音会话继续使用原模型）：{e}"
                                ));
                            }
                        }
                    }
                    _ => (
                        LibRuntimeAction::None,
                        true,
                        format!("已将 {} 设为当前模型", model.display_name),
                    ),
                }
            }
            _ => (
                LibRuntimeAction::None,
                true,
                format!("已将 {} 设为当前模型", model.display_name),
            ),
        };
        return Ok(SetCurrentResult {
            model_type: mt,
            model_id: model.id,
            path: path.display().to_string(),
            runtime_action: action,
            effective_immediately: effective,
            message,
        });
    }

    // ---- LLM：本地推理已移除 ----
    Err("本地 LLM 模型已移除：LLM 改为远程连接，请在 LLM 配置页填写 API 地址与 Key".to_string())
}

/// 删除模型：managed 删文件；external 只移除注册。后端全量安全检查。
#[tauri::command]
fn delete_model(
    dl: State<'_, ModelLibraryState>,
    kws: State<'_, ListenState>,
    asr: State<'_, AsrListenState>,
    id: String,
) -> Result<(), String> {
    let model = resolve_library_model(&id)?;
    let downloading = dl.in_progress.load(Ordering::Relaxed)
        && dl
            .current_id
            .lock()
            .map(|g| g.as_deref() == Some(id.as_str()))
            .unwrap_or(false);
    if downloading {
        return Err("该模型正在下载，请先取消下载".to_string());
    }
    if model.current {
        return Err("该模型当前正在使用，请先切换到其他模型".to_string());
    }
    if let Some(lp) = &model.local_path {
        let lp = Path::new(lp);
        let loaded = kws
            .active_model_dir()
            .is_some_and(|d| model_library::paths_equal(&d, lp))
            || asr
                .active_model_dir()
                .is_some_and(|d| model_library::paths_equal(&d, lp));
        if loaded {
            return Err("该模型当前仍在运行，请先停止或切换模型".to_string());
        }
    }

    if let Some(ext_id) = model_library::external_binding_to_remove(&id) {
        // external：只移除注册，绝不删原始文件
        model_library::remove_local_model_record(&ext_id)?;
        return Ok(());
    }
    // HF 安装：删除具体 artifact 目录（只删该 variant），并清理空父目录
    if model.source == model_library::ModelSource::Hf {
        if let Some(lp) = &model.local_path {
            let dir = model_library::runtime_to_install_dir(Path::new(lp));
            model_library::delete_hf_install_dir(&dir)?;
        }
        return Ok(());
    }
    let reg = model_library::registry::model_by_id(&id)
        .ok_or_else(|| format!("未知的 Registry 模型：{id}"))?;
    // 优先按 local_path（双根定位后的实际位置）推导目录；无 local_path（NotInstalled）
    // 再回退主根标准目录——旧根存量也能删到，而不是对着新根路径误判/漏删。
    let dir = model
        .local_path
        .map(|lp| model_library::runtime_to_install_dir(Path::new(&lp)))
        .filter(|d| d.exists())
        .unwrap_or_else(|| model_library::managed_install_dir(reg));
    if dir.exists() {
        model_library::delete_managed_dir(&dir)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 自定义数据目录（存储位置）
// ---------------------------------------------------------------------------

/// 存储迁移状态：防重入 + 取消标志。
#[derive(Default)]
struct StorageMigrateState {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl StorageMigrateState {
    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

/// 迁移 guard：所有出口（成功/失败/取消/panic）复位 running 与 cancel。
struct StorageMigrateGuard {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Drop for StorageMigrateGuard {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
    }
}

/// 检查「设置/迁移数据目录」是否被占用（下载中 / 语音会话 / 监听 / 迁移中）。
///
/// 命中返回具体错误。
fn check_storage_busy(
    dl_kws: &DownloadState,
    dl_asr: &AsrDownloadState,
    dl_tts: &TtsDownloadState,
    lib_dl: &ModelLibraryState,
    voice: &VoiceSessionState,
    kws: &ListenState,
    asr: &AsrListenState,
) -> Result<(), String> {
    if dl_kws.in_progress.load(Ordering::Relaxed)
        || dl_asr.in_progress.load(Ordering::Relaxed)
        || dl_tts.in_progress.load(Ordering::Relaxed)
        || lib_dl.in_progress.load(Ordering::Relaxed)
    {
        return Err("有模型正在下载，请先等待下载完成或取消后再操作".to_string());
    }
    if voice.is_running() {
        return Err("语音会话正在运行，请先停止会话后再操作".to_string());
    }
    if kws.is_listening() || asr.is_listening() {
        return Err("有监听任务正在运行，请先停止后再操作".to_string());
    }
    Ok(())
}

/// 读取存储信息（当前/旧根、占用大小、迁移可用性、磁盘空间）。
#[tauri::command]
async fn get_storage_info(mig: State<'_, StorageMigrateState>) -> Result<StorageInfoView, String> {
    let mut info =
        tauri::async_runtime::spawn_blocking(zapmomo::model_library::storage::collect_storage_info)
            .await
            .map_err(|e| e.to_string())??;
    info.migrating = mig.is_running();
    Ok(info)
}

/// 设置（或清除）自定义数据目录。切换立即生效：新下载走新目录，存量模型保持可见可用。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn set_data_dir(
    app: AppHandle,
    path: Option<String>,
    dl_kws: State<'_, DownloadState>,
    dl_asr: State<'_, AsrDownloadState>,
    dl_tts: State<'_, TtsDownloadState>,
    lib_dl: State<'_, ModelLibraryState>,
    voice: State<'_, VoiceSessionState>,
    kws: State<'_, ListenState>,
    asr: State<'_, AsrListenState>,
    mig: State<'_, StorageMigrateState>,
) -> Result<StorageInfoView, String> {
    if mig.is_running() {
        return Err("正在迁移模型，请稍候".to_string());
    }
    check_storage_busy(
        dl_kws.inner(),
        dl_asr.inner(),
        dl_tts.inner(),
        lib_dl.inner(),
        voice.inner(),
        kws.inner(),
        asr.inner(),
    )?;

    let data_dir_value = match &path {
        Some(p) if !p.trim().is_empty() => Some(
            zapmomo::model_library::storage::validate_data_dir(Path::new(p))?
                .display()
                .to_string(),
        ),
        _ => None,
    };
    zapmomo::model_library::update_settings(|cfg| {
        cfg.data_dir = data_dir_value.clone();
    })?;
    zapmomo::config::settings::refresh_data_dir_cache();
    let _ = app.emit("storage-dir-changed", ());

    tauri::async_runtime::spawn_blocking(zapmomo::model_library::storage::collect_storage_info)
        .await
        .map_err(|e| e.to_string())?
}

/// 迁移旧根存量到新数据目录（后台执行，进度经 `storage-migrate-progress` 事件推送）。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn migrate_storage(
    app: AppHandle,
    mig: State<'_, StorageMigrateState>,
    dl_kws: State<'_, DownloadState>,
    dl_asr: State<'_, AsrDownloadState>,
    dl_tts: State<'_, TtsDownloadState>,
    lib_dl: State<'_, ModelLibraryState>,
    voice: State<'_, VoiceSessionState>,
    kws: State<'_, ListenState>,
    asr: State<'_, AsrListenState>,
) -> Result<(), String> {
    if mig.is_running() {
        return Err("迁移已在进行中".to_string());
    }
    check_storage_busy(
        dl_kws.inner(),
        dl_asr.inner(),
        dl_tts.inner(),
        lib_dl.inner(),
        voice.inner(),
        kws.inner(),
        asr.inner(),
    )?;
    mig.running.store(true, Ordering::SeqCst);
    mig.cancel.store(false, Ordering::SeqCst);
    let running = mig.running.clone();
    let cancel = mig.cancel.clone();
    let emit_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = StorageMigrateGuard {
            running: running.clone(),
            cancel: cancel.clone(),
        };
        let outcome = zapmomo::model_library::storage::run_migration(
            false,
            &mut |p| {
                let _ = emit_app.emit("storage-migrate-progress", &p);
            },
            Some(&cancel),
        );
        match &outcome {
            Ok(o) => {
                if o.failed.is_empty() {
                    tracing::info!(
                        "存储迁移完成（moved={} skipped={}）",
                        o.moved.len(),
                        o.skipped.len()
                    );
                } else {
                    tracing::warn!("存储迁移部分失败：{:?}", o.failed);
                }
            }
            Err(e) => tracing::error!("存储迁移异常: {e}"),
        }
        outcome
    })
    .await
    .map_err(|e| format!("迁移任务异常: {e}"))??;

    // 迁移完成后：伙伴 active 已 relocate，重新 reconcile（allow_directory + 桌宠重载）
    if let Ok(lib) = zapmomo::companion::load_library_fast() {
        let active = zapmomo::companion::active_model(&lib);
        let _ = reconcile_active(&app, active);
        for model in &lib.models {
            if zapmomo::companion::quick_valid(model) {
                let _ = app
                    .asset_protocol_scope()
                    .allow_directory(Path::new(&model.model_dir), true);
            }
        }
    }
    let _ = app.emit("storage-dir-changed", ());
    Ok(())
}

/// 取消进行中的存储迁移（条目间/拷贝块间生效；已迁移条目保留）。
#[tauri::command]
fn cancel_storage_migration(mig: State<'_, StorageMigrateState>) -> Result<(), String> {
    if !mig.is_running() {
        return Err("当前没有迁移在运行".to_string());
    }
    mig.cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// 在文件管理器中打开当前模型目录。
#[tauri::command]
fn open_storage_dir() -> Result<(), String> {
    open_path(&zapmomo::config::settings::get_models_dir())
}

/// 从伙伴清单定位要打开的托管资产目录：未知 id 或目录已缺失均报错。
///
/// 返回清单中的 `model_dir`（绝对路径字符串），打开前校验目录真实存在，
/// 避免文件管理器弹系统级错误。
fn resolve_companion_dir<'a>(
    models: &'a [zapmomo::companion::CompanionModel],
    id: &str,
) -> Result<&'a str, String> {
    let model = models
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("未知的伙伴：{id}"))?;
    if !Path::new(&model.model_dir).is_dir() {
        return Err(format!(
            "伙伴「{}」的资产目录不存在，可能已被移动或删除",
            model.name
        ));
    }
    Ok(&model.model_dir)
}

/// 在文件管理器中打开指定伙伴的托管资产目录（用户可自行调整音色参考等资产）。
#[tauri::command]
fn open_companion_dir(id: String) -> Result<(), String> {
    let lib = zapmomo::companion::load_library_fast()?;
    let dir = resolve_companion_dir(&lib.models, &id)?;
    open_path(Path::new(dir))
}

/// 角色窗口初始尺寸（逻辑像素，与 `setup` 中的 `inner_size` 保持一致）。
const COMPANION_INITIAL_W: f64 = 360.0;
const COMPANION_INITIAL_H: f64 = 480.0;
/// 角色窗口距屏幕工作区边缘的留白（逻辑像素）。
const COMPANION_MARGIN: f64 = 16.0;
/// 窗口顶部为 dsh 事件 toast 堆叠预留的高度（逻辑像素，最前卡片 + 2 层向上 peek），
/// 模型渲染区整体下移一条，堆叠卡片不遮挡模型。需与前端 `BUBBLE_STRIP`
/// （CompanionRoot.tsx）保持一致。
const COMPANION_BUBBLE_STRIP: f64 = 72.0;
/// 文字输入条窗口尺寸（逻辑像素，与 `setup` 中的 `inner_size` 保持一致）。
/// 高度预留：底部 26px 透明外边距给 CSS 阴影留扩散空间（透明窗口会裁剪窗口外阴影），
/// 其余留给多行生长与行内错误提示（发送失败时展示，正常时不占视觉空间——透明窗口）。
const CHATBOX_W: f64 = 520.0;
const CHATBOX_H: f64 = 96.0;
/// 输入条默认位置距屏幕工作区底边的留白（逻辑像素）：galgame 对话框位，明显高于贴边。
const CHATBOX_BOTTOM_MARGIN: f64 = 120.0;
/// 语音回复气泡窗口尺寸（逻辑像素，与 `setup` 中的 `inner_size` 保持一致）。
/// 高度为初始值：前端 ResizeObserver 随内容自适应调整（底边锚定向上生长），
/// 此值仅用于建窗与默认定位；底部 26px 透明外边距给 CSS 阴影留扩散空间。
const BUBBLE_W: f64 = 480.0;
const BUBBLE_H: f64 = 180.0;
/// 气泡默认位置距屏幕工作区底边的留白（逻辑像素）：输入条默认位正上方
///（输入条底边距 + 输入条高度 + 16px 间距）。
const BUBBLE_BOTTOM_MARGIN: f64 = CHATBOX_BOTTOM_MARGIN + CHATBOX_H + 16.0;

/// 计算角色窗口首次出现的右下角位置（逻辑像素）。
///
/// 基于主屏 `work_area`（排除 Dock / 任务栏），把物理像素坐标除以 scale_factor
/// 转为逻辑像素，再减去窗口尺寸与留白得到窗口左上角坐标。
fn default_bottom_right_position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let right = (work.position.x as f64 + work.size.width as f64) / scale;
    let bottom = (work.position.y as f64 + work.size.height as f64) / scale;
    Some((
        right - COMPANION_INITIAL_W - COMPANION_MARGIN,
        bottom - COMPANION_INITIAL_H - COMPANION_MARGIN,
    ))
}

/// 计算输入条窗口首次出现的位置（逻辑像素）：主屏工作区底部居中
/// （galgame 对话框位；排除 Dock / 任务栏）。拖动后由配置记忆接管。
fn default_chatbox_position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let left = work.position.x as f64 / scale;
    let top = work.position.y as f64 / scale;
    let w = work.size.width as f64 / scale;
    let h = work.size.height as f64 / scale;
    Some((
        left + (w - CHATBOX_W) / 2.0,
        top + h - CHATBOX_H - CHATBOX_BOTTOM_MARGIN,
    ))
}

/// 计算气泡窗口首次出现的位置（逻辑像素）：主屏工作区底部居中、输入条默认位
/// 正上方（排除 Dock / 任务栏）。拖动后由配置记忆接管。
fn default_bubble_position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let left = work.position.x as f64 / scale;
    let top = work.position.y as f64 / scale;
    let w = work.size.width as f64 / scale;
    let h = work.size.height as f64 / scale;
    Some((
        left + (w - BUBBLE_W) / 2.0,
        top + h - BUBBLE_H - BUBBLE_BOTTOM_MARGIN,
    ))
}

/// 逻辑像素坐标是否落在任一显示器的可见工作区内。
///
/// 用于过滤「拔掉外接屏后残留的屏幕外记忆位置」：多屏布局变化后恢复窗口
/// 会导致窗口出现在不可见区域，此时应回退默认定位。查询失败时不拦截（保持恢复行为）。
fn position_on_any_monitor(app: &AppHandle, x: f64, y: f64) -> bool {
    match app.available_monitors() {
        Ok(monitors) => monitors.iter().any(|m| {
            let scale = m.scale_factor();
            let work = m.work_area();
            let left = work.position.x as f64 / scale;
            let top = work.position.y as f64 / scale;
            let right = left + work.size.width as f64 / scale;
            let bottom = top + work.size.height as f64 / scale;
            x >= left && x < right && y >= top && y < bottom
        }),
        Err(_) => true,
    }
}

/// Tauri 应用入口。
pub fn run() {
    zapmomo::logging::init_logging();
    tauri::Builder::default()
        // 单实例防护：官方要求注册为第一个插件。自启常驻后用户再手动点图标时，
        // 回调在已有实例内执行（第二进程自行退出）：恢复可能隐藏的桌宠并前置设置窗。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("companion")
                && !window.is_visible().unwrap_or(true)
            {
                let _ = window.show();
                // 置底时 show 会把窗口顶到 Z 序顶部：Windows 需压回底部（同 toggle_companion_window）。
                if cfg!(windows) && current_companion_layer() == CompanionWindowLayer::Back {
                    apply_companion_layer_platform(app, CompanionWindowLayer::Back);
                }
                // show 改变 visible 输入：单一写点即时重算穿透（同 toggle_companion_window）。
                sync_companion_ignore_cursor_events(app);
            }
            sync_bubble_visibility(app);
            show_settings_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 开机自启动：macOS 用 LaunchAgent（AppleScript 变体依赖 osascript，无必要）；
        // 注册时附加 `--autostart` 参数，setup 检测到则跳过设置窗自动弹出（静默启动）。
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .manage(ListenState::new())
        .manage(CompanionPointerState::new())
        .manage(DownloadState::default())
        .manage(AsrListenState::new())
        .manage(AsrDictateState::new())
        .manage(AsrDownloadState::default())
        .manage(TtsSynthesizeState::new())
        .manage(TtsDownloadState::default())
        .manage(LlmState::new())
        .manage(VoiceSessionState::new())
        .manage(DshBridgeState::new())
        .manage(ModelLibraryState::default())
        .manage(StorageMigrateState::default())
        .invoke_handler(tauri::generate_handler![
            get_app_info,
            list_devices,
            request_mic_permission,
            get_kws_config,
            set_kws_enabled,
            set_kws_custom_keywords,
            set_kws_params,
            start_listen,
            stop_listen,
            is_listening,
            download_kws_model,
            get_microphone,
            set_microphone,
            get_asr_config,
            set_asr_enabled,
            set_asr_params,
            start_asr_listen,
            stop_asr_listen,
            is_asr_listening,
            download_asr_model,
            transcribe_audio,
            start_asr_dictate,
            stop_asr_dictate,
            is_asr_dictating,
            get_tts_config,
            list_tts_voices,
            save_tts_voice,
            delete_tts_voice,
            record_tts_voice,
            transcribe_reference_audio,
            synthesize_tts,
            stop_tts,
            is_tts_synthesizing,
            download_tts_model,
            get_llm_config,
            load_llm_model,
            unload_llm_model,
            chat_llm,
            stop_llm,
            is_llm_ready,
            set_llm_params,
            set_llm_system_prompt,
            set_llm_connection,
            set_tts_enabled,
            set_tts_params,
            set_tts_voice,
            set_tts_backend,
            start_voice_session,
            stop_voice_session,
            is_voice_session_running,
            send_voice_text,
            get_dsh_config,
            set_dsh_enabled,
            set_dsh_params,
            get_dsh_bridge_status,
            test_dsh_announce,
            get_conversation_records,
            list_model_library,
            get_system_resources,
            download_library_model,
            cancel_model_download,
            set_current_model,
            delete_model,
            get_storage_info,
            set_data_dir,
            migrate_storage,
            cancel_storage_migration,
            open_storage_dir,
            clear_conversation_records,
            get_live2d_config,
            list_companions,
            import_companion,
            set_active_companion,
            rename_companion,
            remove_companion,
            open_companion_dir,
            save_cover_image,
            save_companion_position,
            save_chatbox_position,
            save_bubble_position,
            bubble_debug_log,
            hide_chatbox,
            set_companion_scale,
            set_companion_opacity,
            set_companion_click_through,
            set_companion_smart_click_through,
            set_companion_hit_region,
            set_companion_layer,
            set_companion_locked,
            set_companion_drag_mode,
            show_companion_menu,
            start_performance,
            stop_performance,
            is_performing,
            get_hide_dock_icon,
            set_hide_dock_icon,
            get_autostart,
            set_autostart,
            open_settings,
            get_shortcuts,
            set_shortcut,
            clear_shortcut,
            hide_companion,
            quit_app,
            restart_app
        ])
        .setup(|app| {
            // macOS：默认以普通应用出现（Dock + Cmd+Tab 可见，有全局菜单栏）；
            // 用户可在设置中开启「隐藏应用图标」，此时切换为 Accessory（从 Dock 与 Cmd+Tab 消失）。
            let loaded = settings::load_settings().ok().flatten();
            #[cfg(target_os = "macos")]
            let hide_dock_icon = loaded.as_ref().map(|s| s.hide_dock_icon).unwrap_or(false);

            #[cfg(target_os = "macos")]
            {
                app.handle().set_activation_policy(if hide_dock_icon {
                    tauri::ActivationPolicy::Accessory
                } else {
                    tauri::ActivationPolicy::Regular
                })?;
            }

            // 启动自动启动语音会话（若用户启用 voice）：进入待唤醒（Armed），失败静默降级。
            // voice 会话内部自带 KWS 与 LLM（自持引擎），因此自动启动成功时**跳过**
            // 下方独立的 LLM auto_load 与 KWS 自动监听——避免同一模型文件/麦克风设备
            // 被两份并发占用（llama.cpp 双 engine 并发加载会崩，cpal 同设备双路采集冲突）。
            let voice_auto_started = if loaded
                .as_ref()
                .and_then(|s| s.voice.as_ref())
                .and_then(|v| v.enabled)
                .unwrap_or(true)
            {
                let handle = app.handle().clone();
                let state = app.state::<VoiceSessionState>();
                match start_voice_session_impl(handle, state.inner()) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("自动启动语音会话失败: {e}");
                        false
                    }
                }
            } else {
                false
            };

            // 启动时自动连接远程 LLM（voice 未接管且已配置 base_url/model 时）：后台异步连接，
            // 失败静默降级为手动连接。
            if !voice_auto_started
                && llm_resolved_config()
                    .map(|c| {
                        c.enabled
                            && !c.base_url.as_deref().unwrap_or("").trim().is_empty()
                            && !c.model.as_deref().unwrap_or("").trim().is_empty()
                    })
                    .unwrap_or(false)
            {
                let handle = app.handle().clone();
                let state = app.state::<LlmState>();
                if let Err(e) = load_llm_impl(handle, state.inner()) {
                    tracing::warn!("自动连接 LLM 失败: {e}");
                }
            }

            // 启动自动监听 KWS（若用户启用 KWS 且未由语音会话代管）：后台线程监听，失败静默降级。
            // 使用持久化的麦克风（顶层 microphone）与自定义唤醒词（[kws].custom_keywords，空则模型内置）。
            if !voice_auto_started
                && zapmomo::kws::config::resolve(loaded.as_ref().and_then(|s| s.kws.as_ref()), None)
                    .map(|c| c.enabled)
                    .unwrap_or(false)
            {
                let handle = app.handle().clone();
                let state = app.state::<ListenState>();
                let mic = loaded.as_ref().and_then(|s| s.microphone.clone());
                let kw = loaded
                    .as_ref()
                    .and_then(|s| s.kws.as_ref())
                    .and_then(|k| k.custom_keywords.clone());
                if let Err(e) = start_listen_impl(handle, state.inner(), mic, kw) {
                    tracing::warn!("自动监听 KWS 失败: {e}");
                }
            }

            // 启动 dsh 桥（若启用）：loopback HTTP 接收 deepseek-harness 插件推送的
            // 任务事件，桌宠以气泡+语音播报。失败静默降级（不影响主流程）。
            if zapmomo::dsh::config::resolve(loaded.as_ref().and_then(|s| s.dsh.as_ref())).enabled {
                let handle = app.handle().clone();
                let state = app.state::<DshBridgeState>();
                if let Err(e) = start_dsh_bridge_impl(handle, state.inner()) {
                    tracing::warn!("自动启动 dsh 桥失败: {e}");
                }
            }

            // audio.cpp sidecar 环境：注入引擎搜索目录（externalBin 落位点 = 主程序
            // 同目录 + resource 目录），并启用 45s 空闲保活（GUI 测试语音/会话在窗口
            // 内复用热 server，热请求 0.1s 级）。不在此预热：backend 缺省 sherpa 的
            // 用户不触发进程；首次 audiocpp 合成时按需 spawn。
            {
                let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
                if let Some(exe_dir) = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                {
                    search_dirs.push(exe_dir);
                }
                if let Ok(resource_dir) = app.path().resource_dir()
                    && !search_dirs.contains(&resource_dir)
                {
                    search_dirs.push(resource_dir);
                }
                zapmomo::audiocpp::locator::set_search_dirs(search_dirs);
                zapmomo::audiocpp::server::set_idle_keepalive(Some(
                    std::time::Duration::from_secs(45),
                ));
            }

            // 常驻角色窗口：透明、无边框、永远置顶、不入任务栏，静态展示 Live2D。
            // 复用顶部读到的 settings：同时恢复记忆的尺寸与位置。
            // 尺寸/位置是伙伴私有配置（library.json layout），全局 [live2d] 仅作兜底默认。
            let live2d = loaded.as_ref().and_then(|s| s.live2d.clone());
            let startup_layout = zapmomo::companion::active_layout();
            let scale = resolve_scale(live2d.as_ref(), startup_layout.as_ref());

            // 基准高度：min(480, 主屏工作区高度 × 0.6)，另加顶部气泡预留条（与前端
            // computeSize 一致）。setup 阶段按默认 3:4 宽高比建窗，模型加载后前端按真实宽高比修正。
            let avail_height = app
                .primary_monitor()
                .ok()
                .flatten()
                .map(|m| {
                    let work = m.work_area();
                    (work.position.y as f64 + work.size.height as f64) / m.scale_factor()
                })
                .unwrap_or(1080.0);
            let init_h = 480.0_f64.min(avail_height * 0.6) * scale + COMPANION_BUBBLE_STRIP;
            let init_w = (init_h - COMPANION_BUBBLE_STRIP) * (3.0 / 4.0);

            // 启动同步 reconcile：让 settings 的 [live2d].model_dir 与伙伴库 active 一致，
            // 使 CompanionRoot 挂载时 get_live2d_config 直接读到正确的当前伙伴（毫秒级，不迁移）。
            reconcile_active_at_startup(app.handle());

            let mut companion = WebviewWindowBuilder::new(
                app,
                "companion",
                WebviewUrl::App("companion.html".into()),
            )
            .title("ZapMomo")
            .inner_size(init_w, init_h)
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false);

            // macOS：首次点击直达 webview，可立即触发 startDragging（无需先点一下聚焦）。
            #[cfg(target_os = "macos")]
            {
                companion = companion.accept_first_mouse(true);
            }

            // 有记忆位置 → 恢复（落在所有显示器之外时说明多屏布局已变化，回退默认右下角）；
            // 否则 → 首次定位到屏幕右下角。
            let saved_pos = resolve_position(live2d.as_ref(), startup_layout.as_ref())
                .map(|p| (p.x as f64, p.y as f64))
                .filter(|&(x, y)| position_on_any_monitor(app.handle(), x, y));
            if let Some((x, y)) = saved_pos.or_else(|| default_bottom_right_position(app.handle()))
            {
                companion = companion.position(x, y);
            }
            companion.build()?;

            // macOS：把角色窗口转成非激活面板，点击/拖动不抢前台焦点。
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::{CollectionBehavior, StyleMask, WebviewWindowExt};

                let _ = app.handle().plugin(tauri_nspanel::init());
                if let Some(window) = app.get_webview_window("companion")
                    && let Ok(panel) = window.to_panel::<CompanionPanel>()
                {
                    panel.set_style_mask(StyleMask::empty().nonactivating_panel().into());
                    panel.set_collection_behavior(
                        CollectionBehavior::new()
                            .stationary()
                            .move_to_active_space()
                            .full_screen_auxiliary()
                            .into(),
                    );
                }
            }

            // 恢复持久化的显示层级（置顶/置底）并即时应用。
            // 必须在 macOS 面板转换之后调用（此时 panel handle 已注册，get_webview_panel 才命中）。
            if let Some(layer) = live2d.as_ref().and_then(|l| l.window_layer) {
                apply_companion_layer_platform(app.handle(), layer);
            }
            // 恢复穿透：初始化策略快照后经单一写点一次性应用强制穿透/智能穿透/层级
            // 的综合决策（替代旧的裸 set_ignore_cursor_events 直写；窗口 hide/show
            // 不销毁窗口对象，穿透状态跨显隐由轮询与 push 路径维护）。
            refresh_companion_pointer_policy(app.handle());
            sync_companion_ignore_cursor_events(app.handle());
            // 智能穿透轮询线程：按光标位置动态切换穿透（SMART_CLICK_THROUGH_DESIGN.md）。
            init_companion_pointer_geometry(app.handle());
            start_companion_pointer_watcher(app.handle().clone());
            // 位置锁定无需建窗时后端应用：拦截点在前端 CompanionRoot 的 mousedown，
            // 由 get_live2d_config 恢复（见 frontend CompanionRoot）。

            // 后台旧版迁移：库为空且旧 [live2d].model_dir 存在时，把模型复制进托管目录并
            // 设为 active（完成后 reconcile，桌宠从旧目录无缝切到托管副本）。不阻塞启动。
            migrate_legacy_in_background(app.handle().clone());
            // 后台存量迁移：为已导入伙伴补注册未登记的动作/表情文件（幂等，不阻塞启动）。
            register_motions_in_background();

            // 设置窗口：默认隐藏，由 cmd+, 或托盘菜单打开；关闭时隐藏而非退出。
            let mut settings =
                WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
                    .title("ZapMomo 设置")
                    .inner_size(1180.0, 760.0)
                    .min_inner_size(1180.0, 640.0)
                    .resizable(true)
                    .visible(false);

            // macOS 用 titleBarStyle: Overlay 保留红绿灯；其它平台去掉系统标题栏。
            // title_bar_style / hidden_title 是 macOS 专属方法（Linux 上不存在），
            // 必须用 #[cfg] 编译期隔离，而非 cfg! 运行时判断。
            #[cfg(target_os = "macos")]
            {
                // macOS 保留系统红绿灯与阴影；窗口默认不透明。
                settings = settings
                    .title_bar_style(TitleBarStyle::Overlay)
                    .hidden_title(true)
                    .shadow(true);
            }
            // Linux：去掉系统标题栏，保留透明窗口供 CSS 圆角裁出（三键悬浮右上角，与 Windows 一致）。
            #[cfg(target_os = "linux")]
            {
                settings = settings.decorations(false).transparent(true);
            }
            // Windows：去掉系统标题栏即可；无 CSS 圆角处理，无需透明窗口
            //（不透明窗口性能更好）。同时关 DWM shadow：undecorated+shadow 会被
            // tao 在 WM_NCCALCSIZE 里左右底三边缩进客户区、由 DWM 画黑色窗框，
            // 而顶部 inset 在 Win10 强制为 0（否则画出原生标题栏），形成三边黑框；
            // 四边完整边框改由前端 AppShell 用 CSS 自绘。三键悬浮右上角。
            #[cfg(target_os = "windows")]
            {
                settings = settings.decorations(false).shadow(false);
            }
            settings.build()?;

            // 文字输入条窗口：显隐走持久化开关（缺省显示——首次启动随角色一同
            // 出现），托盘/右键菜单「文字输入条」可勾选开关；关闭走全局
            // CloseRequested → hide。macOS 建窗后转为非激活面板
            // （见下方 to_panel::<ChatboxPanel>）：聚焦输入不激活应用，IME 行为
            // 由 can_become_key_window 保证；其它平台保持普通可激活窗口。
            let chatbox_cfg = loaded.as_ref().and_then(|s| s.chatbox.clone());
            let mut chatbox =
                WebviewWindowBuilder::new(app, "chatbox", WebviewUrl::App("chatbox.html".into()))
                    .title("ZapMomo 输入")
                    .inner_size(CHATBOX_W, CHATBOX_H)
                    .resizable(false)
                    .decorations(false)
                    .transparent(true)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    // 透明窗口的原生阴影按整个窗口矩形绘制，与居中的圆角胶囊错位
                    // （视觉上像错位的边框）——与角色窗口一致关闭，胶囊自带 CSS shadow。
                    .shadow(false)
                    .visible(false);
            // macOS：首次点击直达 webview（无需先点一下聚焦），与角色窗口一致——
            // 否则按住把手的第一下只用于激活窗口，第二下才能拖动。
            #[cfg(target_os = "macos")]
            {
                chatbox = chatbox.accept_first_mouse(true);
            }
            // 定位：配置记忆（落在所有显示器之外时视为多屏布局已变化，回退默认）> 屏幕底部居中
            let saved_pos = chatbox_cfg
                .as_ref()
                .and_then(|c| c.window_position.clone())
                .map(|p| (p.x as f64, p.y as f64))
                .filter(|&(x, y)| position_on_any_monitor(app.handle(), x, y));
            if let Some((x, y)) = saved_pos.or_else(|| default_chatbox_position(app.handle())) {
                chatbox = chatbox.position(x, y);
            }
            chatbox.build()?;
            // macOS：转成非激活面板——聚焦输入框不激活应用（不会把开着的设置窗带到最前），
            // 键盘/中文 IME 输入由 can_become_key_window 保证。
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::{CollectionBehavior, StyleMask, WebviewWindowExt};

                if let Some(window) = app.get_webview_window("chatbox")
                    && let Ok(panel) = window.to_panel::<ChatboxPanel>()
                {
                    panel.set_style_mask(StyleMask::empty().nonactivating_panel().into());
                    panel.set_collection_behavior(
                        CollectionBehavior::new()
                            .stationary()
                            .move_to_active_space()
                            .full_screen_auxiliary()
                            .into(),
                    );
                    // 层级常置 Floating 之上 1 级：恒高于角色（前置 4 / 置底 -1），
                    // 角色永不遮挡输入条。
                    panel.set_level(MACOS_OVERLAY_PANEL_LEVEL);
                }
            }
            // 恢复持久化的可见性（缺省显示：首次启动输入条随角色一同出现）
            if resolve_chatbox_visible(chatbox_cfg.as_ref())
                && let Some(window) = app.get_webview_window("chatbox")
            {
                let _ = window.show();
            }

            // 语音回复气泡窗口：纯展示（流式回复打字机），显隐跟随角色窗口
            // （无独立开关、不持久化）；无文本时完全透明且点击穿透（前端按
            // 内容切换 setIgnoreCursorEvents），有文本时整面可拖动。
            // macOS 建窗后转为非激活面板（can_become_key_window: false），
            // 拖动不抢焦点。
            let bubble_cfg = loaded.as_ref().and_then(|s| s.bubble.clone());
            let mut bubble =
                WebviewWindowBuilder::new(app, "bubble", WebviewUrl::App("bubble.html".into()))
                    .title("ZapMomo 气泡")
                    .inner_size(BUBBLE_W, BUBBLE_H)
                    .resizable(false)
                    .decorations(false)
                    .transparent(true)
                    .always_on_top(true)
                    .skip_taskbar(true)
                    // 与 chatbox 一致：透明窗口原生阴影按整个矩形绘制，关闭，CSS 自绘
                    .shadow(false)
                    .visible(false);
            #[cfg(target_os = "macos")]
            {
                bubble = bubble.accept_first_mouse(true);
            }
            // 定位：配置记忆（落在所有显示器之外时视为多屏布局已变化，回退默认）> 输入条正上方
            let saved_pos = bubble_cfg
                .as_ref()
                .and_then(|c| c.window_position.clone())
                .map(|p| (p.x as f64, p.y as f64))
                .filter(|&(x, y)| position_on_any_monitor(app.handle(), x, y));
            if let Some((x, y)) = saved_pos.or_else(|| default_bubble_position(app.handle())) {
                bubble = bubble.position(x, y);
            }
            bubble.build()?;
            // 空闲点穿：初始忽略光标事件，前端有文本时恢复（builder 无此选项，建窗后设置）
            if let Some(window) = app.get_webview_window("bubble") {
                let _ = window.set_ignore_cursor_events(true);
            }
            // macOS：转成非激活面板——拖动/悬停不激活应用、不抢键盘焦点。
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::{CollectionBehavior, StyleMask, WebviewWindowExt};

                if let Some(window) = app.get_webview_window("bubble")
                    && let Ok(panel) = window.to_panel::<BubblePanel>()
                {
                    panel.set_style_mask(StyleMask::empty().nonactivating_panel().into());
                    panel.set_collection_behavior(
                        CollectionBehavior::new()
                            .stationary()
                            .move_to_active_space()
                            .full_screen_auxiliary()
                            .into(),
                    );
                    // 层级常置 Floating 之上 1 级：恒高于角色（前置 4 / 置底 -1），
                    // 角色永不遮挡气泡。
                    panel.set_level(MACOS_OVERLAY_PANEL_LEVEL);
                }
            }
            // 显隐跟随角色（companion 默认可见 → 气泡随之显示；内容为空时仍点穿）
            sync_bubble_visibility(app.handle());
            // Windows：启动时的层级应用先于 chatbox/bubble 建窗（重断为 no-op），
            // 这里补一次，保证角色前置 topmost 时二者在其上。
            #[cfg(windows)]
            raise_overlay_windows(app.handle());

            // 自动打开设置窗口：仅用于「无全局菜单栏」的场景（macOS Accessory 模式或非 macOS），
            // 否则 Cmd+, 快捷键不可靠，自动打开可避免「找不到设置」；普通模式有菜单栏，无需自动弹出。
            // 自启动拉起（--autostart）时跳过：桌宠静默出现，设置窗不自动弹出；
            // 手动启动行为不变。
            let launched_by_autostart = is_launched_by_autostart(std::env::args());
            #[cfg(target_os = "macos")]
            let auto_open_settings = !hide_dock_icon && !launched_by_autostart;
            #[cfg(not(target_os = "macos"))]
            let auto_open_settings = !launched_by_autostart;
            if auto_open_settings {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    show_settings_window(&app_handle);
                });
            }

            // 应用菜单（仅 macOS）：「ZapMomo」子菜单（偏好设置 cmd+, / 退出 Cmd+Q）
            // 与「编辑」菜单。macOS 的 Cmd+C/V/X/A/Z 依赖菜单中的「编辑」项
            // （key equivalent）才能派发到 WebView 输入框；自定义菜单若缺少这些项，
            // 复制/粘贴/全选会全部失效。
            //
            // Windows/Linux 不设 app 级菜单：Tauri 的 set_menu 会把它作为原生菜单栏
            // 渲染进每个窗口（含无边框的 companion），模型顶部会多出一条菜单；
            // 而这些平台的 Ctrl+C/V 无需菜单即可生效，设置入口走托盘/右键菜单。
            #[cfg(target_os = "macos")]
            {
                let show_settings = MenuItem::with_id(
                    app,
                    "show_settings",
                    "偏好设置…",
                    true,
                    Some("CmdOrCtrl+,"),
                )?;
                let undo = PredefinedMenuItem::undo(app, None)?;
                let redo = PredefinedMenuItem::redo(app, None)?;
                let edit_sep1 = PredefinedMenuItem::separator(app)?;
                let cut = PredefinedMenuItem::cut(app, None)?;
                let copy = PredefinedMenuItem::copy(app, None)?;
                let paste = PredefinedMenuItem::paste(app, None)?;
                let select_all = PredefinedMenuItem::select_all(app, None)?;
                let edit_menu = Submenu::with_items(
                    app,
                    "编辑",
                    true,
                    &[&undo, &redo, &edit_sep1, &cut, &copy, &paste, &select_all],
                )?;
                // 退出项必须用自定义 MenuItem 而非 PredefinedMenuItem::quit：
                // 后者在 macOS 绑定原生 `terminate:`，而 terminate 会逐个询问可见窗口
                // `windowShouldClose:`——被下方 on_window_event 的 prevent_close 拦截后
                // 整个退出被取消（Cmd+Q 表现为窗口隐藏、进程残留）。自定义项直接走
                // handle_menu("quit") → app.exit(0)，绕过窗口询问，与托盘「退出」一致。
                let quit =
                    MenuItem::with_id(app, "quit", "退出 ZapMomo", true, Some("CmdOrCtrl+Q"))?;
                // 注意：muda 在 macOS 只把 Submenu 渲染为菜单栏项，顶级普通 MenuItem
                // 不显示（快捷键仍可派发）。因此偏好设置/退出须收进 app 名子菜单，
                // 保持「Apple | ZapMomo | 编辑」的 macOS 惯例结构。
                let sep = PredefinedMenuItem::separator(app)?;
                let app_submenu =
                    Submenu::with_items(app, "ZapMomo", true, &[&show_settings, &sep, &quit])?;
                let app_menu = Menu::with_items(app, &[&app_submenu, &edit_menu])?;
                app.set_menu(app_menu)?;
            }

            // 托盘菜单：显示/隐藏角色、窗口尺寸/透明度、打开设置、重启、退出。
            let tray_menu = build_tray_menu(app.handle())?;

            // 托盘图标：使用专用托盘图标（tray-icon.png）——真实应用图标的无边距版本，
            // 撑满菜单栏，避免 512px 主图标 9% 留白导致的偏小。
            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
                    .expect("托盘图标加载失败");
            // 菜单事件统一由 app 级 on_menu_event 处理（见下方 Builder::on_menu_event）。
            // 不可在 TrayIcon 上再注册 on_menu_event：tauri 会把 TrayIcon 的 handler
            // 也注册到全局菜单监听器，与 app 级并列，导致每个菜单事件被 handle_menu
            // 处理两次（CheckMenuItem 取反因此净效果为零，表现为点击无效）。
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .menu(&tray_menu)
                .build(app)?;

            // 注册用户自定义全局快捷键（[shortcuts] 分节；单个失败仅告警）
            register_shortcuts_at_startup(app.handle());

            Ok(())
        })
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 关闭设置/角色窗口时仅隐藏，不退出进程；退出走托盘/菜单 Cmd+Q
                //（菜单退出项须用自定义 MenuItem——原生 quit 会走 terminate: →
                //  windowShouldClose:，被本拦截器取消，见上方菜单构建处注释）。
                api.prevent_close();
                let _ = window.hide();
            }
            // 智能穿透输入缓存：窗口几何事件驱动更新，轮询 tick 只消费缓存值。
            // Moved 同时记录移动时刻（DRAG_HOLD 内视为拖动中，保护期持续顺延）。
            if window.label() == "companion" {
                let state = window.app_handle().state::<CompanionPointerState>();
                match event {
                    WindowEvent::Moved(p) => {
                        *state.origin.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(PhysicalPosition::new(f64::from(p.x), f64::from(p.y)));
                        *state.last_move_at.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(std::time::Instant::now());
                    }
                    WindowEvent::Resized(s) => {
                        *state.size.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(PhysicalSize::new(f64::from(s.width), f64::from(s.height)));
                    }
                    WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                        *state.scale.lock().unwrap_or_else(|e| e.into_inner()) = *scale_factor;
                        let size = window.outer_size().unwrap_or_default();
                        *state.size.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                            PhysicalSize::new(f64::from(size.width), f64::from(size.height)),
                        );
                    }
                    _ => {}
                }
            }
        })
        // RunEvent::Exit 兜底回收 audio.cpp sidecar：覆盖全部退出路径（含未来新增
        // 出口与系统强退前的钩子），与三处显式 shutdown_blocking（幂等）双保险。
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                zapmomo::audiocpp::server::shutdown_blocking();
            }
        });
}

#[cfg(test)]
mod companion_click_through_tests {
    use super::resolve_click_through;
    use zapmomo::config::settings::Live2dSettings;

    #[test]
    fn test_resolve_click_through_missing_defaults_false() {
        assert!(!resolve_click_through(None));
        assert!(!resolve_click_through(Some(&Live2dSettings::default())));
    }

    #[test]
    fn test_resolve_click_through_reads_flag() {
        let on = Live2dSettings {
            click_through: Some(true),
            ..Default::default()
        };
        let off = Live2dSettings {
            click_through: Some(false),
            ..Default::default()
        };
        assert!(resolve_click_through(Some(&on)));
        assert!(!resolve_click_through(Some(&off)));
    }
}

#[cfg(test)]
mod companion_locked_tests {
    use super::resolve_locked;
    use zapmomo::config::settings::Live2dSettings;

    #[test]
    fn test_resolve_locked_missing_defaults_false() {
        assert!(!resolve_locked(None));
        assert!(!resolve_locked(Some(&Live2dSettings::default())));
    }

    #[test]
    fn test_resolve_locked_reads_flag() {
        let on = Live2dSettings {
            locked: Some(true),
            ..Default::default()
        };
        let off = Live2dSettings {
            locked: Some(false),
            ..Default::default()
        };
        assert!(resolve_locked(Some(&on)));
        assert!(!resolve_locked(Some(&off)));
    }
}

#[cfg(test)]
mod chatbox_visible_tests {
    use super::resolve_chatbox_visible;
    use zapmomo::config::settings::ChatboxSettings;

    #[test]
    fn test_resolve_chatbox_visible_missing_defaults_true() {
        // 首次启动（无 [chatbox] 段）：输入条随角色默认显示
        assert!(resolve_chatbox_visible(None));
        assert!(resolve_chatbox_visible(Some(&ChatboxSettings::default())));
    }

    #[test]
    fn test_resolve_chatbox_visible_reads_flag() {
        let on = ChatboxSettings {
            visible: Some(true),
            ..Default::default()
        };
        let off = ChatboxSettings {
            visible: Some(false),
            ..Default::default()
        };
        assert!(resolve_chatbox_visible(Some(&on)));
        assert!(!resolve_chatbox_visible(Some(&off)));
    }
}

#[cfg(test)]
mod companion_opacity_tests {
    use super::{clamp_opacity, opacity_from_id};

    #[test]
    fn test_opacity_from_id_mappings() {
        assert_eq!(opacity_from_id("opacity_100"), Some(1.0));
        assert_eq!(opacity_from_id("opacity_80"), Some(0.8));
        assert_eq!(opacity_from_id("opacity_60"), Some(0.6));
        assert_eq!(opacity_from_id("opacity_40"), Some(0.4));
        assert_eq!(opacity_from_id("opacity_20"), Some(0.2));
        assert_eq!(opacity_from_id("scale_100"), None);
        assert_eq!(opacity_from_id("unknown"), None);
    }

    #[test]
    fn test_clamp_opacity_bounds() {
        assert_eq!(clamp_opacity(0.05), 0.2);
        assert_eq!(clamp_opacity(-1.0), 0.2);
        assert_eq!(clamp_opacity(1.5), 1.0);
        assert_eq!(clamp_opacity(0.2), 0.2);
        assert_eq!(clamp_opacity(1.0), 1.0);
        assert_eq!(clamp_opacity(0.65), 0.65);
    }
}

#[cfg(test)]
mod companion_layout_merge_tests {
    use super::{resolve_position, resolve_scale};
    use zapmomo::companion::CompanionLayout;
    use zapmomo::config::settings::{CompanionWindowPosition, Live2dSettings};

    fn global(scale: Option<f64>, pos: Option<(i32, i32)>) -> Live2dSettings {
        Live2dSettings {
            window_scale: scale,
            window_position: pos.map(|(x, y)| CompanionWindowPosition { x, y }),
            ..Default::default()
        }
    }

    #[test]
    fn test_resolve_scale_private_overrides_global() {
        let layout = CompanionLayout {
            scale: Some(1.5),
            ..Default::default()
        };
        let g = global(Some(0.8), None);
        assert_eq!(resolve_scale(Some(&g), Some(&layout)), 1.5);
    }

    #[test]
    fn test_resolve_scale_falls_back_to_global_then_default() {
        let g = global(Some(0.8), None);
        // 无私有 layout → 全局；全局也缺省 → 1.0。
        assert_eq!(resolve_scale(Some(&g), None), 0.8);
        let empty_layout = CompanionLayout::default();
        assert_eq!(resolve_scale(Some(&g), Some(&empty_layout)), 0.8);
        assert_eq!(resolve_scale(None, None), 1.0);
        assert_eq!(resolve_scale(Some(&Live2dSettings::default()), None), 1.0);
    }

    #[test]
    fn test_resolve_position_private_overrides_global() {
        let layout = CompanionLayout {
            position: Some(CompanionWindowPosition { x: 10, y: 20 }),
            ..Default::default()
        };
        let g = global(None, Some((1, 2)));
        assert_eq!(
            resolve_position(Some(&g), Some(&layout)),
            Some(CompanionWindowPosition { x: 10, y: 20 })
        );
    }

    #[test]
    fn test_resolve_position_falls_back_to_global_then_none() {
        let g = global(None, Some((1, 2)));
        assert_eq!(
            resolve_position(Some(&g), None),
            Some(CompanionWindowPosition { x: 1, y: 2 })
        );
        assert_eq!(resolve_position(None, None), None);
        assert_eq!(
            resolve_position(Some(&Live2dSettings::default()), None),
            None
        );
    }
}

#[cfg(test)]
mod companion_layer_tests {
    use super::{CompanionWindowLayer, layer_from_id};

    #[test]
    fn test_layer_front_is_default() {
        assert_eq!(CompanionWindowLayer::default(), CompanionWindowLayer::Front);
    }

    #[test]
    fn test_layer_from_id() {
        assert_eq!(
            layer_from_id("layer_front"),
            Some(CompanionWindowLayer::Front)
        );
        assert_eq!(
            layer_from_id("layer_back"),
            Some(CompanionWindowLayer::Back)
        );
        assert_eq!(layer_from_id("opacity_100"), None);
        assert_eq!(layer_from_id("unknown"), None);
    }

    #[test]
    fn test_layer_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&CompanionWindowLayer::Front).unwrap(),
            "\"front\""
        );
        assert_eq!(
            serde_json::to_string(&CompanionWindowLayer::Back).unwrap(),
            "\"back\""
        );
        assert_eq!(
            serde_json::from_str::<CompanionWindowLayer>("\"back\"").unwrap(),
            CompanionWindowLayer::Back
        );
        // 未知值应解析失败（避免前端传错静默落到 Front）
        assert!(serde_json::from_str::<CompanionWindowLayer>("\"bogus\"").is_err());
    }
}

#[cfg(test)]
mod companion_menu_tests {
    use super::{companion_id_from_menu_id, companion_menu_entries};
    use zapmomo::companion::CompanionModel;

    fn model(id: &str, name: &str, model_dir: &str) -> CompanionModel {
        CompanionModel {
            id: id.to_string(),
            name: name.to_string(),
            source_path: None,
            model_dir: model_dir.to_string(),
            model_file: format!("{model_dir}/{name}.model3.json"),
            format: "cubism3".to_string(),
            imported_at: "2026-01-01T00:00:00Z".to_string(),
            layout: None,
        }
    }

    #[test]
    fn test_companion_id_from_menu_id_parses_prefix() {
        assert_eq!(
            companion_id_from_menu_id("companion_set_companion-abc123def456"),
            Some("companion-abc123def456")
        );
    }

    #[test]
    fn test_companion_id_from_menu_id_rejects_other_namespaces() {
        assert_eq!(companion_id_from_menu_id("companion_set_"), None);
        assert_eq!(companion_id_from_menu_id("scale_100"), None);
        assert_eq!(companion_id_from_menu_id("open_settings"), None);
        assert_eq!(companion_id_from_menu_id("no_companions"), None);
        assert_eq!(companion_id_from_menu_id(""), None);
    }

    #[test]
    fn test_companion_menu_entries_marks_active_and_invalid() {
        // 目录不存在 → 无效（禁用 + label 追加「（不可用）」）；active 项 checked。
        let models = vec![
            model("companion-aaa", "大月下", "/nonexistent/zapmomo/aaa"),
            model("companion-bbb", "星语", "/nonexistent/zapmomo/bbb"),
        ];
        let entries = companion_menu_entries(&models, Some("companion-bbb"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "companion_set_companion-aaa");
        assert_eq!(entries[0].label, "大月下（不可用）");
        assert!(!entries[0].enabled);
        assert!(!entries[0].checked);
        assert!(entries[1].checked);
    }

    #[test]
    fn test_companion_menu_entries_valid_model_enabled() {
        // quick_valid 只探测目录 + 清单文件存在性：建真实临时目录与空清单文件。
        let dir = std::env::temp_dir().join("zapmomo-companion-menu-test");
        std::fs::create_dir_all(&dir).unwrap();
        let m = model("companion-ccc", "mochi", dir.to_str().unwrap());
        std::fs::write(&m.model_file, "{}").unwrap();
        let entries = companion_menu_entries(&[m], None);
        assert!(entries[0].enabled);
        assert_eq!(entries[0].label, "mochi");
        assert!(!entries[0].checked);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod companion_open_dir_tests {
    use super::resolve_companion_dir;
    use std::path::Path;
    use zapmomo::companion::CompanionModel;

    fn model(id: &str, name: &str, model_dir: &Path) -> CompanionModel {
        let manifest = model_dir.join(format!("{name}.model3.json"));
        CompanionModel {
            id: id.to_string(),
            name: name.to_string(),
            source_path: None,
            model_dir: model_dir.display().to_string(),
            model_file: manifest.display().to_string(),
            format: "cubism3".to_string(),
            imported_at: "2026-01-01T00:00:00Z".to_string(),
            layout: None,
        }
    }

    #[test]
    fn test_resolve_companion_dir_returns_managed_dir() {
        // 托管目录真实存在 → 返回目录路径（交给 open_path 打开）。
        let dir = std::env::temp_dir().join("zapmomo-companion-open-dir-hit");
        std::fs::create_dir_all(&dir).unwrap();
        let m = model("companion-aaa", "大月下", &dir);
        assert_eq!(
            resolve_companion_dir(&[m], "companion-aaa").unwrap(),
            dir.display().to_string()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_resolve_companion_dir_unknown_id_errors() {
        let dir = std::env::temp_dir().join("zapmomo-companion-open-dir-miss");
        std::fs::create_dir_all(&dir).unwrap();
        let m = model("companion-aaa", "大月下", &dir);
        let err = resolve_companion_dir(&[m], "companion-bbb").unwrap_err();
        assert!(err.contains("companion-bbb"), "错误需包含未知 id：{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_resolve_companion_dir_missing_dir_errors() {
        // 托管目录被用户删掉/移动 → 报错而非让文件管理器弹错。
        let missing = Path::new("/nonexistent/zapmomo/aaa");
        let m = model("companion-aaa", "大月下", missing);
        let err = resolve_companion_dir(&[m], "companion-aaa").unwrap_err();
        assert!(err.contains("不存在"), "错误需说明目录缺失：{err}");
    }
}

#[cfg(test)]
mod autostart_tests {
    use super::{autostart_item_labels, is_launched_by_autostart};

    #[test]
    fn test_autostart_flag_hits_at_any_position() {
        // 尾部命中（系统拉起的典型形态：可执行路径 + 插件附加参数）
        assert!(is_launched_by_autostart([
            "/usr/bin/ZapMomo",
            "--autostart"
        ]));
        // 中段命中（未来若再附加其它参数）
        assert!(is_launched_by_autostart([
            "/Applications/ZapMomo.app/Contents/MacOS/ZapMomo",
            "--autostart",
            "--other"
        ]));
    }

    #[test]
    fn test_autostart_flag_requires_exact_match() {
        // 空命令行 / 仅可执行路径
        assert!(!is_launched_by_autostart(Vec::<String>::new()));
        assert!(!is_launched_by_autostart(["target/debug/ZapMomo"]));
        // 前缀 / 去杠 / 赋值变体均不命中（精确匹配，避免误吞用户显式参数）
        assert!(!is_launched_by_autostart(["--autostart-x"]));
        assert!(!is_launched_by_autostart(["autostart"]));
        assert!(!is_launched_by_autostart(["--autostart=1"]));
    }

    #[test]
    fn test_autostart_item_labels_flip_by_state() {
        assert_eq!(
            autostart_item_labels(false),
            ("enable_autostart", "开机自启动")
        );
        assert_eq!(
            autostart_item_labels(true),
            ("disable_autostart", "关闭开机自启动")
        );
    }
}

#[cfg(test)]
mod preflight_tests {
    use super::collect_asr_preflight_files;
    use std::path::PathBuf;
    use zapmomo::asr::config::{AsrModelKind, ResolvedAsrConfig};

    fn cfg_with(kind: AsrModelKind) -> ResolvedAsrConfig {
        let base = PathBuf::from("/models");
        ResolvedAsrConfig {
            model_type: kind,
            model_dir: base.clone(),
            model: Some(base.join("model.onnx")),
            encoder: base.join("encoder.onnx"),
            decoder: base.join("decoder.onnx"),
            joiner: base.join("joiner.onnx"),
            tokens: base.join("tokens.txt"),
            ..ResolvedAsrConfig::default()
        }
    }

    fn labels(files: &[(&'static str, &std::path::Path)]) -> Vec<&'static str> {
        files.iter().map(|(l, _)| *l).collect()
    }

    #[test]
    fn test_preflight_files_zipformer_includes_joiner() {
        let cfg = cfg_with(AsrModelKind::Zipformer);
        let files = collect_asr_preflight_files(&cfg).unwrap();
        let labels = labels(&files);
        assert!(labels.contains(&"ASR joiner"));
        assert!(labels.contains(&"ASR encoder"));
        assert!(labels.contains(&"ASR tokens"));
    }

    #[test]
    fn test_preflight_files_sensevoice_no_joiner() {
        let cfg = cfg_with(AsrModelKind::SenseVoice);
        let files = collect_asr_preflight_files(&cfg).unwrap();
        let labels = labels(&files);
        assert!(labels.contains(&"ASR model"));
        assert!(labels.contains(&"ASR tokens"));
        assert!(!labels.contains(&"ASR joiner"));
        assert!(!labels.contains(&"ASR encoder"));
    }

    #[test]
    fn test_preflight_files_whisper_no_joiner() {
        let cfg = cfg_with(AsrModelKind::Whisper);
        let files = collect_asr_preflight_files(&cfg).unwrap();
        let labels = labels(&files);
        assert!(labels.contains(&"ASR encoder"));
        assert!(labels.contains(&"ASR decoder"));
        assert!(labels.contains(&"ASR tokens"));
        assert!(!labels.contains(&"ASR joiner"));
    }

    #[test]
    fn test_preflight_files_qwen3_no_joiner_tokenizer_as_dir() {
        // 手建临时目录（src-tauri 无 tempfile 依赖）
        let dir =
            std::env::temp_dir().join(format!("zapmomo-preflight-qwen3-{}", std::process::id()));
        let tokenizer = dir.join("tokenizer");
        std::fs::create_dir_all(&tokenizer).unwrap();

        // tokenizer 目录存在 → conv_frontend/encoder/decoder 三件（无 joiner/tokens 标签）
        let cfg = ResolvedAsrConfig {
            model_type: AsrModelKind::Qwen3Asr,
            model_dir: dir.clone(),
            model: Some(dir.join("conv_frontend.onnx")),
            encoder: dir.join("encoder.int8.onnx"),
            decoder: dir.join("decoder.int8.onnx"),
            tokens: tokenizer.clone(),
            ..ResolvedAsrConfig::default()
        };
        let files = collect_asr_preflight_files(&cfg).unwrap();
        let labels = labels(&files);
        assert!(labels.contains(&"ASR conv_frontend"));
        assert!(labels.contains(&"ASR encoder"));
        assert!(labels.contains(&"ASR decoder"));
        assert!(!labels.contains(&"ASR joiner"));
        assert!(!labels.contains(&"ASR tokens"));

        // tokenizer 目录缺失 → 直接报错（目录级校验，不进 is_file 循环）
        let cfg_bad = ResolvedAsrConfig {
            tokens: dir.join("nonexistent-tokenizer"),
            ..cfg
        };
        let err = collect_asr_preflight_files(&cfg_bad).err().unwrap();
        assert!(err.contains("tokenizer"), "err: {err}");

        std::fs::remove_dir_all(&dir).ok();
    }
}
