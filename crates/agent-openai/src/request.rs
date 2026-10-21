//! TurnContext → OpenAI Chat Completions 请求体。
//!
//! ⛔ 本模块只负责纯文本回合；工具调用往返不在本卡范围
//! （`tools` 字段**不送出**——传了但解析不了 = 模型调了但我们接不住）。
//!
//! 形状严格照 OpenAI Chat Completions 规范：
//! `{"model":…, "messages":[…], "stream":false}`。
//!
//! ## `memory_digest` 不进请求体
//!
//! `TurnContext` 有 `memory_digest`（旧单段摘要），但它**已含在 `system_prompt` 里**
//! 的等价内容（记忆四段）——两者的取值同源于一次读盘
//! （`agent-core/src/runtime/sessions/memory_vars.rs:112-116`：
//! `digest` 与 `segments` 由同一次 `collect_memory_layers` 产出）。
//! Composer 把四段渲染成 system prompt 的独立小节
//! （`agent-prompts/src/compose.rs:160-163` + `templates/common/mod.rs:132-135`）。
//! ⇒ 再单独追加 digest 是**同一份记忆第二次进模型**（重复且更啰嗦的旧口径摘要），
//! 故**不追加**。判断依据见 `docs/openai-provider-status.md`。

use agent_core::provider::TurnContext;
use agent_core::schema::{Message, Role};
use serde_json::{json, Value};

/// 把回合上下文渲染成 OpenAI 请求体。
///
/// `max_tokens` 不送（网关按模型默认；本仓没有可配置的输出预算口径）。
pub fn build_request(context: &TurnContext, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !context.system_prompt.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": context.system_prompt }));
    }
    for message in &context.history {
        if let Some(value) = history_message(message) {
            messages.push(value);
        }
    }
    messages.push(json!({ "role": "user", "content": context.user_text }));

    json!({
        "model": model,
        "messages": messages,
        "stream": false,
    })
}

/// 历史消息 → OpenAI `messages[]` 元素。
///
/// **角色映射**（本仓 `Role` 四类 → OpenAI 规范三类）：
/// - `User` → `user`
/// - `Assistant` → `assistant`
/// - `ToolCall` → `user`（工具调用是**已经发生**的事实，不是待执行请求；
///   本卡不做 tools 往返，故按纯文本事实回放）
/// - `ToolResult` → `user`（同上）
///
/// 工具类消息**不丢弃**：它们承载「上一步做了什么、结果是什么」，
/// 丢掉会让模型失忆。降级成 `user` 文本是本卡（无 tools 往返）下的唯一可行映射。
///
/// 空内容的历史消息跳过（OpenAI 规范不接受空 `content`）。
fn history_message(message: &Message) -> Option<Value> {
    let content = message.content.trim();
    if content.is_empty() {
        return None;
    }
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::ToolCall | Role::ToolResult => "user",
    };
    Some(json!({ "role": role, "content": message.content }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::provider::Uuid;
    use agent_core::schema::Mode;

    fn context(history: Vec<Message>, user_text: &str) -> TurnContext {
        TurnContext {
            session_id: Uuid::nil(),
            mode: Mode::Quick,
            system_prompt: "你是一个助手".to_string(),
            memory_digest: Some("[身份] AI=测试".to_string()),
            history,
            user_text: user_text.to_string(),
        }
    }

    fn message(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            tool: None,
            ts_ms: 0,
        }
    }

    #[test]
    fn system_prompt_becomes_system_message() {
        let body = build_request(&context(Vec::new(), "你好"), "auto");
        let messages = body["messages"].as_array().expect("messages 数组");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "你是一个助手");
    }

    #[test]
    fn history_roles_map_to_openai_roles() {
        let history = vec![
            message(Role::User, "我叫小明"),
            message(Role::Assistant, "你好小明"),
            message(Role::ToolCall, "[read] file.txt"),
            message(Role::ToolResult, "内容"),
        ];
        let body = build_request(&context(history, "我叫什么"), "auto");
        let messages = body["messages"].as_array().expect("messages 数组");
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[4]["role"], "user");
        assert_eq!(messages[5]["role"], "user");
        assert_eq!(messages[5]["content"], "我叫什么");
    }

    #[test]
    fn memory_digest_is_not_duplicated_into_body() {
        let body = build_request(&context(Vec::new(), "你好"), "auto");
        let text = serde_json::to_string(&body).expect("序列化");
        assert!(!text.contains("AI=测试"), "digest 不应重复进请求体");
    }

    #[test]
    fn body_is_stream_false_and_carries_model() {
        let body = build_request(&context(Vec::new(), "你好"), "auto");
        assert_eq!(body["model"], "auto");
        assert_eq!(body["stream"], false);
        assert!(body.get("tools").is_none(), "本卡不送 tools 字段");
    }

    #[test]
    fn empty_history_messages_are_skipped() {
        let history = vec![message(Role::User, "   ")];
        let body = build_request(&context(history, "你好"), "auto");
        let messages = body["messages"].as_array().expect("messages 数组");
        assert_eq!(messages.len(), 2);
    }
}
