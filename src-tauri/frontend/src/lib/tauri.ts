import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  LibraryModel,
  ModelLibraryProgress,
  SetCurrentResult,
  StorageInfo,
  StorageMigrateProgress,
  SystemResources,
} from "@/types/modelLibrary";
import type {
  AppInfo,
  AsrConfigInfo,
  AsrParamsPatch,
  AsrResult,
  CompanionDragMode,
  CompanionLibraryView,
  CompanionSpriteEvent,
  CompanionWindowLayer,
  ConversationRecord,
  DeviceEventPayload,
  DownloadProgress,
  DshBridgeStatus,
  DshConfigInfo,
  DshInstallProgress,
  DshIntegrationInfo,
  DshParamsPatch,
  DshSpeakPayload,
  HitRect,
  ImportCompanionResult,
  KwsConfigInfo,
  KwsParamsPatch,
  KwsResult,
  ListenStopped,
  Live2dConfigInfo,
  Live2dModelInfo,
  LlmConfigInfo,
  LlmFinishReason,
  LlmParamsPatch,
  LlmStatus,
  LlmToken,
  PerformanceScene,
  PerformanceStartedPayload,
  PerformanceStoppedPayload,
  SaveTtsVoiceRequest,
  ShortcutActionId,
  SpeakerConfigInfo,
  SpeakerEnrollResult,
  SpeakerIdentifyResult,
  SpeakerInfo,
  SpeakerParamsPatch,
  TranscribeResult,
  TtsConfigInfo,
  TtsParamsPatch,
  TtsProgress,
  TtsResult,
  TtsVoice,
  VoiceError,
  VoicePlaySentence,
  VoiceReplyFinished,
  VoiceReplySentence,
  VoiceSessionStatePayload,
  VoiceStopped,
  VoiceToken,
  VoiceTranscript,
  VoiceWake,
} from "@/types/tauri";

/** 类型安全的 Tauri command 封装。 */
export const api = {
  getAppInfo: () => invoke<AppInfo>("get_app_info"),
  listDevices: () => invoke<string[]>("list_devices"),
  requestMicPermission: () => invoke<boolean>("request_mic_permission"),
  getKwsConfig: () => invoke<KwsConfigInfo>("get_kws_config"),
  setKwsEnabled: (args: { enabled: boolean }) => invoke<void>("set_kws_enabled", args),
  setKwsCustomKeywords: (args: { keywords: string }) =>
    invoke<void>("set_kws_custom_keywords", args),
  setKwsParams: (args: { params: KwsParamsPatch }) => invoke<void>("set_kws_params", args),
  startListen: (args: { device: string | null; keywords: string | null }) =>
    invoke<void>("start_listen", args),
  stopListen: () => invoke<void>("stop_listen"),
  isListening: () => invoke<boolean>("is_listening"),
  downloadKwsModel: () => invoke<void>("download_kws_model"),
  getMicrophone: () => invoke<string>("get_microphone"),
  setMicrophone: (args: { mic: string }) => invoke<void>("set_microphone", args),
  getAsrConfig: () => invoke<AsrConfigInfo>("get_asr_config"),
  setAsrEnabled: (args: { enabled: boolean }) => invoke<void>("set_asr_enabled", args),
  setAsrParams: (args: { params: AsrParamsPatch }) => invoke<void>("set_asr_params", args),
  startAsrListen: (args: { device: string | null }) => invoke<void>("start_asr_listen", args),
  stopAsrListen: () => invoke<void>("stop_asr_listen"),
  isAsrListening: () => invoke<boolean>("is_asr_listening"),
  startAsrDictate: (args: { device: string | null }) => invoke<void>("start_asr_dictate", args),
  stopAsrDictate: () => invoke<void>("stop_asr_dictate"),
  isAsrDictating: () => invoke<boolean>("is_asr_dictating"),
  downloadAsrModel: () => invoke<void>("download_asr_model"),
  transcribeAudio: (args: { wavPath: string | null }) =>
    invoke<TranscribeResult>("transcribe_audio", args),
  // ---- 声纹识别（Speaker Recognition）----
  getSpeakerConfig: () => invoke<SpeakerConfigInfo>("get_speaker_config"),
  setSpeakerEnabled: (args: { enabled: boolean }) => invoke<void>("set_speaker_enabled", args),
  setSpeakerParams: (args: { params: SpeakerParamsPatch }) =>
    invoke<void>("set_speaker_params", args),
  downloadSpeakerModel: () => invoke<void>("download_speaker_model"),
  /** 录制声纹样本（固定时长，后端 clamp 1~30 秒），返回 wav 路径 */
  recordSpeakerSample: (args: { seconds: number; device: string | null }) =>
    invoke<string>("record_speaker_sample", args),
  /** 恢复录音期间被自动挂起的语音会话/监听（弹窗关闭时调用；幂等） */
  speakerResumeMic: () => invoke<void>("speaker_resume_mic"),
  /** 注册说话人（wavPaths 为录音临时文件或自选 wav；注意 camelCase，snake_case 会被后端静默丢参） */
  speakerEnroll: (args: { speakerId: string; wavPaths: string[] }) =>
    invoke<SpeakerEnrollResult>("speaker_enroll", args),
  listSpeakers: () => invoke<SpeakerInfo[]>("list_speakers"),
  removeSpeaker: (args: { speakerId: string }) => invoke<boolean>("remove_speaker", args),
  /** 对一段 wav 做声纹识别（1:N 测试） */
  speakerIdentifyWav: (args: { wavPath: string }) =>
    invoke<SpeakerIdentifyResult>("speaker_identify_wav", args),
  getLive2dConfig: () => invoke<Live2dConfigInfo>("get_live2d_config"),
  listCompanions: () => invoke<CompanionLibraryView>("list_companions"),
  // Tauri v2 命令参数默认 camelCase（`source` 单字段名两端一致，无映射）。
  // 源可以是 Live2D 模型目录或 GIF 动图文件（后端 import_source 分派）。
  importCompanion: (args: { source: string }) =>
    invoke<ImportCompanionResult>("import_companion", args),
  setActiveCompanion: (args: { id: string }) =>
    invoke<CompanionLibraryView>("set_active_companion", args),
  renameCompanion: (args: { id: string; name: string }) =>
    invoke<CompanionLibraryView>("rename_companion", args),
  removeCompanion: (args: { id: string }) => invoke<CompanionLibraryView>("remove_companion", args),
  /** 绑定/解绑伙伴音色（voiceId 传 null 解绑）；注意 camelCase，snake_case 会被后端静默丢参 */
  setCompanionVoice: (args: { id: string; voiceId: string | null }) =>
    invoke<CompanionLibraryView>("set_companion_voice", args),
  /** 音色库全量自定义音色（模型无关，供伙伴页音色绑定选择器；区别于按 TTS 模型过滤的 listTtsVoices） */
  listVoiceLibrary: () => invoke<TtsVoice[]>("list_voice_library"),
  /** 在文件管理器中打开伙伴的托管资产目录（可自行调整音色参考等资产）。 */
  openCompanionDir: (args: { id: string }) => invoke<void>("open_companion_dir", args),
  saveCoverImage: (args: { id: string; png: number[] }) =>
    invoke<CompanionLibraryView>("save_cover_image", args),
  getTtsConfig: () => invoke<TtsConfigInfo>("get_tts_config"),
  listTtsVoices: () => invoke<TtsVoice[]>("list_tts_voices"),
  saveTtsVoice: (args: SaveTtsVoiceRequest) => invoke<TtsVoice>("save_tts_voice", args),
  deleteTtsVoice: (args: { id: string }) => invoke<void>("delete_tts_voice", args),
  recordTtsVoice: (args: { seconds: number; device: string | null }) =>
    invoke<string>("record_tts_voice", args),
  transcribeReferenceAudio: (args: { wavPath: string }) =>
    invoke<string>("transcribe_reference_audio", args),
  synthesizeTts: (args: {
    text: string;
    speed: number | null;
    sid: number | null;
    voice: string | null;
    referenceWav: string | null;
    referenceText: string | null;
  }) => invoke<void>("synthesize_tts", args),
  stopTts: () => invoke<void>("stop_tts"),
  isTtsSynthesizing: () => invoke<boolean>("is_tts_synthesizing"),
  setTtsEnabled: (args: { enabled: boolean }) => invoke<void>("set_tts_enabled", args),
  setTtsParams: (args: { params: TtsParamsPatch }) => invoke<void>("set_tts_params", args),
  setTtsVoice: (voice: string | null) => invoke<void>("set_tts_voice", { voice }),
  /** 切换 TTS 推理后端（sherpa/audiocpp）；常规入口是「选择模型」弹窗的设为当前 */
  setTtsBackend: (backend: string) => invoke<void>("set_tts_backend", { backend }),
  getLlmConfig: () => invoke<LlmConfigInfo>("get_llm_config"),
  loadLlmModel: () => invoke<void>("load_llm_model"),
  unloadLlmModel: () => invoke<void>("unload_llm_model"),
  chatLlm: (args: { text: string }) => invoke<void>("chat_llm", args),
  stopLlm: () => invoke<void>("stop_llm"),
  isLlmReady: () => invoke<boolean>("is_llm_ready"),
  /** 保存远程 LLM 连接配置（base_url/api_key/model）；apiKey 为空串时清空，不传则保持不变 */
  setLlmConnection: (args: { baseUrl: string; apiKey?: string | null; model: string }) =>
    invoke<void>("set_llm_connection", args),
  setLlmSystemPrompt: (args: { prompt: string }) => invoke<void>("set_llm_system_prompt", args),
  /** 批量保存 LLM 参数补丁（含 thinking / reasoning_effort）；None 字段保持不变 */
  setLlmParams: (args: { params: LlmParamsPatch }) => invoke<void>("set_llm_params", args),
  // ---- 语音会话（KWS→ASR→LLM→TTS 全链路）----
  startVoiceSession: () => invoke<void>("start_voice_session"),
  stopVoiceSession: () => invoke<void>("stop_voice_session"),
  isVoiceSessionRunning: () => invoke<boolean>("is_voice_session_running"),
  // ---- 文字输入条（chatbox 窗口）----
  sendVoiceText: (args: { text: string }) => invoke<void>("send_voice_text", args),
  saveChatboxPosition: (args: { x: number; y: number }) =>
    invoke<void>("save_chatbox_position", args),
  hideChatbox: () => invoke<void>("hide_chatbox"),
  // ---- 语音回复气泡（bubble 窗口）----
  saveBubblePosition: (args: { x: number; y: number }) =>
    invoke<void>("save_bubble_position", args),
  /** （临时调试）气泡窗口状态日志，验收后删除 */
  bubbleDebugLog: (args: { message: string }) => invoke<void>("bubble_debug_log", args),
  // ---- 对话记录（~/.zapmomo/conversations.json）----
  getConversationRecords: () => invoke<ConversationRecord[]>("get_conversation_records"),
  clearConversationRecords: () => invoke<void>("clear_conversation_records"),
  // ---- dsh 桥（deepseek-harness 任务事件 → 桌宠说话）----
  getDshConfig: () => invoke<DshConfigInfo>("get_dsh_config"),
  setDshEnabled: (args: { enabled: boolean }) => invoke<void>("set_dsh_enabled", args),
  setDshParams: (args: { params: DshParamsPatch }) => invoke<void>("set_dsh_params", args),
  getDshBridgeStatus: () => invoke<DshBridgeStatus>("get_dsh_bridge_status"),
  testDshAnnounce: () => invoke<void>("test_dsh_announce"),
  // ---- dsh 集成（插件检测 / 一键安装；「插件集成」页）----
  detectDshIntegration: () => invoke<DshIntegrationInfo>("detect_dsh_integration"),
  installDshPlugin: (args: { path?: string | null }) => invoke<void>("install_dsh_plugin", args),
  // ---- 模型列表（registry 预设 + 安装状态；供各「选择模型」弹窗）----
  listModelLibrary: () => invoke<LibraryModel[]>("list_model_library"),
  getSystemResources: () => invoke<SystemResources>("get_system_resources"),
  // ---- 存储位置（数据目录）----
  getStorageInfo: () => invoke<StorageInfo>("get_storage_info"),
  setStorageDir: (args: { path: string | null }) => invoke<StorageInfo>("set_data_dir", args),
  migrateStorage: () => invoke<void>("migrate_storage"),
  cancelStorageMigration: () => invoke<void>("cancel_storage_migration"),
  openStorageDir: () => invoke<void>("open_storage_dir"),
  downloadLibraryModel: (args: { id: string }) => invoke<void>("download_library_model", args),
  cancelModelDownload: () => invoke<void>("cancel_model_download"),
  setCurrentModel: (args: { id: string }) => invoke<SetCurrentResult>("set_current_model", args),
  deleteModel: (args: { id: string }) => invoke<void>("delete_model", args),
  saveCompanionPosition: (args: { x: number; y: number }) =>
    invoke<void>("save_companion_position", args),
  setCompanionScale: (args: { scale: number }) => invoke<void>("set_companion_scale", args),
  setCompanionOpacity: (args: { opacity: number }) => invoke<void>("set_companion_opacity", args),
  setCompanionClickThrough: (args: { enabled: boolean }) =>
    invoke<void>("set_companion_click_through", args),
  setCompanionSmartClickThrough: (args: { enabled: boolean }) =>
    invoke<void>("set_companion_smart_click_through", args),
  setCompanionHitRegion: (args: { rects: HitRect[] }) =>
    invoke<void>("set_companion_hit_region", args),
  setCompanionLayer: (args: { layer: CompanionWindowLayer }) =>
    invoke<void>("set_companion_layer", args),
  setCompanionLocked: (args: { enabled: boolean }) => invoke<void>("set_companion_locked", args),
  setCompanionDragMode: (args: { mode: CompanionDragMode }) =>
    invoke<void>("set_companion_drag_mode", args),
  showCompanionMenu: (args: { x: number; y: number }) => invoke<void>("show_companion_menu", args),
  startPerformance: (args: { scene: PerformanceScene }) => invoke<void>("start_performance", args),
  stopPerformance: () => invoke<void>("stop_performance"),
  isPerforming: () => invoke<PerformanceScene | null>("is_performing"),
  getHideDockIcon: () => invoke<boolean>("get_hide_dock_icon"),
  setHideDockIcon: (args: { hide: boolean }) => invoke<void>("set_hide_dock_icon", args),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (args: { enabled: boolean }) => invoke<void>("set_autostart", args),
  getShortcuts: () => invoke<Record<string, string>>("get_shortcuts"),
  setShortcut: (args: { action: ShortcutActionId; accelerator: string }) =>
    invoke<void>("set_shortcut", args),
  clearShortcut: (args: { action: ShortcutActionId }) => invoke<void>("clear_shortcut", args),
  openSettings: () => invoke<void>("open_settings"),
  hideCompanion: () => invoke<void>("hide_companion"),
  quitApp: () => invoke<void>("quit_app"),
  restartApp: () => invoke<void>("restart_app"),
};

/** 类型安全的事件订阅（返回的 Promise resolve 后得到取消订阅函数）。 */
export function onKeywordDetected(handler: (result: KwsResult) => void): Promise<UnlistenFn> {
  return listen<KwsResult>("kws-detected", (e) => handler(e.payload));
}

export function onListenStopped(handler: (payload: ListenStopped) => void): Promise<UnlistenFn> {
  return listen<ListenStopped>("kws-stopped", (e) => handler(e.payload));
}

export function onListenStarted(handler: (payload: ListenStopped) => void): Promise<UnlistenFn> {
  return listen<ListenStopped>("kws-started", (e) => handler(e.payload));
}

export function onDownloadProgress(
  handler: (payload: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>("kws-model-download-progress", (e) => handler(e.payload));
}

export function onSpeakerModelDownloadProgress(
  handler: (payload: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>("speaker-model-download-progress", (e) => handler(e.payload));
}

export function onAsrResult(handler: (result: AsrResult) => void): Promise<UnlistenFn> {
  return listen<AsrResult>("asr-result", (e) => handler(e.payload));
}

export function onAsrStopped(handler: (payload: ListenStopped) => void): Promise<UnlistenFn> {
  return listen<ListenStopped>("asr-stopped", (e) => handler(e.payload));
}

export function onAsrStarted(handler: (payload: ListenStopped) => void): Promise<UnlistenFn> {
  return listen<ListenStopped>("asr-started", (e) => handler(e.payload));
}

export function onAsrDownloadProgress(
  handler: (payload: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>("asr-model-download-progress", (e) => handler(e.payload));
}

export function onAsrVadDownloadProgress(
  handler: (payload: DownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<DownloadProgress>("asr-vad-download-progress", (e) => handler(e.payload));
}

export function onAsrDictateResult(handler: (result: AsrResult) => void): Promise<UnlistenFn> {
  return listen<AsrResult>("asr-dictate-result", (e) => handler(e.payload));
}

export function onAsrDictateStarted(
  handler: (payload: ListenStopped) => void,
): Promise<UnlistenFn> {
  return listen<ListenStopped>("asr-dictate-started", (e) => handler(e.payload));
}

export function onAsrDictateStopped(
  handler: (payload: ListenStopped) => void,
): Promise<UnlistenFn> {
  return listen<ListenStopped>("asr-dictate-stopped", (e) => handler(e.payload));
}

export function onTtsResult(handler: (result: TtsResult) => void): Promise<UnlistenFn> {
  return listen<TtsResult>("tts-result", (e) => handler(e.payload));
}

export function onTtsProgress(handler: (p: TtsProgress) => void): Promise<UnlistenFn> {
  return listen<TtsProgress>("tts-progress", (e) => handler(e.payload));
}

export function onTtsStopped(handler: (payload: ListenStopped) => void): Promise<UnlistenFn> {
  return listen<ListenStopped>("tts-stopped", (e) => handler(e.payload));
}

export function onLive2dModelChanged(
  handler: (info: Live2dModelInfo) => void,
): Promise<UnlistenFn> {
  return listen<Live2dModelInfo>("live2d-model-changed", (e) => handler(e.payload));
}

/** `companion-sprite-changed`：LLM 工具切换角色形象（path 为图片绝对路径，default = 默认立绘） */
export function onCompanionSpriteChanged(
  handler: (ev: CompanionSpriteEvent) => void,
): Promise<UnlistenFn> {
  return listen<CompanionSpriteEvent>("companion-sprite-changed", (e) => handler(e.payload));
}

export function onCompanionScaleChanged(handler: (scale: number) => void): Promise<UnlistenFn> {
  return listen<number>("companion-scale-changed", (e) => handler(e.payload));
}

export function onCompanionOpacityChanged(handler: (opacity: number) => void): Promise<UnlistenFn> {
  return listen<number>("companion-opacity-changed", (e) => handler(e.payload));
}

export function onCompanionLayerChanged(
  handler: (layer: CompanionWindowLayer) => void,
): Promise<UnlistenFn> {
  return listen<CompanionWindowLayer>("companion-layer-changed", (e) => handler(e.payload));
}

/** 智能穿透开关变化（设置页 / 托盘菜单切换时广播）。 */
export function onCompanionSmartClickThroughChanged(
  handler: (enabled: boolean) => void,
): Promise<UnlistenFn> {
  return listen<boolean>("companion-smart-click-through-changed", (e) => handler(e.payload));
}

export function onCompanionLockedChanged(handler: (locked: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("companion-locked-changed", (e) => handler(e.payload));
}

export function onCompanionDragModeChanged(
  handler: (mode: CompanionDragMode) => void,
): Promise<UnlistenFn> {
  return listen<CompanionDragMode>("companion-drag-mode-changed", (e) => handler(e.payload));
}

/** 表演开始（桌宠窗口为唯一订阅者）。 */
export function onPerformanceStarted(
  handler: (payload: PerformanceStartedPayload) => void,
): Promise<UnlistenFn> {
  return listen<PerformanceStartedPayload>("performance-started", (e) => handler(e.payload));
}

/** 表演停止（桌宠窗口为唯一订阅者）。 */
export function onPerformanceStopped(
  handler: (payload: PerformanceStoppedPayload) => void,
): Promise<UnlistenFn> {
  return listen<PerformanceStoppedPayload>("performance-stopped", (e) => handler(e.payload));
}

/** 模拟键鼠事件流（与 BongoCat device-changed 逐字节同构；桌宠窗口为唯一订阅者）。 */
export function onDeviceChanged(
  handler: (payload: DeviceEventPayload) => void,
): Promise<UnlistenFn> {
  return listen<DeviceEventPayload>("device-changed", (e) => handler(e.payload));
}

/** 开机自启动状态变化（设置页为唯一订阅者：托盘菜单改动后同步开关）。 */
export function onAutostartChanged(handler: (enabled: boolean) => void): Promise<UnlistenFn> {
  return listen<boolean>("autostart-changed", (e) => handler(e.payload));
}

export function onModelLibraryDownloadProgress(
  handler: (p: ModelLibraryProgress) => void,
): Promise<UnlistenFn> {
  return listen<ModelLibraryProgress>("model-library-download-progress", (e) => handler(e.payload));
}

/** 存储迁移进度（`storage-migrate-progress`）。 */
export function onStorageMigrateProgress(
  handler: (p: StorageMigrateProgress) => void,
): Promise<UnlistenFn> {
  return listen<StorageMigrateProgress>("storage-migrate-progress", (e) => handler(e.payload));
}

export function onLlmToken(handler: (delta: LlmToken) => void): Promise<UnlistenFn> {
  return listen<LlmToken>("llm-token", (e) => handler(e.payload));
}

export function onLlmFinished(handler: (reason: LlmFinishReason) => void): Promise<UnlistenFn> {
  return listen<LlmFinishReason>("llm-finished", (e) => handler(e.payload));
}

export function onLlmError(handler: (error: string) => void): Promise<UnlistenFn> {
  return listen<string>("llm-error", (e) => handler(e.payload));
}

export function onLlmStatus(handler: (status: LlmStatus) => void): Promise<UnlistenFn> {
  return listen<LlmStatus>("llm-status", (e) => handler(e.payload));
}

// ---- 语音会话事件 ----

export function onVoiceSessionState(
  handler: (payload: VoiceSessionStatePayload) => void,
): Promise<UnlistenFn> {
  return listen<VoiceSessionStatePayload>("voice-session-state", (e) => handler(e.payload));
}

export function onVoiceSessionWake(handler: (payload: VoiceWake) => void): Promise<UnlistenFn> {
  return listen<VoiceWake>("voice-session-wake", (e) => handler(e.payload));
}

export function onVoiceSessionTranscript(
  handler: (payload: VoiceTranscript) => void,
): Promise<UnlistenFn> {
  return listen<VoiceTranscript>("voice-session-transcript", (e) => handler(e.payload));
}

export function onVoiceSessionToken(handler: (payload: VoiceToken) => void): Promise<UnlistenFn> {
  return listen<VoiceToken>("voice-session-token", (e) => handler(e.payload));
}

export function onVoiceSessionReply(
  handler: (payload: VoiceReplySentence) => void,
): Promise<UnlistenFn> {
  return listen<VoiceReplySentence>("voice-session-reply", (e) => handler(e.payload));
}

export function onVoiceSessionPlay(
  handler: (payload: VoicePlaySentence) => void,
): Promise<UnlistenFn> {
  return listen<VoicePlaySentence>("voice-session-play", (e) => handler(e.payload));
}

export function onVoiceSessionReplyFinished(
  handler: (payload: VoiceReplyFinished) => void,
): Promise<UnlistenFn> {
  return listen<VoiceReplyFinished>("voice-session-reply-finished", (e) => handler(e.payload));
}

export function onVoiceSessionError(handler: (payload: VoiceError) => void): Promise<UnlistenFn> {
  return listen<VoiceError>("voice-session-error", (e) => handler(e.payload));
}

export function onVoiceSessionStopped(
  handler: (payload: VoiceStopped) => void,
): Promise<UnlistenFn> {
  return listen<VoiceStopped>("voice-session-stopped", (e) => handler(e.payload));
}

// ---- dsh 桥事件 ----

export function onDshSpeak(handler: (payload: DshSpeakPayload) => void): Promise<UnlistenFn> {
  return listen<DshSpeakPayload>("dsh-speak", (e) => handler(e.payload));
}

export function onDshBridgeStatus(
  handler: (payload: DshBridgeStatus) => void,
): Promise<UnlistenFn> {
  return listen<DshBridgeStatus>("dsh-bridge-status", (e) => handler(e.payload));
}

/** 订阅 dsh 插件安装进度（一键安装的逐行输出 / 状态翻转）。 */
export function onDshInstallProgress(
  handler: (p: DshInstallProgress) => void,
): Promise<UnlistenFn> {
  return listen<DshInstallProgress>("dsh-install-progress", (e) => handler(e.payload));
}

/**
 * 把本地绝对路径转成 Tauri asset 协议 URL，供 Live2D 运行时加载。
 *
 * 不能直接用 `@tauri-apps/api/core` 的 `convertFileSrc`：它用 `encodeURIComponent`
 * 编码整个路径（含 `/`），导致 URL 的 path 退化成单个段、没有目录结构，Live2D
 * 运行时解析模型清单里的相对路径（如 `xxx.moc3`）时会错误地解析到根目录。
 *
 * 这里改为逐段编码、保留 `/` 分隔符——Tauri 的 asset handler 会「skip leading /」，
 * 去掉一个 `/` 后得到的仍是绝对路径（如 `/Users/...`），从而同时满足
 * 「相对路径正确解析」与「文件正确打开」两个要求。
 *
 * 平台差异（同 convertFileSrc 的规则）：
 * - Windows 的 WebView2 是 Chromium 内核，禁止对自定义 scheme 发跨源请求，
 *   必须用虚拟主机形式 `http://asset.localhost/<path>`（CSP 已放行该来源）；
 * - macOS/Linux 保持 `asset://localhost/<path>`。
 *
 * 另外 Tauri 返回的是原生路径：Windows 用 `\` 分隔，需先归一化为 `/`，
 * 否则整条路径会被编码成单个段（`%5C`），相对资源解析会全部失配。
 */
export function toAssetUrl(path: string): string {
  const isWindows = navigator.userAgent.includes("Windows");
  const segments = path
    .replace(/\\/g, "/")
    .split("/")
    .map((s) => encodeURIComponent(s))
    .join("/");
  return isWindows ? `http://asset.localhost/${segments}` : `asset://localhost/${segments}`;
}
