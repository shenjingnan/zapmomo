/// LLM 配置解析：把可缺省的 `LlmSettings` 合并成解析后的 `ResolvedLlmConfig`。
///
/// 优先级：settings.toml > 内置默认。支持 OpenAI 兼容远程 provider
/// （智谱 GLM / DeepSeek / OpenRouter / llama-server / Ollama 等）与 Anthropic
/// 原生 Messages API（provider = "anthropic"），本地 llama.cpp 推理已移除。
use crate::config::settings::LlmSettings;
use crate::llm::types::GenParams;

/// 形象切换 subagent 缺省开启：对话内不再注册 `set_character_sprite`，
/// 语音链路在回复结束后由后台单次调用自动决策切形象（见 `voice::sprite_agent`）。
pub const DEFAULT_SPRITE_AGENT: bool = true;

/// 解析后的 LLM 配置（字段全部为具体类型，非 `Option`）。
#[derive(Debug, Clone)]
pub struct ResolvedLlmConfig {
    /// 是否启用 LLM
    pub enabled: bool,
    /// 是否注册 CLI 工具（run_command）；未注册即对模型不可达
    pub cli_tools: bool,
    /// 是否注册形象切换工具（`set_character_sprite`）＝ `!sprite_agent`；
    /// false 时语音链路由 subagent 自动决策（`voice::sprite_agent`）
    pub sprite_tool: bool,
    /// 是否启用 prompt caching（仅 anthropic provider 生效）
    pub prompt_cache: bool,
    /// 是否启用思考（extended thinking 开关；仅 anthropic provider 生效）
    pub thinking: bool,
    /// 思考力度（仅 anthropic provider 生效；thinking 关闭时保留但忽略）
    pub reasoning_effort: Option<String>,
    /// provider 标识（"openai" / "llamacpp-server" / "anthropic"）
    pub provider: String,
    /// 角色 system prompt
    pub system_prompt: String,
    /// 采样/生成参数（远程 API 仅 max_tokens / temperature / top_p 生效）
    pub params: GenParams,
    /// HTTP provider 的 base URL（如 https://open.bigmodel.cn/api/paas/v4；
    /// anthropic 缺省为官方端点 https://api.anthropic.com/v1/）
    pub base_url: Option<String>,
    /// HTTP provider 的 API key
    pub api_key: Option<String>,
    /// HTTP provider 的模型名（如 glm-4.7-flash）
    pub model: Option<String>,
}

/// 内置默认 system prompt（用户可在 settings 覆盖）。
///
/// 纯文本约束是为 TTS 朗读服务：markdown 符号/emoji 会被读出或产生异常发音。
/// 此约束只是减负——清洗层（`voice::sanitizer`）才是必要兜底：角色包
/// character.md 会整体覆盖本 prompt（`voice::config::apply_character_override`），
/// 用户自定义人设的输出格式不可控。
pub fn default_system_prompt() -> String {
    "你是 ZapMomo，一个友好的桌面 AI 伙伴。请用简洁自然的中文回答，语气亲切，不要啰嗦。\
     回复必须是纯文本：不要用 Markdown（标题、列表、加粗、代码块、链接），不要输出 \
     emoji 或表情符号，要点直接写成短句。"
        .to_string()
}

/// 合并 settings 得到最终配置。默认 provider 为 "openai"。
pub fn resolve(settings: Option<&LlmSettings>) -> Result<ResolvedLlmConfig, String> {
    let defaults = GenParams::default();
    // thinking 缺省值按「是否配置了推理强度」推断：配过 effort ≈ 想用思考；
    // 都没配 → 关闭（语音场景延迟优先）。避免硬编码 false 静默破坏已配置用户
    let reasoning_effort = settings.and_then(|s| s.reasoning_effort.clone());
    let thinking = settings
        .and_then(|s| s.thinking)
        .unwrap_or_else(|| reasoning_effort.is_some());

    Ok(ResolvedLlmConfig {
        enabled: settings.and_then(|s| s.enabled).unwrap_or(false),
        cli_tools: settings.and_then(|s| s.cli_tools).unwrap_or(false),
        sprite_tool: !settings
            .and_then(|s| s.sprite_agent)
            .unwrap_or(DEFAULT_SPRITE_AGENT),
        prompt_cache: settings.and_then(|s| s.prompt_cache).unwrap_or(true),
        thinking,
        reasoning_effort,
        provider: settings
            .and_then(|s| s.provider.clone())
            .unwrap_or_else(|| "openai".to_string()),
        system_prompt: settings
            .and_then(|s| s.system_prompt.clone())
            .unwrap_or_else(default_system_prompt),
        params: GenParams {
            max_tokens: settings
                .and_then(|s| s.max_tokens)
                .unwrap_or(defaults.max_tokens),
            temperature: settings
                .and_then(|s| s.temperature)
                .unwrap_or(defaults.temperature),
            top_p: settings.and_then(|s| s.top_p).unwrap_or(defaults.top_p),
            ..defaults
        },
        base_url: settings.and_then(|s| s.base_url.clone()),
        api_key: settings.and_then(|s| s.api_key.clone()),
        model: settings.and_then(|s| s.model.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_resolve() {
        crate::test_util::run_with_temp_home(|_| {
            let cfg = resolve(None).unwrap();
            assert!(!cfg.enabled);
            assert_eq!(cfg.provider, "openai");
            assert_eq!(cfg.params.max_tokens, 512);
            assert_eq!(cfg.params.temperature, 0.7);
            assert!(cfg.base_url.is_none());
            assert!(cfg.model.is_none());
            assert!(!cfg.system_prompt.is_empty());
            // 纯文本约束防回退：TTS 朗读依赖它减少 markdown/emoji 清洗压力
            assert!(cfg.system_prompt.contains("纯文本"));
        });
    }

    #[test]
    fn test_settings_provider_and_params() {
        let s = LlmSettings {
            enabled: Some(true),
            cli_tools: Some(true),
            provider: Some("llamacpp-server".to_string()),
            temperature: Some(0.9),
            max_tokens: Some(128),
            base_url: Some("https://api.example.com/v1".to_string()),
            api_key: Some("sk-test".to_string()),
            model: Some("glm-4.7-flash".to_string()),
            system_prompt: Some("自定义提示".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(&s)).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.cli_tools);
        assert_eq!(cfg.provider, "llamacpp-server");
        assert_eq!(cfg.params.temperature, 0.9);
        assert_eq!(cfg.params.max_tokens, 128);
        assert_eq!(cfg.base_url.as_deref(), Some("https://api.example.com/v1"));
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
        assert_eq!(cfg.model.as_deref(), Some("glm-4.7-flash"));
        assert_eq!(cfg.system_prompt, "自定义提示");
        // 未配置项回退默认
        assert_eq!(cfg.params.top_p, 0.8);
    }

    #[test]
    fn test_legacy_local_provider_rejected() {
        // 旧配置里 provider = "local"：resolve 本身不报错（避免 TOML 解析炸），
        // 由 create_provider 报「不支持的 provider」
        let s = LlmSettings {
            provider: Some("local".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(&s)).unwrap();
        assert_eq!(cfg.provider, "local");
        // `Box<dyn LlmProvider>` 不实现 Debug，不能用 unwrap_err()，改用 match
        let err = match crate::llm::create_provider(cfg) {
            Ok(_) => panic!("provider='local' 应被拒绝"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("不支持的 LLM provider"),
            "实际错误：{err}"
        );
    }

    #[test]
    fn test_resolve_sprite_agent_flag() {
        crate::test_util::run_with_temp_home(|_| {
            // 缺省：subagent 开启 → 不注册对话内工具
            let cfg = resolve(None).unwrap();
            assert!(!cfg.sprite_tool);
            // 显式关闭 subagent → 恢复对话内工具（一键回滚）
            let s = LlmSettings {
                sprite_agent: Some(false),
                ..Default::default()
            };
            let cfg = resolve(Some(&s)).unwrap();
            assert!(cfg.sprite_tool);
        });
    }

    #[test]
    fn test_thinking_default_inference() {
        // 都没配：关闭（语音场景延迟优先）
        let cfg = resolve(None).unwrap();
        assert!(!cfg.thinking);
        assert!(cfg.reasoning_effort.is_none());
        // 只配力度未配开关：推断为开启（避免静默破坏已配置用户）
        let s = LlmSettings {
            reasoning_effort: Some("low".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(&s)).unwrap();
        assert!(cfg.thinking);
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("low"));
        // 显式配置优先于推断
        let s = LlmSettings {
            thinking: Some(false),
            reasoning_effort: Some("high".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(&s)).unwrap();
        assert!(!cfg.thinking);
        // 开关关闭时力度仍保留（运行时忽略）
        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn test_anthropic_provider_accepted() {
        let s = LlmSettings {
            provider: Some("anthropic".to_string()),
            model: Some("claude-haiku-4-5".to_string()),
            api_key: Some("sk-test".to_string()),
            ..Default::default()
        };
        let cfg = resolve(Some(&s)).unwrap();
        assert_eq!(cfg.provider, "anthropic");
        assert!(crate::llm::create_provider(cfg).is_ok());
    }
}
