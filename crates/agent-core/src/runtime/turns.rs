//! 回合执行：send_text（单队列投递）、run_turn（步驱循环）、单步原语
//! [`Runtime::step`](StepOutcome)、工具调用/结果记录与 check_tool。
//!
//! 步语义：一步 = 一次逻辑 LLM 调用。
//! - 每步只发一次 provider 调用；步在**首个工具调用**处终结，同批尾随的文本/结束项
//!   不在本步内存里续消费，由重读历史后的新一步重新产出；
//! - 工具调用先落进会话历史再结算结果；下一步**重读会话历史**（ToolCall 与
//!   ToolResult 成对在列）后再发起新的一次调用；
//! - 工具步结算（金标 opencode `step.ts:91-134`：门检 → executeTool → 截断入
//!   历史 → 发布；失败走 failTool 结算成结果而非中断回合）：注入执行器时按
//!   [`Runtime::check_tool`] 判定真实执行；未注入/需审批时保持「待宿主执行」
//!   占位，真实结果由宿主经 [`Runtime::record_tool_result`] 回填；
//! - 步数达上限仍未收敛 ⇒ **末步缴械**（金标 opencode `max-steps.ts` + `llm.ts` 的
//!   `stepLimitReached` 分支）：以 `toolChoice=none` 追加一次纯文本总结步
//!   （`MAX_STEPS_PROMPT` 临时进请求历史副本，工具定义保留以维持 provider 缓存
//!   前缀），该步的工具请求不执行、回错误结果，随后以
//!   `done{status:"max_steps_reached"}` 收口（沿用既有熔断事件形状）。
//!
//! 分层（对齐金标 `step.ts` / `llm.ts`）：[`Runtime::step`] 是「一步」的完整
//! 原语（读会话 → 组上下文 → 恰好一次 provider 调用 → 消费产出 → 工具结算 →
//! 返回 [`StepOutcome`]）；`run_turn` 只做回合政策——首步历史快照、步循环、
//! 每步 outcome 的分派与 `done` 收口、末步 `TurnReceipt`。步计数、事件累积器
//! 与 deny 记账都由调用方持有，原语不藏全局态。

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::permission::Decision;
use crate::provider::{ProviderStep, StepOptions, ToolChoice};
use crate::r#loop::{DEFAULT_MAX_STEPS, MAX_STEPS_PROMPT};
use crate::schema::{Event, Message, Role, Session, SessionStatus, ToolInfo};

use super::{narrow, now_ms, Runtime, TurnReceipt, CONTENT_PREVIEW_CHARS};

/// 单回合步数上限（一步 = 一次逻辑 LLM 调用）；复用 agent loop 的熔断阈值，不另造。
const MAX_TURN_STEPS: usize = DEFAULT_MAX_STEPS;

/// 一步原语的走向（对齐金标 `step.ts` 的 `Outcome`：原语只报告本步发生了
/// 什么，回合推进/收口策略归调用方）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    /// 本步无工具调用 ⇒ 回合收敛（金标 `Outcome.Completed`）。
    Converged,
    /// 工具已结算（真实/失败/首次 deny/pending 皆属此）⇒ 可开下一步。
    ToolSettled,
    /// 末步缴械已执行（`toolChoice=none` 的总结步跑完；其工具请求被拒不执行）。
    Surrendered,
    /// 同一工具二次被拒 ⇒ 回合终止（既有 deny 语义）。
    Aborted { tool: String },
}

impl Runtime {
    /// 发送一条用户输入，跑一个回合（echo provider 下即回显）。
    ///
    /// 单队列投递：同一会话的回合串行执行；后到的输入在回合之外排队，
    /// 不插入进行中的步驱循环。
    pub fn send_text(&self, id: Uuid, text: &str) -> Result<TurnReceipt, CoreError> {
        let drain = self.drain_lock(id)?;
        let _queued = drain.lock().expect("会话投递锁不得毒化");
        // 首步 history 取本轮输入推入之前的快照（本轮输入只作为 user_text 出现）。
        let history = self.get_session(id)?.messages;
        self.push_message(id, Role::User, text.to_string(), None)?;
        self.run_turn(id, text, &history)
    }

    /// 取会话投递锁（同一会话的回合据此串行，不穿插步边界）。
    fn drain_lock(&self, id: Uuid) -> Result<Arc<Mutex<()>>, CoreError> {
        let guard = self.sessions.lock().expect("会话表锁不得毒化");
        guard
            .get(&id)
            .map(|hot| Arc::clone(&hot.drain))
            .ok_or(CoreError::SessionNotFound(id))
    }

    /// 步驱循环（回合政策层，对齐金标 `llm.ts:191-198` 的外层 while）：一步
    /// 一次调用；工具步结算后重读历史开新步；到限转缴械步收口。每步经
    /// [`Runtime::step`] 执行，本方法只分派 outcome 与收口 `TurnReceipt`。
    fn run_turn(
        &self,
        id: Uuid,
        text: &str,
        history: &[Message],
    ) -> Result<TurnReceipt, CoreError> {
        let mut events: Vec<Event> = Vec::new();
        // 首步用「回合前历史 + 本轮输入」；续步重读会话历史（本轮输入已在其中）。
        let mut pre_turn = Some(history.to_vec());
        let mut steps = 0usize;
        // 同一工具被拒后再次被拒 ⇒ 回合终止（既有 deny 语义，`loop` 同款记账）。
        let mut denied_tools: BTreeSet<String> = BTreeSet::new();

        loop {
            let outcome = if steps >= MAX_TURN_STEPS {
                // 到限不硬停：走一次缴械步（toolChoice=none 的纯文本总结步）。
                self.surrender_step(id, &mut denied_tools, &mut events)?
            } else {
                let (step_history, user_text) = match pre_turn.take() {
                    Some(pre) => (pre, text),
                    None => (self.get_session(id)?.messages.clone(), ""),
                };
                self.step(
                    id,
                    &step_history,
                    user_text,
                    &StepOptions::default(),
                    &mut denied_tools,
                    &mut events,
                )?
            };
            steps += 1;

            match outcome {
                // 本步无工具调用 ⇒ 回合收敛。
                StepOutcome::Converged => {
                    let session = self.get_session(id)?;
                    events.push(self.emit(
                        &session,
                        "done",
                        json!({ "status": "completed", "steps": steps }),
                    )?);
                    break;
                }
                // 工具已结算 ⇒ 重读历史开下一步（或到限转缴械步）。
                StepOutcome::ToolSettled => {}
                // 末步缴械已执行 ⇒ 熔断收口（沿用既有事件形状）。
                StepOutcome::Surrendered => {
                    let session = self.get_session(id)?;
                    events.push(self.emit(
                        &session,
                        "done",
                        json!({
                            "status": "max_steps_reached",
                            "steps": steps,
                            "maxSteps": MAX_TURN_STEPS,
                        }),
                    )?);
                    break;
                }
                // 同一工具二次被拒 ⇒ 回合终止（既有 done{denied} 收口形状）。
                StepOutcome::Aborted { tool } => {
                    let session = self.get_session(id)?;
                    events.push(self.emit(
                        &session,
                        "done",
                        json!({ "status": "denied", "steps": steps, "tool": tool }),
                    )?);
                    break;
                }
            }
        }

        let current = self.get_session(id)?;
        Ok(TurnReceipt {
            session_id: id,
            events,
            last_seq: current.last_seq,
        })
    }

    /// 单步原语（对齐金标 `step.ts` 的 `attempt`：一步 = 一次调用 + 工具 +
    /// 结算 + 返回走向）：读会话 → 组上下文 → **恰好一次** provider 调用 →
    /// 消费产出（thinking/text 落事件与消息，遇首个工具调用即止）→ 工具
    /// 结算 → 返回 [`StepOutcome`]。
    ///
    /// 请求三要素（`history` / `user_text` / `options`）由回合政策层准备
    /// （对齐金标 `llm.ts` `runStep` 的组装位）：首步传回合前历史快照 + 本轮
    /// 输入；续步传重读的会话历史 + 空 `user_text`；缴械步传「会话历史 +
    /// [`MAX_STEPS_PROMPT`] 副本」且 `tool_choice=None`。`tool_choice=None` 时
    /// 本步工具请求不执行、按金标 `step.ts:91-94` 回错误结果（不落 ToolCall
    /// 消息），走向为 [`StepOutcome::Surrendered`]；正常步走既有
    /// `next_steps_with_context` 入口，缴械步走带选项入口（调用面与抽出前
    /// 逐字一致）。
    ///
    /// `denied_tools`（同一工具二次被拒记账）与 `events`（事件累积器）由调用
    /// 方持有；本方法不发回合级 `done` 事件（那属 `run_turn` 的收口职责）。
    pub fn step(
        &self,
        id: Uuid,
        history: &[Message],
        user_text: &str,
        options: &StepOptions,
        denied_tools: &mut BTreeSet<String>,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        let session = self.get_session(id)?;
        if matches!(session.status, SessionStatus::Done | SessionStatus::Failed) {
            return Err(CoreError::SessionClosed(id));
        }
        let context = self.build_turn_context(&session, history, user_text);
        let batch = if options.tool_choice == ToolChoice::None {
            self.provider.next_steps_with_options(&context, options)?
        } else {
            self.provider.next_steps_with_context(&context)?
        };

        let Some((name, args)) = self.consume_step(id, &session, batch, events)? else {
            return Ok(if options.tool_choice == ToolChoice::None {
                StepOutcome::Surrendered
            } else {
                StepOutcome::Converged
            });
        };

        if options.tool_choice == ToolChoice::None {
            // 缴械步止于首个工具调用：请求被拒不执行，回合就此收口。
            self.reject_disabled_tool(id, &session, &name, args, events)?;
            return Ok(StepOutcome::Surrendered);
        }

        // 工具步：调用先持久化并广播（模型确实发出了请求），再按门检执行结算，
        // 下一步重读历史。
        self.record_tool_call(id, &name, &args)?;
        let args = self.redactor.redact_json(&args);
        events.push(self.emit(
            &session,
            "tool_use",
            json!({ "name": name.clone(), "args": args.clone() }),
        )?);
        self.execute_tool(id, &session, &name, &args, denied_tools, events)
    }

    /// 末步缴械的政策准备 + 一步原语调用（金标 opencode `max-steps.ts` +
    /// `llm.ts:226-247` 的 `stepLimitReached` 分支 + `step.ts:91-94`）：到限
    /// 不硬停，把 [`MAX_STEPS_PROMPT`] 作为 assistant 消息**临时**拼进本次
    /// 请求的 history 副本（不写会话历史——它不是真实产出），以
    /// `tool_choice=None` 走 [`Runtime::step`]。该步的工具调用不执行（不调
    /// [`Runtime::record_tool_call`]、不发待执行占位），按 `step.ts` 的
    /// `executeTool` 回错误结果，走向为 [`StepOutcome::Surrendered`]。
    fn surrender_step(
        &self,
        id: Uuid,
        denied_tools: &mut BTreeSet<String>,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        let session = self.get_session(id)?;
        // 续步口径：历史重读会话消息、本轮输入不再重复出现。
        let mut history = session.messages.clone();
        history.push(Message {
            role: Role::Assistant,
            content: MAX_STEPS_PROMPT.to_string(),
            tool: None,
            ts_ms: now_ms(),
        });
        self.step(
            id,
            &history,
            "",
            &StepOptions {
                tool_choice: ToolChoice::None,
            },
            denied_tools,
            events,
        )
    }

    /// 缴械步工具请求的拒绝结算（金标 `step.ts:91-94`）：不执行、不落 ToolCall
    /// 消息（未获准的调用不进历史），只回一条错误 `tool_result`（消息 + 事件，
    /// 错误文本为金标原文）。
    fn reject_disabled_tool(
        &self,
        id: Uuid,
        session: &Session,
        name: &str,
        args: Value,
        events: &mut Vec<Event>,
    ) -> Result<(), CoreError> {
        let error = json!({
            "type": "error",
            "message": "Tools are disabled after the maximum agent steps",
        });
        self.push_tool_result(id, name, args, &error.to_string())?;
        events.push(self.emit(
            session,
            "tool_result",
            json!({ "name": name, "result": error, "isError": true }),
        )?);
        Ok(())
    }

    /// 消费一步的产出：逐条落事件，遇工具调用或结束即止（批内尾随项属新一步）。
    /// 返回本步终结处的工具调用；`None` 表示本步无工具调用 ⇒ 回合收敛。
    fn consume_step(
        &self,
        id: Uuid,
        session: &Session,
        batch: Vec<ProviderStep>,
        events: &mut Vec<Event>,
    ) -> Result<Option<(String, Value)>, CoreError> {
        for item in batch {
            match item {
                ProviderStep::Thinking(note) => {
                    events.push(self.emit(
                        session,
                        "thinking",
                        json!({ "text": self.redact(&note) }),
                    )?);
                }
                ProviderStep::Text(reply) => {
                    let reply = self.redact(&reply);
                    events.push(self.emit(session, "text", json!({ "text": reply }))?);
                    self.push_message(id, Role::Assistant, reply, None)?;
                }
                // 本步止于首个工具调用；同批尾随项不在此消费。
                ProviderStep::ToolCall { name, args } => return Ok(Some((name, args))),
                ProviderStep::Finished => return Ok(None),
            }
        }
        Ok(None)
    }

    /// 把工具结果写进会话历史（大输出进 blob，消息只留摘要），不落事件。
    ///
    /// 事件流里的 `tool_result` 由三类真实结算产生：宿主经
    /// [`Runtime::record_tool_result`] 回填的真实结果、缴械步的拒绝结算
    /// （[`Runtime::reject_disabled_tool`]，本文件私有），与注入执行器后
    /// [`Runtime::execute_tool`] 的真实/失败/deny 结算；pending 占位
    /// 不冒充真实结果、不落事件。
    pub(super) fn push_tool_result(
        &self,
        id: Uuid,
        tool: &str,
        args: Value,
        result: &str,
    ) -> Result<(Message, Value), CoreError> {
        let args = self.redactor.redact_json(&args);
        let args_ref = self
            .blobs
            .put(args.to_string().as_bytes())
            .map(|r| r.0)
            .ok();
        let raw = self.redactor.redact_str(result);
        let (preview, result_ref) = if raw.chars().count() > CONTENT_PREVIEW_CHARS {
            let blob = self.blobs.put(raw.as_bytes())?;
            (narrow(&raw, CONTENT_PREVIEW_CHARS), Some(blob.0))
        } else {
            (raw.clone(), None)
        };
        let tool_info = ToolInfo {
            name: tool.to_string(),
            args_ref,
            result_ref: result_ref.clone(),
            ms: None,
        };
        let message = Message {
            role: Role::ToolResult,
            content: preview.clone(),
            tool: Some(tool_info.clone()),
            ts_ms: now_ms(),
        };
        self.push_message(id, Role::ToolResult, preview, Some(tool_info))?;
        let payload = json!({
            "name": tool,
            "chars": raw.chars().count(),
            "resultRef": result_ref,
            "preview": message.content,
        });
        Ok((message, payload))
    }

    /// 记录一次工具结果：大输出进 blob，消息只留摘要（§3 防分页打爆）。
    pub fn record_tool_result(
        &self,
        id: Uuid,
        tool: &str,
        args: Value,
        result: &str,
    ) -> Result<Message, CoreError> {
        let (message, payload) = self.push_tool_result(id, tool, args, result)?;
        let session = self.get_session(id)?;
        self.emit(&session, "tool_result", payload)?;
        Ok(message)
    }

    /// 追加工具调用消息（权限门放行后由宿主调用）。
    pub fn record_tool_call(
        &self,
        id: Uuid,
        tool: &str,
        args: &Value,
    ) -> Result<Session, CoreError> {
        let args = self.redactor.redact_json(args);
        let args_ref = self
            .blobs
            .put(args.to_string().as_bytes())
            .map(|r| r.0)
            .ok();
        let summary = self.redactor.summarize_args(&args);
        self.push_message(
            id,
            Role::ToolCall,
            summary,
            Some(ToolInfo {
                name: tool.to_string(),
                args_ref,
                result_ref: None,
                ms: None,
            }),
        )
    }

    /// 权限判定（工具执行前的唯一入口）。
    pub fn check_tool(&self, id: Uuid, tool: &str, args: &Value) -> Result<Decision, CoreError> {
        let session = self.get_session(id)?;
        if matches!(session.status, SessionStatus::Done | SessionStatus::Failed) {
            return Err(CoreError::SessionClosed(id));
        }
        Ok(self
            .gate
            .check(session.mode, session.permission_mode, tool, args))
    }
}
