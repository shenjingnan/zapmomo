/// audio.cpp sidecar 集成（TTS 第二后端）。
///
/// audio.cpp（Apache-2.0，ggml 系）作为独立进程 `audiocpp_server` 运行，暴露
/// OpenAI 风格 HTTP API（`/health`、`/v1/models`、`/v1/audio/speech`）。本模块
/// 负责：引擎二进制定位（locator）、server config 生成（server_config）、进程
/// 生命周期管理（server，lease + 按配置指纹多实例并存 + 健康轮询 + 懒重启）、
/// HTTP 客户端与 wav 解码（client）。
///
/// 与 sherpa-onnx 进程内引擎的边界：[`crate::tts::TtsEngine`] 门面按
/// `ResolvedTtsConfig.backend` 分派，本模块不接触 sherpa 类型。
///
/// 模型族差异（pocket / omnivoice 等）统一收敛在 [`families`] 静态描述表：
/// 模型 id、GGUF 文件名、必需文件清单、采样率、缺省推理后端、音色语义。
pub mod client;
pub mod families;
pub mod locator;
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
    /// 后端不支持的音色参数（如对 PocketTTS 传参考音频克隆）
    UnsupportedVoice(String),
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
            Self::StartupTimeout {
                timeout_secs,
                stderr_tail,
            } => format!(
                "audiocpp_server 启动超时（{timeout_secs}s）。引擎输出末尾：\n{stderr_tail}"
            ),
            Self::ModelNotListed { model_id } => format!(
                "audiocpp_server 未加载模型 {model_id}，请检查模型文件是否完整\
                 （在模型库重新下载，或运行 `zapmomo tts install-model`）。"
            ),
            Self::Connection(e) => {
                format!("无法连接 audiocpp_server（引擎未启动或已退出）: {e}")
            }
            Self::HttpStatus { status, body } => {
                format!("audiocpp_server 请求失败（HTTP {status}）: {body}")
            }
            Self::DecodeWav(e) => format!("解码合成音频失败: {e}"),
            Self::UnsupportedVoice(e) => e.clone(),
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
