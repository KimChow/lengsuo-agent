//! TurnContext → OpenAI Chat Completions 请求体。
//!
//! ⛔ 本模块只负责纯文本回合；工具调用往返不在本卡范围。

use agent_core::provider::TurnContext;
use serde_json::{json, Value};

/// 把回合上下文渲染成 OpenAI 请求体。
pub fn build_request(context: &TurnContext, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !context.system_prompt.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": context.system_prompt }));
    }
    let mut user = String::new();
    for message in &context.history {
        let text = message_text(message);
        if !text.trim().is_empty() {
            user.push_str(&text);
            user.push('\n');
        }
    }
    user.push_str(&context.user_text);
    messages.push(json!({ "role": "user", "content": user }));

    json!({
        "model": model,
        "messages": messages,
        "stream": false,
    })
}

/// 历史消息转文本。
fn message_text(message: &agent_core::schema::Message) -> String {
    // ⛔ 不猜 Message 的字段结构：以编译器为准，必要时在实现时按真实 schema 调整。
    String::new()
}
