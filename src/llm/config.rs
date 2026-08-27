/// LLM 配置解析：把可缺省的 `LlmSettings` 合并成解析后的 `ResolvedLlmConfig`。
///
/// 优先级：settings.toml > 内置默认。只支持 OpenAI 兼容远程 provider
/// （智谱 GLM / DeepSeek / OpenRouter / llama-server / Ollama 等），
/// 本地 llama.cpp 推理已移除。
use crate::config::settings::LlmSettings;
use crate::llm::types::GenParams;

/// 解析后的 LLM 配置（字段全部为具体类型，非 `Option`）。
#[derive(Debug, Clone)]
pub struct ResolvedLlmConfig {
    /// 是否启用 LLM
    pub enabled: bool,
    /// provider 标识（"openai" / "llamacpp-server"）
    pub provider: String,
    /// 角色 system prompt
    pub system_prompt: String,
    /// 采样/生成参数（远程 API 仅 max_tokens / temperature / top_p 生效）
    pub params: GenParams,
    /// HTTP provider 的 base URL（如 https://open.bigmodel.cn/api/paas/v4）
    pub base_url: Option<String>,
    /// HTTP provider 的 API key
    pub api_key: Option<String>,
    /// HTTP provider 的模型名（如 glm-4.7-flash）
    pub model: Option<String>,
}

/// 内置默认 system prompt（用户可在 settings 覆盖）。
pub fn default_system_prompt() -> String {
    "你是 ZapMomo，一个友好的桌面 AI 伙伴。请用简洁自然的中文回答，语气亲切，不要啰嗦。".to_string()
}

/// 合并 settings 得到最终配置。默认 provider 为 "openai"。
pub fn resolve(settings: Option<&LlmSettings>) -> Result<ResolvedLlmConfig, String> {
    let defaults = GenParams::default();

    Ok(ResolvedLlmConfig {
        enabled: settings.and_then(|s| s.enabled).unwrap_or(false),
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
        });
    }

    #[test]
    fn test_settings_provider_and_params() {
        let s = LlmSettings {
            enabled: Some(true),
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
}
