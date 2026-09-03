/// audio.cpp sidecar 集成（TTS/ASR 第二后端）。
///
/// audio.cpp（Apache-2.0，ggml 系）作为独立进程 `audiocpp_server` 运行，暴露
/// OpenAI 风格 HTTP API（`/health`、`/v1/models`、`/v1/audio/speech`、
/// `/v1/audio/transcriptions`）。本模块负责：引擎二进制定位（locator）、server
/// config 生成（server_config）、进程生命周期管理（server，lease + 按配置指纹
/// 多实例并存 + 健康轮询 + 懒重启）、HTTP 客户端与 wav 编解码（client）。
///
/// 与 sherpa-onnx 进程内引擎的边界：[`crate::tts::TtsEngine`] 门面按
/// `ResolvedTtsConfig.backend` 分派，ASR 侧由 `voice::asr_backend::AsrBackend`
/// 按 `ResolvedAsrConfig.backend` 分派，本模块不接触 sherpa 类型。
///
/// 模型族差异统一收敛在描述表：TTS 见 [`families`]（音色语义等），ASR 见
/// [`asr_families`]；任务中立的实例规格见 [`server_config::ServerInstanceSpec`]。
pub mod asr_families;
pub mod client;
pub mod families;
pub mod locator;
pub mod provider;
pub mod server;
pub mod server_config;

/// audio.cpp sidecar 集成的统一错误分类。
///
/// 各变体的用户文案见 [`Self::to_user_message`]（中文，测试锚定关键子串）。
#[derive(Debug)]
pub enum AudiocppError {
    /// 引擎二进制未找到（携带已搜索路径列表）
    EngineNotFound { searched: Vec<std::path::PathBuf> },
    /// 进程启动失败（spawn 报错）
    SpawnFailed(String),
    /// 引擎启动后立即退出——典型原因：请求的 backend 未被该引擎构建编入
    /// （如 CPU-only 引擎收到 cuda）、无 NVIDIA GPU、驱动过旧、引擎目录缺少
    /// ggml 运行库 DLL（后端枚举只扫引擎目录与进程 CWD）。
    EngineExitedImmediately {
        backend: String,
        stderr_tail: String,
    },
    /// 健康检查超时（携带 server stderr 末尾若干行辅助诊断）
    StartupTimeout {
        timeout_secs: u32,
        stderr_tail: String,
    },
    /// `/v1/models` 未列出所需模型（模型文件缺失/损坏）
    ModelNotListed { model_id: String },
    /// 连接失败（server 未起或已退出）
    Connection(String),
    /// HTTP 非 2xx（携带状态码与响应体）
    HttpStatus { status: u16, body: String },
    /// wav 解码失败
    DecodeWav(String),
    /// wav 编码失败（ASR 上传方向）
    EncodeWav(String),
    /// 后端不支持的音色参数（如对 qwen3_tts 传具名音色）
    UnsupportedVoice(String),
    /// 该模型族不支持 SSE 流式合成（`families::supports_streaming == false`）
    StreamingUnsupported(String),
    /// SSE 流内错误事件（server 在流中途上报 `{"type":"error",...}`）
    StreamEvent(String),
}

impl AudiocppError {
    /// 面向用户的中文文案（调用方直接 `to_string()` 展示）。
    pub fn to_user_message(&self) -> String {
        match self {
            Self::EngineNotFound { searched } => format!(
                "未找到 audiocpp_server 引擎（已搜索：{}）。安装包应内置该引擎；\
                 开发模式请运行 scripts/fetch-audiocpp-dev.sh 或放置到 ~/.zapmomo/engines/。",
                searched
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::SpawnFailed(e) => format!("启动 audiocpp_server 失败: {e}"),
            Self::EngineExitedImmediately {
                backend,
                stderr_tail,
            } => format!(
                "audiocpp_server 以 {backend} 后端启动后立即退出\
                 （常见原因：该后端未被引擎构建支持、无 NVIDIA GPU 或驱动过旧、\
                 引擎目录缺少 ggml 运行库 DLL）。\
                 引擎输出末尾：\n{stderr_tail}"
            ),
            Self::StartupTimeout {
                timeout_secs,
                stderr_tail,
            } => format!(
                "audiocpp_server 启动超时（{timeout_secs}s）。引擎输出末尾：\n{stderr_tail}"
            ),
            Self::ModelNotListed { model_id } => format!(
                "audiocpp_server 未加载模型 {model_id}，请检查模型文件是否完整\
                 （在模型库重新下载，或运行 `zapmomo install-model`）。"
            ),
            Self::Connection(e) => {
                format!("无法连接 audiocpp_server（引擎未启动或已退出）: {e}")
            }
            Self::HttpStatus { status, body } => {
                format!("audiocpp_server 请求失败（HTTP {status}）: {body}")
            }
            Self::DecodeWav(e) => format!("解码合成音频失败: {e}"),
            Self::EncodeWav(e) => format!("编码上传音频失败: {e}"),
            Self::UnsupportedVoice(e) => e.clone(),
            Self::StreamingUnsupported(model_id) => {
                format!("该模型（{model_id}）不支持流式合成，请使用整段合成或更换模型。")
            }
            Self::StreamEvent(e) => format!("流式合成被服务端中断: {e}"),
        }
    }
}

impl std::fmt::Display for AudiocppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_user_message())
    }
}

impl std::error::Error for AudiocppError {}

impl From<AudiocppError> for String {
    fn from(e: AudiocppError) -> String {
        e.to_user_message()
    }
}
