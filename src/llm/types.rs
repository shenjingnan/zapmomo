/// LLM 模块的公共数据类型。
///
/// 这些类型是「owned + Serialize」的，用于在 Rust Core 与 Tauri 前端之间跨线程/跨进程传递，
/// 与 `kws::reaction::KwsResult` / `asr::reaction::AsrResult` 的约定一致--不泄漏 provider 内部类型。
use serde::{Deserialize, Serialize};

use crate::config::settings::LlmSettings;

/// 对话消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

/// 一条对话消息。
///
/// `tool_calls` 为后续 Tool Calling 预留（第一版不使用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
        }
    }
}

/// 一次工具调用（预留，第一版不实现）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// JSON 编码的参数
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// 一次流式生成的文本增量（可能是半个字 / 半个词）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TokenDelta {
    pub text: String,
    pub is_final: bool,
}

impl TokenDelta {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_final: false,
        }
    }

    pub fn final_(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_final: true,
        }
    }
}

/// 一次生成结束的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishReason {
    /// 模型自然结束（命中 EOS / stop token）
    Eos,
    /// 达到 `max_tokens` 上限
    MaxTokens,
    /// 被用户取消
    Cancelled,
    /// 出错终止
    Error,
}

/// 生成采样参数。
///
/// OpenAI 兼容 Chat Completions 实际生效的是 `max_tokens` / `temperature` / `top_p`；
/// `top_k` / `min_p` / `repeat_penalty` / `seed` 部分服务端支持（如 llama-server），
/// 不支持的服务端会忽略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenParams {
    /// 最多生成的 token 数
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub min_p: f32,
    pub repeat_penalty: f32,
    /// 随机种子；0 表示随机
    pub seed: u32,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.7,
            top_p: 0.8,
            top_k: 20,
            min_p: 0.05,
            repeat_penalty: 1.05,
            seed: 0,
        }
    }
}

/// 采样参数的写侧 DTO：`set_llm_params` 命令载荷。
///
/// 全部字段可缺省：`None` 表示「本次不修改该项」。字段名与 `GenParams` 一致（snake_case），
/// 前端载荷形如 `{ params: { max_tokens: 512, temperature: 0.7, ... } }`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmParamsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// 是否启用思考（thinking 开关）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    /// 思考力度（low / medium / high / max；仅 thinking 开启时生效。
    /// 开关关闭时该值仍持久化保留，只是运行时忽略——GUI 置灰保留选择的语义）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl LlmParamsPatch {
    /// 先整体校验（任一越界立即 Err），再逐项写入 `LlmSettings`，保证出错时不部分修改。
    pub fn apply_to(&self, llm: &mut LlmSettings) -> Result<(), String> {
        if let Some(v) = self.max_tokens
            && !(16..=262_144).contains(&v)
        {
            return Err(format!("最大生成 Tokens 需在 16~262144，当前 {v}"));
        }
        if let Some(v) = self.temperature
            && !(0.0..=2.0).contains(&v)
        {
            return Err(format!("温度需在 0~2，当前 {v}"));
        }
        if let Some(v) = self.top_p
            && !(0.0..=1.0).contains(&v)
        {
            return Err(format!("Top-P 需在 0~1，当前 {v}"));
        }
        if let Some(v) = self.top_k
            && v > 500
        {
            return Err(format!("Top-K 需在 0~500，当前 {v}"));
        }
        if let Some(v) = self.min_p
            && !(0.0..=1.0).contains(&v)
        {
            return Err(format!("Min-P 需在 0~1，当前 {v}"));
        }
        if let Some(v) = self.repeat_penalty
            && !(0.0..=3.0).contains(&v)
        {
            return Err(format!("重复惩罚需在 0~3，当前 {v}"));
        }
        // seed 为任意 u32，无需边界校验

        if let Some(v) = self.max_tokens {
            llm.max_tokens = Some(v);
        }
        if let Some(v) = self.temperature {
            llm.temperature = Some(v);
        }
        if let Some(v) = self.top_p {
            llm.top_p = Some(v);
        }
        if let Some(v) = self.top_k {
            llm.top_k = Some(v);
        }
        if let Some(v) = self.min_p {
            llm.min_p = Some(v);
        }
        if let Some(v) = self.repeat_penalty {
            llm.repeat_penalty = Some(v);
        }
        if let Some(v) = self.seed {
            llm.seed = Some(v);
        }
        if let Some(v) = self.thinking {
            llm.thinking = Some(v);
        }
        if let Some(v) = &self.reasoning_effort {
            let lowered = v.trim().to_lowercase();
            // 与 anthropic.rs 的 parse_reasoning_effort 保持同一合法集（low/medium/high/max）
            if !matches!(lowered.as_str(), "low" | "medium" | "high" | "max") {
                return Err(format!("推理强度需为 low / medium / high / max，当前 {v}"));
            }
            llm.reasoning_effort = Some(lowered);
        }
        Ok(())
    }
}

/// 工具调用结果（对应 OpenAI Responses 的 `function_call_output` item）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// 关联的 tool call id
    pub id: String,
    pub name: String,
    /// 工具执行的文本结果
    pub content: String,
}

/// 工具定义（供 Tool Calling 传给模型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema 参数
    pub parameters: serde_json::Value,
}

/// LLM 输入项（一次 Agent 步的上下文，有序）。
///
/// 统一抽象：Responses API 的 `input` 与 Chat Completions 的 `messages` 都映射到它。
#[derive(Debug, Clone, PartialEq)]
pub enum InputItem {
    /// 一条聊天消息（system / user / assistant）
    Message(ChatMessage),
    /// assistant 的一次工具调用（对应 Responses 的 `function_call`，回填到 input）
    ToolCall(ToolCall),
    /// 一次工具调用结果（对应 `function_call_output`）
    ToolResult(ToolResult),
}

/// LLM 输出项（流式，逐 item 产出）。
#[derive(Debug, Clone)]
pub enum OutputItem {
    /// 文本增量（最终回复 / reasoning 内容）
    MessageDelta(TokenDelta),
    /// 一次工具调用请求
    ToolCall(ToolCall),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch_with_all() -> LlmParamsPatch {
        LlmParamsPatch {
            max_tokens: Some(1024),
            temperature: Some(0.9),
            top_p: Some(0.95),
            top_k: Some(40),
            min_p: Some(0.1),
            repeat_penalty: Some(1.2),
            seed: Some(42),
            thinking: Some(true),
            reasoning_effort: Some("High".to_string()), // 大小写归一化
        }
    }

    #[test]
    fn test_apply_patch_sets_all_fields() {
        let mut llm = LlmSettings::default();
        patch_with_all().apply_to(&mut llm).unwrap();
        assert_eq!(llm.max_tokens, Some(1024));
        assert_eq!(llm.temperature, Some(0.9));
        assert_eq!(llm.top_p, Some(0.95));
        assert_eq!(llm.top_k, Some(40));
        assert_eq!(llm.min_p, Some(0.1));
        assert_eq!(llm.repeat_penalty, Some(1.2));
        assert_eq!(llm.seed, Some(42));
        assert_eq!(llm.thinking, Some(true));
        assert_eq!(llm.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn test_apply_patch_partial_keeps_others_none() {
        let mut llm = LlmSettings::default();
        let patch = LlmParamsPatch {
            temperature: Some(1.1),
            ..Default::default()
        };
        patch.apply_to(&mut llm).unwrap();
        assert_eq!(llm.temperature, Some(1.1));
        assert_eq!(llm.max_tokens, None);
        assert_eq!(llm.top_k, None);
    }

    #[test]
    fn test_apply_patch_rejects_out_of_range() {
        let cases: Vec<(LlmParamsPatch, &str)> = vec![
            (
                LlmParamsPatch {
                    max_tokens: Some(15),
                    ..Default::default()
                },
                "最大生成",
            ),
            (
                LlmParamsPatch {
                    temperature: Some(2.1),
                    ..Default::default()
                },
                "温度",
            ),
            (
                LlmParamsPatch {
                    top_p: Some(1.1),
                    ..Default::default()
                },
                "Top-P",
            ),
            (
                LlmParamsPatch {
                    top_k: Some(501),
                    ..Default::default()
                },
                "Top-K",
            ),
            (
                LlmParamsPatch {
                    min_p: Some(-0.1),
                    ..Default::default()
                },
                "Min-P",
            ),
            (
                LlmParamsPatch {
                    repeat_penalty: Some(3.1),
                    ..Default::default()
                },
                "重复惩罚",
            ),
            (
                LlmParamsPatch {
                    reasoning_effort: Some("extreme".to_string()),
                    ..Default::default()
                },
                "推理强度",
            ),
        ];
        for (patch, label) in cases {
            let mut llm = LlmSettings::default();
            let err = patch.apply_to(&mut llm).unwrap_err();
            assert!(
                err.contains(label),
                "非法值 {label} 应被拒绝，实际错误：{err}"
            );
            // 校验失败时不得部分写入
            assert_eq!(
                llm,
                LlmSettings::default(),
                "{label} 校验失败后不应修改 settings"
            );
        }
    }

    #[test]
    fn test_apply_patch_accepts_boundary_values() {
        let patch = LlmParamsPatch {
            max_tokens: Some(16),
            temperature: Some(0.0),
            top_p: Some(1.0),
            ..Default::default()
        };
        let mut llm = LlmSettings::default();
        assert!(patch.apply_to(&mut llm).is_ok());
        assert_eq!(llm.max_tokens, Some(16));
        assert_eq!(llm.temperature, Some(0.0));
        assert_eq!(llm.top_p, Some(1.0));
    }

    #[test]
    fn test_patch_serde_uses_snake_case() {
        let patch = patch_with_all();
        let value = serde_json::to_value(&patch).unwrap();
        let obj = value.as_object().unwrap();
        // 载荷键必须与前端 snake_case 契约一致（Tauri 嵌套结构按 serde 名直传）
        for key in [
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "min_p",
            "repeat_penalty",
            "seed",
        ] {
            assert!(obj.contains_key(key), "缺失字段 {key}");
        }
    }

    #[test]
    fn test_patch_resolve_roundtrip() {
        crate::test_util::run_with_temp_home(|_home| {
            let mut llm = LlmSettings {
                system_prompt: Some("自定义提示词".to_string()),
                ..Default::default()
            };
            patch_with_all().apply_to(&mut llm).unwrap();

            let cfg = crate::llm::config::resolve(Some(&llm)).unwrap();
            assert_eq!(cfg.system_prompt, "自定义提示词");
            assert_eq!(cfg.params.temperature, 0.9);
            assert_eq!(cfg.params.top_p, 0.95);
            assert_eq!(cfg.params.max_tokens, 1024);
            // patch → settings → resolve 全链路透传
            assert!(cfg.thinking);
            assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
        });
    }
}
