/// [dsh] 配置解析：未配置项回退内置默认值。
use crate::config::settings::DshSettings;

/// 解析后的 dsh 桥配置（全字段非 Option）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedDshConfig {
    pub enabled: bool,
    /// 监听端口，0 = 随机
    pub port: u16,
    pub voice_enabled: bool,
    /// 事件是否经 LLM 生成播报文案（生效前提：[llm].enabled 且引擎已连接）
    pub llm_enabled: bool,
    pub record_to_history: bool,
}

pub fn resolve(settings: Option<&DshSettings>) -> ResolvedDshConfig {
    ResolvedDshConfig {
        enabled: settings.and_then(|s| s.enabled).unwrap_or(true),
        port: settings.and_then(|s| s.port).unwrap_or(0),
        voice_enabled: settings.and_then(|s| s.voice_enabled).unwrap_or(true),
        llm_enabled: settings.and_then(|s| s.llm_enabled).unwrap_or(true),
        record_to_history: settings.and_then(|s| s.record_to_history).unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_when_none() {
        let c = resolve(None);
        assert!(c.enabled);
        assert_eq!(c.port, 0);
        assert!(c.voice_enabled);
        assert!(c.llm_enabled);
        assert!(c.record_to_history);
    }

    #[test]
    fn test_overrides() {
        let s = DshSettings {
            enabled: Some(false),
            port: Some(47800),
            voice_enabled: Some(false),
            llm_enabled: Some(false),
            record_to_history: Some(false),
        };
        let c = resolve(Some(&s));
        assert!(!c.enabled);
        assert_eq!(c.port, 47800);
        assert!(!c.voice_enabled);
        assert!(!c.llm_enabled);
        assert!(!c.record_to_history);
    }
}
