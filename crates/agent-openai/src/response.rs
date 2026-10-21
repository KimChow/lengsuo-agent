//! OpenAI 响应 → ProviderStep + usage 解析。
//!
//! ⚠️ 骨架：实现由本卡的执行者补全。

use agent_core::provider::ProviderStep;

/// 从 OpenAI 响应体解析步骤序列与 usage。
pub fn parse_response(body: &serde_json::Value) -> (Vec<ProviderStep>, Option<agent_core::provider::TokenUsage>) {
    (Vec::new(), None)
}
