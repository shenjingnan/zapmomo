//! 远程 LLM 真实 API 冒烟测试（默认忽略，不进入 CI）。
//!
//! 手动运行（智谱 GLM）：
//! ```bash
//! ZHIPU_API_KEY=<your-key> cargo test --test remote_llm_smoke -- --ignored --nocapture
//! ```
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use zapmomo::llm::config::ResolvedLlmConfig;
use zapmomo::llm::http::OpenAiChatProvider;
use zapmomo::llm::provider::LlmProvider;
use zapmomo::llm::types::{ChatMessage, ChatRole, FinishReason, GenParams, InputItem, OutputItem};

#[test]
#[ignore = "真实 API 冒烟：需 ZHIPU_API_KEY 环境变量"]
fn test_zhipu_glm_streaming_smoke() {
    let api_key = std::env::var("ZHIPU_API_KEY").expect("需设置 ZHIPU_API_KEY");
    let config = ResolvedLlmConfig {
        enabled: true,
        provider: "openai".to_string(),
        system_prompt: "你是测试助手，用一句话回答。".to_string(),
        params: GenParams::default(),
        base_url: Some("https://open.bigmodel.cn/api/paas/v4".to_string()),
        api_key: Some(api_key),
        model: Some("glm-4.7-flash".to_string()),
    };
    let mut provider = OpenAiChatProvider::new(&config).unwrap();

    let input = vec![InputItem::Message(ChatMessage::new(
        ChatRole::User,
        "用一句话介绍你自己",
    ))];
    let mut text = String::new();
    let reason = provider
        .generate(
            &input,
            &[],
            &GenParams::default(),
            &mut |item| {
                if let OutputItem::MessageDelta(delta) = item {
                    text.push_str(&delta.text);
                }
            },
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

    println!("finish: {reason:?}\n回复: {text}");
    assert_eq!(reason, FinishReason::Eos);
    assert!(!text.is_empty(), "应收到非空回复");
}
