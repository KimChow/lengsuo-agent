//! OpenAI 响应 → ProviderStep + usage 解析。
//!
//! ⛔ 严格按 OpenAI Chat Completions 规范解析，**不自创协议**。
//!
//! ## 字段映射表
//!
//! | OpenAI 字段 | 本仓产物 |
//! |---|---|
//! | `choices[0].message.content` | `ProviderStep::Text` |
//! | `choices[0].message.reasoning_content` | `ProviderStep::Thinking` |
//! | `choices[0].message.reasoning` | `ProviderStep::Thinking` |
//! | `choices[0].finish_reason` = `stop` / `length` | `ProviderStep::Finished` |
//! | `usage.prompt_tokens` | `TokenUsage::prompt_tokens` |
//! | `usage.completion_tokens` | `TokenUsage::completion_tokens` |
//! | `usage.total_tokens` | `TokenUsage::total_tokens` |
//! | `usage.prompt_tokens_details.cached_tokens` | `TokenUsage::cached_tokens` |
//!
//! ## `reasoning` / `reasoning_content` 双读
//!
//! 两家网关的字段名不一样（有的叫 `reasoning_content`，有的叫 `reasoning`），
//! 故**两个都读**：先 `reasoning_content`，再 `reasoning`，取第一个非空者
//! （同一个字段只产一次 `Thinking`，不重复）。

use agent_core::provider::{ProviderStep, TokenUsage};
use serde_json::Value;

/// 响应体解析失败（协议违例）。
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// 缺 `choices` 数组，或数组为空。
    #[error("响应体缺少 choices")]
    MissingChoices,
    /// `choices[0]` 缺 `message` 对象。
    #[error("choices[0] 缺少 message")]
    MissingMessage,
    /// `message.content` 既不是字符串也不是 `null`。
    #[error("message.content 类型非法（期望字符串或 null）")]
    InvalidContent,
}

/// 从 OpenAI 响应体解析步骤序列与 usage。
///
/// **usage 缺失 ⇒ `None`**（不估算、不报错）。
/// `usage` 对象存在但某个计数字段缺失 ⇒ 该字段取 0。
pub fn parse_response(body: &Value) -> Result<(Vec<ProviderStep>, Option<TokenUsage>), ParseError> {
    let choice = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or(ParseError::MissingChoices)?;
    let message = choice.get("message").ok_or(ParseError::MissingMessage)?;

    let mut steps = Vec::new();
    if let Some(reasoning) = reasoning_text(message) {
        steps.push(ProviderStep::Thinking(reasoning));
    }
    if let Some(content) = content_text(message)? {
        steps.push(ProviderStep::Text(content));
    }
    if is_finished(choice) {
        steps.push(ProviderStep::Finished);
    }
    Ok((steps, parse_usage(body.get("usage"))))
}

/// 推理正文：先 `reasoning_content`，再 `reasoning`（见模块文档「双读」）。
fn reasoning_text(message: &Value) -> Option<String> {
    ["reasoning_content", "reasoning"]
        .iter()
        .filter_map(|key| message.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|text| !text.is_empty())
        .map(str::to_string)
}

/// 回复正文。`null` ⇒ 无正文（`None`）；非字符串非 null ⇒ 协议违例。
fn content_text(message: &Value) -> Result<Option<String>, ParseError> {
    match message.get("content") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(ParseError::InvalidContent),
    }
}

/// 回合是否结束：`finish_reason` 是 `stop` / `length` ⇒ 模型已停止生成。
///
/// `tool_calls` **不**算结束——但本卡不送 `tools` 字段，正常不会走到这里；
/// 真走到了也不产 `Finished`（不假装成功）。
fn is_finished(choice: &Value) -> bool {
    matches!(
        choice.get("finish_reason").and_then(Value::as_str),
        Some("stop") | Some("length")
    )
}

/// 解析 `usage` 对象；缺失（非对象）⇒ `None`。
fn parse_usage(usage: Option<&Value>) -> Option<TokenUsage> {
    let usage = usage?;
    if !usage.is_object() {
        return None;
    }
    Some(TokenUsage {
        prompt_tokens: number_at(usage, &["prompt_tokens"]),
        completion_tokens: number_at(usage, &["completion_tokens"]),
        total_tokens: number_at(usage, &["total_tokens"]),
        cached_tokens: number_at(usage, &["prompt_tokens_details", "cached_tokens"]),
    })
}

/// 按路径取值（非数字/缺路径 ⇒ 0）。
///
/// 路径逐层下钻（`["prompt_tokens_details", "cached_tokens"]`）——
/// 中间层不是对象时返回 0（该字段缺失，**不**回退去猜别的位置）。
fn number_at(root: &Value, path: &[&str]) -> u64 {
    let mut cursor = root;
    for key in path {
        let Some(next) = cursor.get(*key) else {
            return 0;
        };
        cursor = next;
    }
    cursor.as_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text_of(steps: &[ProviderStep]) -> Option<&str> {
        steps.iter().find_map(|step| match step {
            ProviderStep::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }

    fn thinking_of(steps: &[ProviderStep]) -> Option<&str> {
        steps.iter().find_map(|step| match step {
            ProviderStep::Thinking(text) => Some(text.as_str()),
            _ => None,
        })
    }

    #[test]
    fn content_becomes_text_and_stop_becomes_finished() {
        let body = json!({
            "choices": [{
                "message": {"role": "assistant", "content": "你好"},
                "finish_reason": "stop"
            }]
        });
        let (steps, usage) = parse_response(&body).expect("解析应成功");
        assert_eq!(text_of(&steps), Some("你好"));
        assert!(steps.contains(&ProviderStep::Finished));
        assert!(usage.is_none(), "无 usage 时应为 None");
    }

    #[test]
    fn reasoning_content_is_read_before_reasoning() {
        let body = json!({
            "choices": [{
                "message": {
                    "content": "答案",
                    "reasoning_content": "从 reasoning_content 来",
                    "reasoning": "从 reasoning 来"
                },
                "finish_reason": "stop"
            }]
        });
        let (steps, _) = parse_response(&body).expect("解析应成功");
        assert_eq!(thinking_of(&steps), Some("从 reasoning_content 来"));
        assert_eq!(
            steps
                .iter()
                .filter(|step| matches!(step, ProviderStep::Thinking(_)))
                .count(),
            1,
            "两个字段只产一次 Thinking"
        );
    }

    #[test]
    fn reasoning_field_is_read_when_reasoning_content_absent() {
        let body = json!({
            "choices": [{
                "message": {"content": "答案", "reasoning": "从 reasoning 来"},
                "finish_reason": "stop"
            }]
        });
        let (steps, _) = parse_response(&body).expect("解析应成功");
        assert_eq!(thinking_of(&steps), Some("从 reasoning 来"));
    }

    #[test]
    fn usage_is_parsed_with_cached_tokens() {
        let body = json!({
            "choices": [{
                "message": {"content": "答案"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 208,
                "completion_tokens": 33,
                "total_tokens": 241,
                "prompt_tokens_details": {"cached_tokens": 195}
            }
        });
        let (_, usage) = parse_response(&body).expect("解析应成功");
        let usage = usage.expect("usage 应存在");
        assert_eq!(usage.prompt_tokens, 208);
        assert_eq!(usage.completion_tokens, 33);
        assert_eq!(usage.total_tokens, 241);
        assert_eq!(usage.cached_tokens, 195);
        assert_eq!(usage.billed_tokens(), 33 + 13);
    }

    #[test]
    fn missing_choices_is_an_error_not_empty_output() {
        let body = json!({"object": "chat.completion"});
        assert!(matches!(
            parse_response(&body),
            Err(ParseError::MissingChoices)
        ));
    }

    #[test]
    fn null_content_yields_no_text_step() {
        let body = json!({
            "choices": [{
                "message": {"content": null},
                "finish_reason": "stop"
            }]
        });
        let (steps, _) = parse_response(&body).expect("解析应成功");
        assert!(text_of(&steps).is_none());
        assert!(steps.contains(&ProviderStep::Finished));
    }
}
