//! 驱动路径的适配面（缺口 B1/B4 的落地）：把「回合开始 / 每步重读历史 / 消息落地 /
//! 一次 provider 调用」按 [`Driver`] 分派到本地或 [`Runtime`](crate::runtime::Runtime)。
//!
//! 这里是「历史真相源」这条不变式的唯一开关点：
//! - **本地路径**（[`Driver::Local`]）：历史是本层 [`History`](super::run) 的
//!   `Vec<Value>`，请求由 [`History::request`](super::run) 派生；
//! - **`Runtime` 路径**（[`Driver::Runtime`]）：历史在**会话**上——每步经
//!   [`Runtime::get_session`] 重读 `messages` 现派生请求，assistant / user 注记
//!   经 [`Runtime::append_message`] 直接落会话。本层**不驻留**历史副本。
//!
//! 对照金标：`session/runner/llm.ts` 每步 `context.load` 从持久层重读、
//! `session/context.ts` 把持久层历史装配成请求。

use serde_json::Value;

use crate::provider::StepOptions;
use crate::r#loop::{ChatTurn, ToolCall};
use crate::schema::{Message, Role};

use super::types::MAX_STEPS_PROMPT;
use super::{AgentLoop, Driver};

/// 把一条会话 `Message` 编成 `Value`（`Runtime` 路径的「请求」载体）。
///
/// 用 serde 的原生形态（`role` / `content` / `tool` / `tsMs`，见 `schema::Message`），
/// 与 [`decode_messages`] 严格互逆——`load_step` 编、[`AgentLoop::chat`] 解，故
/// `Runtime` 侧只认这一种形状（本地路径的 `{role,content,...}` 走另一分支）。
fn encode_message(message: &Message) -> Value {
    serde_json::to_value(message).unwrap_or(Value::Null)
}

/// [`encode_message`] 的逆：把 `load_step` 重读出的会话消息还原成 `Message` 序列。
///
/// 解析失败的项被丢弃（不会让整步失败）：会话消息是本层自己编的，正常不会失败；
/// 真要出现，按「少一条历史」降级比让回合起不来更符合最小上下文口径。
fn decode_messages(request: &[Value]) -> Vec<Message> {
    request
        .iter()
        .filter_map(|value| serde_json::from_value::<Message>(value.clone()).ok())
        .collect()
}

impl<'a> AgentLoop<'a> {
    /// 回合开始（首步之前调一次）：`Runtime` 路径把本轮用户输入落进会话（经 B4
    /// 的新公开面 [`Runtime::append_message`]）；本地路径无操作（[`History`] 的起点
    /// 已含本轮输入）。
    ///
    /// [`History`]: super::run
    pub(super) fn begin_turn(&mut self, user_input: &str) -> Result<(), String> {
        match &self.driver {
            Driver::Local { .. } => Ok(()),
            Driver::Runtime {
                runtime, session, ..
            } => {
                // 本轮输入先落一次（脱敏走 Runtime 内部口径）：历史即时生效，后续
                // 每步重读都能看到它。
                runtime
                    .append_message(*session, Role::User, user_input)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        }
    }

    /// **每步重读**本轮请求（「历史不驻留驱动层」的落点）。
    ///
    /// - 本地路径：从 [`History`](super::run) 组装口派生（`surrender` 时在请求末尾
    ///   临时追加 [`MAX_STEPS_PROMPT`]，不进历史）；
    /// - `Runtime` 路径：从 [`Runtime::get_session`] 的 `messages` **重读**——
    ///   外部（宿主 / 另一步）刚写进会话的消息，下一步请求即反映，本层没有缓存副本。
    ///   `surrender` 时同样把 [`MAX_STEPS_PROMPT`] 作为末条 assistant 追加进**本次
    ///   请求**（不写会话，它不是真实产出）。
    pub(super) fn load_step(
        &mut self,
        history: &super::run::History,
        surrender: bool,
    ) -> Result<Vec<Value>, String> {
        match &self.driver {
            Driver::Local { .. } => Ok(history.request(surrender)),
            Driver::Runtime {
                runtime, session, ..
            } => {
                let messages = runtime
                    .get_session(*session)
                    .map_err(|e| e.to_string())?
                    .messages;
                let mut request: Vec<Value> = messages.iter().map(encode_message).collect();
                if surrender {
                    request.push(encode_message(&Message {
                        role: Role::Assistant,
                        content: MAX_STEPS_PROMPT.to_string(),
                        tool: None,
                        ts_ms: now_ts_ms(),
                    }));
                }
                Ok(request)
            }
        }
    }

    /// 提醒注记落地：`Runtime` 路径写会话（[`Role::User`]）；本地路径无操作
    /// （由 [`History::push_user_note`](super::run) 负责）。
    pub(super) fn append_user_note(&mut self, note: &str) -> Result<(), String> {
        match &self.driver {
            Driver::Local { .. } => Ok(()),
            Driver::Runtime {
                runtime, session, ..
            } => runtime
                .append_message(*session, Role::User, note)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    /// assistant 文本落地：`Runtime` 路径写会话；本地路径无操作（组装口负责）。
    pub(super) fn append_assistant(&mut self, text: &str) -> Result<(), String> {
        match &self.driver {
            Driver::Local { .. } => Ok(()),
            Driver::Runtime {
                runtime, session, ..
            } => runtime
                .append_message(*session, Role::Assistant, text)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
    }

    /// 一次 provider 调用（对齐金标 `step.ts` 的 `attempt` 里的那一调）。
    ///
    /// - 本地路径：调宿主借进来的 [`ChatProvider`](super::types::ChatProvider)；
    /// - `Runtime` 路径：经 [`Runtime::chat_turn`] 转调 Runtime 注入的 provider
    ///   （上下文由 Runtime 按会话模式组装，历史即 `load_step` 重读出的一份），
    ///   再把 [`ProviderStep`](crate::provider::ProviderStep) 序列译成本层的
    ///   [`ChatTurn`]。`ProviderStep::ToolCall` 不带 id（缺口 B5），故 id 在此按序
    ///   合成（`c0`、`c1`…）——只用于本层事件形状的 `tool_use`/`tool_result` 配对，
    ///   与 Runtime 侧事件无关。
    pub(super) fn chat(
        &mut self,
        request: &[Value],
        tool_defs: &[Value],
        options: Option<&StepOptions>,
    ) -> Result<ChatTurn, String> {
        match &self.driver {
            Driver::Local { provider, .. } => match options {
                Some(options) => provider.chat_with_options(request, tool_defs, options),
                None => provider.chat(request, tool_defs),
            },
            Driver::Runtime { .. } => {
                let history = decode_messages(request);
                let steps = self
                    .runtime_chat_turn(&history, options)
                    .map_err(|e| e.to_string())?;
                Ok(turn_from_steps(steps))
            }
        }
    }

    /// `Runtime` 路径的 provider 转调（拆出以便 [`AgentLoop::chat`] 借用不冲突）。
    fn runtime_chat_turn(
        &self,
        history: &[Message],
        options: Option<&StepOptions>,
    ) -> Result<Vec<crate::provider::ProviderStep>, crate::error::CoreError> {
        match &self.driver {
            Driver::Runtime {
                runtime, session, ..
            } => runtime.chat_turn(*session, history, options),
            Driver::Local { .. } => Err(crate::error::CoreError::Unsupported(
                "runtime_chat_turn 仅用于 Runtime 路径",
            )),
        }
    }
}

/// [`ProviderStep`](crate::provider::ProviderStep) 序列 → 本层 [`ChatTurn`]。
fn turn_from_steps(steps: Vec<crate::provider::ProviderStep>) -> ChatTurn {
    use crate::provider::ProviderStep;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for step in steps {
        match step {
            ProviderStep::Text(chunk) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&chunk);
            }
            ProviderStep::ToolCall { name, args } => tool_calls.push(ToolCall {
                id: format!("c{}", tool_calls.len()),
                name,
                args,
            }),
            // `thinking` 与 `finished` 不进本层的 `ChatTurn`：前者由 Runtime 的
            // 事件面自行落 `thinking`（本层不重复），后者只是批次结束标记。
            ProviderStep::Thinking(_) | ProviderStep::Finished => {}
        }
    }
    ChatTurn { text, tool_calls }
}

fn now_ts_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
