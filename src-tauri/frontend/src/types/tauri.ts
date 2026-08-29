// Tauri 后端命令 / 事件的类型契约。
// 与 src-tauri/src/lib.rs 的命令签名及 src/kws/reaction.rs 的 KwsResult 一一对应。

/** `get_app_info` 返回 */
export interface AppInfo {
  version: string;
  product_name: string;
}

/** `get_kws_config` 返回 */
export interface KwsConfigInfo {
  /** 是否启用 KWS（打开开关即持久化；下次启动自动监听的前提） */
  enabled: boolean;
  /** 持久化的会话级自定义唤醒词（原始字符串，多个用 / 分隔；空 = 模型内置） */
  custom_keywords: string;
  model_dir: string;
  provider: string;
  num_threads: number;
  sample_rate: number;
  /** 每次喂给模型的采样数（@16k） */
  chunk_size: number;
  /** 关键词 boosting 分数 */
  keywords_score: number;
  /** 触发阈值（灵敏度，0~1） */
  keywords_threshold: number;
  debug: boolean;
  keywords: string[];
  models_present: boolean;
  model_downloading: boolean;
  settings_path: string;
}

/** `set_kws_params` 载荷：可调整的 KWS 引擎/运行参数（snake_case 直传，缺省项不修改）。 */
export interface KwsParamsPatch {
  keywords_threshold?: number;
  keywords_score?: number;
  chunk_size?: number;
  num_threads?: number;
  debug?: boolean;
}

/** `kws-detected` 事件载荷（对应后端 KwsResult） */
export interface KwsResult {
  keyword: string;
  tokens: string;
  tokens_arr: string[];
  timestamps: number[];
  start_time: number;
  json: string;
}

/** `kws-stopped` 事件载荷（正常停止时 error 为 null） */
export interface ListenStopped {
  error: string | null;
}

/** `kws-model-download-progress` / `asr-model-download-progress` 事件载荷 */
export type DownloadStage = "downloading" | "verifying" | "extracting" | "done";

export interface DownloadProgress {
  stage: DownloadStage;
  percent: number;
  message: string;
}

/** `get_asr_config` 返回（含可经 `set_asr_params` 调整的引擎参数） */
export interface AsrConfigInfo {
  enabled: boolean;
  /** 模型类型（zipformer/sensevoice/whisper），决定是否展示流式专属参数 */
  model_type: string;
  /** 推理后端（sherpa/audiocpp）：audiocpp 时显示 audio.cpp 标识并隐藏热词参数 */
  backend: string;
  model_dir: string;
  provider: string;
  num_threads: number;
  sample_rate: number;
  chunk_size: number;
  decoding_method: string;
  enable_endpoint: boolean;
  rule1_min_trailing_silence: number;
  rule2_min_trailing_silence: number;
  rule3_min_utterance_length: number;
  blank_penalty: number;
  hotwords: string | null;
  enable_punctuation: boolean;
  debug: boolean;
  models_present: boolean;
  punctuation_present: boolean;
  /** Silero VAD 模型是否已就绪（离线听写首次启动会自动下载） */
  vad_present: boolean;
  model_downloading: boolean;
  settings_path: string;
}

/** `set_asr_params` 载荷：可调整的 ASR 引擎/运行参数（snake_case 直传，缺省项不修改）。 */
export interface AsrParamsPatch {
  num_threads?: number;
  chunk_size?: number;
  enable_endpoint?: boolean;
  rule1_min_trailing_silence?: number;
  rule2_min_trailing_silence?: number;
  rule3_min_utterance_length?: number;
  blank_penalty?: number;
  hotwords?: string;
  enable_punctuation?: boolean;
  language?: string;
  use_itn?: boolean;
  debug?: boolean;
}

/** `transcribe_audio` 返回（snake_case 直传） */
export interface TranscribeResult {
  text: string;
  model_type: string;
  model_dir: string;
}

/** `asr-result` 事件载荷（对应后端 AsrResult） */
export interface AsrResult {
  text: string;
  tokens: string[];
  timestamps: number[] | null;
  start_time: number | null;
  is_final: boolean;
}

/** 角色窗口显示层级：置顶（front）悬浮在所有窗口之上 / 置底（back）沉到窗口之下并点穿 */
export type CompanionWindowLayer = "front" | "back";

/** 角色窗口拖拽模式：direct（左键直接拖动，默认）/ modifier（需按住 cmd/Ctrl） */
export type CompanionDragMode = "direct" | "modifier";

/**
 * 角色窗口可命中矩形（窗口内逻辑像素，原点 = 窗口左上角）。
 * 与 Rust 侧 `zapmomo::companion_click_through::HitRect` 一一对应；
 * 智能穿透用它判定光标是否落在角色画面上。
 */
export interface HitRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** BongoCat 表演道具资源（非 BongoCat 模型为 null） */
export interface PerformancePropsInfo {
  /** 键盘背景图绝对路径（resources/background.png） */
  background: string | null;
  /** 按键贴图清单（爪子按在某键上的预渲染图） */
  keys: PerformanceKeyInfo[];
}

export interface PerformanceKeyInfo {
  /** 键名（如 KeyA、CapsLock） */
  key: string;
  /** 贴图绝对路径 */
  path: string;
  /** 所属的手：left / right */
  hand: "left" | "right";
}

/** `get_live2d_config` 返回 */
export interface Live2dConfigInfo {
  model_dir: string | null;
  model_file: string | null;
  /** 模型格式："cubism3"（Live2D）/ "gif"（GIF 伙伴）/ "character"（角色包） */
  format: string | null;
  models_present: boolean;
  window_scale: number | null;
  window_opacity: number | null;
  click_through: boolean | null;
  /** 智能穿透（null = 旧后端未返回，视为开启——其缺省值） */
  smart_click_through: boolean | null;
  window_layer: CompanionWindowLayer | null;
  /** 位置锁定（禁止拖动窗口；null = 旧后端未返回，视为未锁定） */
  locked: boolean | null;
  /** 拖拽模式（null = 旧后端未返回，视为 direct） */
  drag_mode: CompanionDragMode | null;
  settings_path: string;
  /** BongoCat 道具资源（非 BongoCat 模型为 null） */
  props: PerformancePropsInfo | null;
}

/** `live2d-model-changed` 事件载荷（切换伙伴 / 清屏；清屏时三字段均为 null） */
export interface Live2dModelInfo {
  model_dir: string | null;
  model_file: string | null;
  /** "cubism3"（Live2D）/ "gif"（GIF 伙伴）/ "character"（角色包） */
  format: string | null;
  /** BongoCat 道具资源（非 BongoCat 模型为 null） */
  props: PerformancePropsInfo | null;
  /** 该伙伴的私有缩放（null/缺字段 = 未单独配置，窗口沿用当前尺寸） */
  window_scale?: number | null;
  /** 该伙伴的私有窗口位置（逻辑像素；null/缺字段 = 未配置或已落屏外，窗口沿用当前位置） */
  window_position?: { x: number; y: number } | null;
}

/** `companion-sprite-changed` 事件载荷（LLM 工具切换角色形象；形象是会话态，不持久化） */
export interface CompanionSpriteEvent {
  /** 事件所属伙伴 id（前端以路径前缀校验归属，此字段供诊断/未来使用） */
  companion_id: string;
  /** 形象名（sprites/ 文件名 stem）；"default" = 恢复默认立绘 */
  name: string;
  /** 图片绝对路径（default 时为该伙伴的 character.png） */
  path: string;
}

// ---- 表演（BongoCat 兼容模拟键鼠）----

/** 表演场景（"both" = 键鼠同动，同时驱动键盘 + 鼠标两个通道） */
export type PerformanceScene = "typing" | "mouse" | "both";

/** `performance-started` 事件载荷（含鼠标通道时带 play_area） */
export type PerformanceStartedPayload =
  | { scene: "typing" }
  | { scene: "mouse" | "both"; play_area: { x: number; y: number; width: number; height: number } };

/** `performance-stopped` 事件载荷 */
export interface PerformanceStoppedPayload {
  scene: PerformanceScene;
}

/** `device-changed` 事件载荷（与 BongoCat device.rs 逐字节同构） */
export type DeviceEventPayload =
  | { kind: "KeyboardPress" | "KeyboardRelease"; value: string }
  | { kind: "MousePress" | "MouseRelease"; value: string }
  | { kind: "MouseMove"; value: { x: number; y: number } };

/** `list_companions` / `set_active_companion` 里的单个伙伴 */
export interface CompanionModelInfo {
  id: string;
  name: string;
  /** 原始导入目录（仅记录来源；运行时不依赖，源删除后伙伴仍有效） */
  source_path: string | null;
  /** 应用托管目录 `~/.zapmomo/companions/{id}` */
  model_dir: string;
  /** 托管目录内的 .model3.json（Live2D）/ .gif（GIF 伙伴）/ character.png（角色包）绝对路径 */
  model_file: string;
  /** "cubism3"（Live2D）/ "gif"（GIF 伙伴）/ "character"（角色包） */
  format: string;
  imported_at: string;
  /** 快速有效判定：托管目录与清单文件是否都还在磁盘上 */
  valid: boolean;
  /** 探测到的封面图绝对路径（无封面图为 null，列表用占位图标） */
  cover_image: string | null;
  /** 角色包是否带人设（character.md 非空；非角色包恒 false） */
  has_persona: boolean;
  /** 角色包是否带音色克隆参考（voice/reference.wav + reference.txt 成对） */
  has_voice: boolean;
}

/** `list_companions` / `set_active_companion` 返回的伙伴库视图 */
export interface CompanionLibraryView {
  models: CompanionModelInfo[];
  active_model_id: string | null;
}

/** `import_companion` 返回 */
export interface ImportCompanionResult {
  library: CompanionLibraryView;
  model_id: string;
  already_imported: boolean;
}

/** `get_tts_config` 返回 */
export interface TtsConfigInfo {
  /** 模型类型（zipvoice/omnivoice/...），前端据此切换音色语义 */
  model_type: string;
  /** 推理后端（sherpa/audiocpp），前端据此显示引擎徽标 */
  backend: string;
  model_dir: string;
  provider: string;
  num_threads: number;
  enabled: boolean;
  models_present: boolean;
  model_downloading: boolean;
  settings_path: string;
  /** 扩散解码步数（质量/速度权衡），可经 `set_tts_params` 修改 */
  num_steps: number;
  /** 默认语速，可经 `set_tts_params` 修改 */
  speed: number;
  /** 调试输出，可经 `set_tts_params` 修改 */
  debug: boolean;
  /** 默认音色 id（`null` = 内置 leijun），可经 `set_tts_voice` 修改 */
  voice: string | null;
}

/** `set_tts_params` 载荷：可调整的 TTS 合成参数（snake_case 直传，缺省项不修改）。 */
export interface TtsParamsPatch {
  num_steps?: number;
  speed?: number;
  num_threads?: number;
  debug?: boolean;
}

/** `tts-result` 事件载荷（对应后端 TtsResult） */
export interface TtsResult {
  path: string;
  duration: number;
  sample_rate: number;
}

/** `tts-progress` 事件载荷（对应后端 TtsProgress） */
export interface TtsProgress {
  percent: number;
}

/** `list_tts_voices` 返回的音色（对应后端 TtsVoice） */
export interface TtsVoice {
  id: string;
  name: string;
  wav_path: string;
  reference_text: string;
  /** 是否为用户自定义音色（true = 来自音色库，false = 模型包内置） */
  custom: boolean;
}

/** `save_tts_voice` 载荷：把源 wav 拷贝进音色库并登记。 */
export type SaveTtsVoiceRequest = {
  name: string;
  sourceWavPath: string;
  referenceText: string;
};

/** 解析后的 LLM 采样参数（对应后端 GenParams，snake_case 直传）。 */
export interface LlmParams {
  max_tokens: number;
  temperature: number;
  top_p: number;
  top_k: number;
  min_p: number;
  repeat_penalty: number;
  seed: number;
}

/** `set_llm_params` 载荷（参数补丁；未传字段保持不变） */
export interface LlmParamsPatch {
  thinking?: boolean;
  reasoning_effort?: string;
}

/** `get_llm_config` 返回 */
export interface LlmConfigInfo {
  enabled: boolean;
  provider: string;
  ready: boolean;
  settings_path: string;
  /** 当前生效的角色 system prompt */
  system_prompt: string;
  /** 当前生效的采样参数（已 resolve） */
  params: LlmParams;
  /** OpenAI 兼容接口地址 */
  base_url: string | null;
  /** 完整 API Key（本机桌面应用；前端默认 password 圆点展示，小眼睛切换明文） */
  api_key: string | null;
  /** 模型名（如 glm-4.7-flash） */
  model: string | null;
  /** 是否启用思考（已 resolve 缺省推断） */
  thinking: boolean;
  /** 思考力度（thinking 关闭时保留原值但运行时忽略） */
  reasoning_effort: string | null;
}

/** `llm-token` 事件载荷（对应后端 TokenDelta） */
export interface LlmToken {
  text: string;
  is_final: boolean;
}

/** `llm-finished` 事件载荷（对应后端 FinishReason，序列化为小写） */
export type LlmFinishReason = "eos" | "max_tokens" | "cancelled" | "error";

/** `llm-status` 事件载荷（对应后端 LlmStatusPayload） */
export interface LlmStatus {
  ready: boolean;
}

// ---- 语音会话（KWS→ASR→LLM→TTS 全链路）----

/** `voice-session-state` 事件的会话阶段 */
export type VoiceSessionPhase =
  | "idle"
  | "armed"
  | "greeting"
  | "waiting_speech"
  | "listening"
  | "thinking"
  | "speaking";

/** `voice-session-state` 事件载荷 */
export interface VoiceSessionStatePayload {
  running: boolean;
  state: VoiceSessionPhase;
}

/** `voice-session-wake` 事件载荷 */
export interface VoiceWake {
  keyword: string;
}

/** `voice-session-transcript` 事件载荷（ASR 实时字幕） */
export interface VoiceTranscript {
  text: string;
  is_final: boolean;
}

/** `voice-session-token` 事件载荷（LLM 流式增量） */
export interface VoiceToken {
  delta: string;
}

/** `voice-session-reply` 事件载荷（切句入队合成） */
export interface VoiceReplySentence {
  sentence: string;
}

/** `voice-session-play` 事件载荷（正在播报的句子） */
export interface VoicePlaySentence {
  sentence: string;
}

/** `voice-session-reply-finished` 事件载荷 */
export interface VoiceReplyFinished {
  reason: string;
  /** 该轮完整可见回复（`null` = 空回复），供前端提交对话记录 */
  text: string | null;
}

/** 一条持久化的对话记录（`~/.zapmomo/conversations.json`） */
export interface ConversationRecord {
  role: "user" | "assistant";
  text: string;
  /** ISO 8601 时间戳 */
  at: string;
}

/** `voice-session-error` 事件载荷 */
export interface VoiceError {
  message: string;
}

/** `voice-session-stopped` 事件载荷 */
export interface VoiceStopped {
  error: string | null;
}

// ---- 全局快捷键 ----

/** 可绑定全局快捷键的操作标识（与 Rust `ShortcutAction::as_str` 一致）。 */
export type ShortcutActionId =
  | "toggle_companion"
  | "toggle_voice_session"
  | "interrupt_reply"
  | "open_settings";

// ---- dsh 桥（deepseek-harness 任务事件 → 桌宠说话）----

/** dsh 任务事件（后端 DshEvent 序列化；type 为 kebab-case 判别字段） */
export interface DshEventInfo {
  type: "task-started" | "task-finished" | "task-failed" | "task-interrupted";
  session_id: string;
  title?: string | null;
  reason?: string | null;
  detail?: string | null;
}

/** `dsh-speak` 事件载荷（气泡台词 + 原始事件） */
export interface DshSpeakPayload {
  text: string;
  event: DshEventInfo;
}

/** `dsh-bridge-status` 事件载荷 / `get_dsh_bridge_status` 返回 */
export interface DshBridgeStatus {
  running: boolean;
  port: number | null;
  error: string | null;
}

/** `get_dsh_config` 返回 */
export interface DshConfigInfo {
  enabled: boolean;
  port: number;
  voice_enabled: boolean;
  llm_enabled: boolean;
  record_to_history: boolean;
  running: boolean;
  actual_port: number | null;
  /** 最近一次桥线程错误（启动失败/退出异常；null = 正常），设置页展示 */
  error: string | null;
  discovery_path: string;
}

/** `set_dsh_params` 载荷（snake_case 直传，缺省项不修改） */
export interface DshParamsPatch {
  voice_enabled?: boolean;
  llm_enabled?: boolean;
  record_to_history?: boolean;
  port?: number;
}
