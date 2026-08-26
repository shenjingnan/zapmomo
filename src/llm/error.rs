/// LLM 模块的错误类型。
///
/// 公开模块边界用明确的 `LlmError` 枚举（而非到处 `anyhow!("...")`），
/// 便于调用方（CLI / Tauri 命令）把错误映射成中文友好提示返回给用户。
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// 推理失败（请求/流式解析/响应异常）
    #[error("推理失败：{0}")]
    InferenceFailed(String),

    /// 生成被取消
    #[error("生成已取消")]
    GenerationCancelled,

    /// 后端不可用（网络/服务不可达、配置缺失）
    #[error("后端不可用：{0}")]
    BackendUnavailable(String),

    /// 已在进行中的生成任务
    #[error("已在进行中的生成任务")]
    Busy,

    /// 不支持的 provider
    #[error("不支持的 LLM provider：{0}")]
    UnsupportedProvider(String),
}
