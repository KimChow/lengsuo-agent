//! 回合执行：send_text（单队列投递）、run_turn（步驱循环）、单步原语
//! [`Runtime::step`](StepOutcome)、工具调用/结果记录与 check_tool。
//!
//! 步语义：一步 = 一次逻辑 LLM 调用。
//! - 用户输入本身也是一条 durable 事件（金标 `session-event` 的 user 消息
//!   投影：`InboxDelivered` 是会话事件面上的首条，投影出 user 消息）：
//!   [`Runtime::send_text`] 先落 user 消息、随后落 `"user"` 事件；提醒回合的
//!   提醒消息同样落一条 `"user"` 事件。二者都在回合事件面**开头**，事件日志
//!   因此可逐条重建消息序列（重建本身留给下一张卡）。
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
//! - 交付物提醒（wb 18 册 §4 的 present outcome 强制级下沉）：Craft/Expert 下
//!   任务文本含写意图（创建/写入/修改）却整回合未成功执行任何写/执行工具且
//!   **自然收敛** ⇒ 向会话追加一条提醒 user 消息后走一次提醒回合（正常步循
//!   环，步数与本回合共享 [`Runtime::max_turn_steps`]），整个 `send_text` 至多一次；
//!   缴械（`Surrendered`）与被拒终止（`Aborted`）不提醒。
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
use crate::r#loop::MAX_STEPS_PROMPT;
use crate::schema::{Event, Message, Mode, Role, Session, SessionStatus, ToolInfo};

use super::{narrow, now_ms, Runtime, TurnReceipt, CONTENT_PREVIEW_CHARS};

/// 交付物意图关键词（卡面冻结：创建/写入/修改；与 `loop/run.rs` 的
/// `DELIVERABLE_KEYWORDS` 同表——该常量是 loop 模块私有，本卡不动 `loop/`，
/// 故在此复刻同一张表，loop 侧收敛时去重）。
const DELIVERABLE_KEYWORDS: [&str; 3] = ["创建", "写入", "修改"];

/// 交付物提醒文案（逐字沿用 `loop/run.rs` 既有原文）。
const DELIVERABLE_REMINDER_NOTE: &str =
    "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

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
    /// 本回合实际生效的单回合步数上限（一步 = 一次逻辑 LLM 调用）。
    ///
    /// 对齐金标 opencode `llm.ts:226` 的 `stepLimitReached = agent.info.steps !==
    /// undefined && step >= agent.info.steps`：上限是**每 agent 的配置值**，不是写死
    /// 常量。本仓 agent 级配置面 v0 尚未落地，故挂在
    /// [`RuntimeConfig::max_turn_steps`] 上，默认值仍来自既有常量
    /// [`crate::r#loop::DEFAULT_MAX_STEPS`]（默认行为不变）。配置锁中毒时退回默认值：
    /// 上限属安全阀，取不到配置也不该让回合起不来。
    ///
    /// [`RuntimeConfig::max_turn_steps`]: crate::config::RuntimeConfig::max_turn_steps
    fn max_turn_steps(&self) -> usize {
        self.config
            .lock()
            .map(|c| c.max_turn_steps)
            .unwrap_or(crate::config::DEFAULT_MAX_TURN_STEPS)
    }

    /// 发送一条用户输入，跑一个回合（echo provider 下即回显）。
    ///
    /// 单队列投递：同一会话的回合串行执行；后到的输入在回合之外排队，
    /// 不插入进行中的步驱循环。
    pub fn send_text(&self, id: Uuid, text: &str) -> Result<TurnReceipt, CoreError> {
        let drain = self.drain_lock(id)?;
        let _queued = drain.lock().expect("会话投递锁不得毒化");
        // 首步 history 取本轮输入推入之前的快照（本轮输入只作为 user_text 出现）。
        let history = self.get_session(id)?.messages;
        // 脱敏只做一次：`emit` 内部还会再过一次 `redact_json`，若此处先脱敏，
        // 事件 text 与历史消息 content 才逐字同一（否则敏感片段被改两次）。
        let redacted = self.redact(text);
        let session = self.push_message(id, Role::User, redacted.clone(), None)?;
        // 用户输入本身也是一条 durable 事件（金标 `session-event` 的 user 消息投影：
        // `InboxDelivered` 是会话事件面上的首条，投影出 user 消息）。顺序：消息先落、
        // 事件随后，且事件必须是回合事件面的第一条，否则 receipt ≡ 持久日志会破。
        let event = self.emit(&session, "user", json!({ "text": redacted }))?;
        self.run_turn(id, text, &history, event)
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
    /// 一次调用；工具步结算后重读历史开新步；到限转缴械步收口；自然收敛时
    /// 先过交付物提醒判定（满足则追加一次提醒回合）。每步经
    /// [`Runtime::step_with_write_ledger`] 执行，本方法只分派 outcome 与收口
    /// `TurnReceipt`。
    ///
    /// `first_event` 是 `send_text` 已在消息落盘后发布的 `user` 事件：它是
    /// 回合事件面的第一条（金标 `session-event` 的 user 消息投影），必须进
    /// [`TurnReceipt::events`]，否则 receipt 与持久日志的 seq 逐项相等会破。
    fn run_turn(
        &self,
        id: Uuid,
        text: &str,
        history: &[Message],
        first_event: Event,
    ) -> Result<TurnReceipt, CoreError> {
        let mut events: Vec<Event> = vec![first_event];
        // 首步用「回合前历史 + 本轮输入」；续步重读会话历史（本轮输入已在其中）。
        let mut pre_turn = Some(history.to_vec());
        // 回合输入：原回合为本轮用户输入；提醒回合切换为提醒文案。
        let mut turn_input = text.to_string();
        let mut steps = 0usize;
        // 上限在本回合**开头**取一次快照：回合进行中改配置不改已开跑回合的判定
        // （一处取值，循环内复用同一数值，`maxSteps` 事件与判定必然一致）。
        let max_turn_steps = self.max_turn_steps();
        // 同一工具被拒后再次被拒 ⇒ 回合终止（既有 deny 语义，`loop` 同款记账）。
        let mut denied_tools: BTreeSet<String> = BTreeSet::new();
        // 交付物记账（覆盖原回合 + 提醒回合）：成功执行过写/执行工具 ⇒ 不提醒。
        let mut write_executed = false;
        let mut reminder_used = false;

        loop {
            let outcome = if steps >= max_turn_steps {
                // 到限不硬停：走一次缴械步（toolChoice=none 的纯文本总结步）。
                self.surrender_step(id, &mut denied_tools, &mut write_executed, &mut events)?
            } else {
                let (step_history, user_text) = match pre_turn.take() {
                    Some(pre) => (pre, turn_input.as_str()),
                    None => (self.get_session(id)?.messages.clone(), ""),
                };
                self.step_with_write_ledger(
                    id,
                    &step_history,
                    user_text,
                    &StepOptions::default(),
                    &mut denied_tools,
                    &mut write_executed,
                    &mut events,
                )?
            };
            steps += 1;

            match outcome {
                // 本步无工具调用 ⇒ 回合收敛（或转一次交付物提醒回合）。
                StepOutcome::Converged => {
                    let session = self.get_session(id)?;
                    if needs_deliverable_reminder(session.mode, text, write_executed, reminder_used)
                    {
                        // 提醒回合（`loop/run.rs` 同款行为下沉）：追加提醒 user
                        // 消息后走正常回合循环（首步历史取追加后的会话历史、
                        // 本轮输入 = 提醒文案），步数与本回合共享计数；整个
                        // send_text 至多一次，提醒回合自身不再触发提醒。
                        reminder_used = true;
                        events.push(self.emit(
                            &session,
                            "thinking",
                            json!({ "reminder": DELIVERABLE_REMINDER_NOTE }),
                        )?);
                        // 提醒文案是常量，但同样只脱敏一次（与 `send_text` 同口径）：
                        // 提醒消息先落，随后落提醒回合的 `user` 事件，顺序与消息一致。
                        let reminder_text = self.redact(DELIVERABLE_REMINDER_NOTE);
                        let session =
                            self.push_message(id, Role::User, reminder_text.clone(), None)?;
                        events.push(self.emit(
                            &session,
                            "user",
                            json!({ "text": reminder_text }),
                        )?);
                        pre_turn = Some(self.get_session(id)?.messages);
                        turn_input = DELIVERABLE_REMINDER_NOTE.to_string();
                        continue;
                    }
                    events.push(self.emit(
                        &session,
                        "done",
                        json!({ "status": "completed", "steps": steps }),
                    )?);
                    break;
                }
                // 工具已结算 ⇒ 重读历史开下一步（或到限转缴械步）。
                StepOutcome::ToolSettled => {}
                // 末步缴械已执行 ⇒ 熔断收口（沿用既有事件形状）；缴械不提醒。
                StepOutcome::Surrendered => {
                    let session = self.get_session(id)?;
                    events.push(self.emit(
                        &session,
                        "done",
                        json!({
                            "status": "max_steps_reached",
                            "steps": steps,
                            "maxSteps": max_turn_steps,
                        }),
                    )?);
                    break;
                }
                // 同一工具二次被拒 ⇒ 回合终止（既有 done{denied} 收口形状）；
                // 被拒终止不提醒。
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
    /// 公开原语不带写/执行记账面（那属回合政策层 [`Runtime::run_turn`] 的
    /// 职责），内部委托 [`Runtime::step_with_write_ledger`]（丢弃记账）。
    pub fn step(
        &self,
        id: Uuid,
        history: &[Message],
        user_text: &str,
        options: &StepOptions,
        denied_tools: &mut BTreeSet<String>,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        let mut write_executed = false;
        self.step_with_write_ledger(
            id,
            history,
            user_text,
            options,
            denied_tools,
            &mut write_executed,
            events,
        )
    }

    /// [`Runtime::step`] 的带记账实现：`write_executed` 由回合政策层持有，
    /// 传入 [`Runtime::execute_tool`] 的执行成功分支置位（交付物提醒的判定
    /// 输入）；语义与公开原语完全一致。
    #[allow(clippy::too_many_arguments)]
    fn step_with_write_ledger(
        &self,
        id: Uuid,
        history: &[Message],
        user_text: &str,
        options: &StepOptions,
        denied_tools: &mut BTreeSet<String>,
        write_executed: &mut bool,
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
        self.execute_tool(
            id,
            &session,
            &name,
            &args,
            denied_tools,
            write_executed,
            events,
        )
    }

    /// 末步缴械的政策准备 + 一步原语调用（金标 opencode `max-steps.ts` +
    /// `llm.ts:226-247` 的 `stepLimitReached` 分支 + `step.ts:91-94`）：到限
    /// 不硬停，把 [`MAX_STEPS_PROMPT`] 作为 assistant 消息**临时**拼进本次
    /// 请求的 history 副本（不写会话历史——它不是真实产出），以
    /// `tool_choice=None` 走 [`Runtime::step_with_write_ledger`]。该步的工具
    /// 调用不执行（不调 [`Runtime::record_tool_call`]、不发待执行占位），按
    /// `step.ts` 的 `executeTool` 回错误结果，走向为 [`StepOutcome::Surrendered`]。
    fn surrender_step(
        &self,
        id: Uuid,
        denied_tools: &mut BTreeSet<String>,
        write_executed: &mut bool,
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
        self.step_with_write_ledger(
            id,
            &history,
            "",
            &StepOptions {
                tool_choice: ToolChoice::None,
            },
            denied_tools,
            write_executed,
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

/// 交付物提醒判定（`loop/run.rs` 的 `needs_reminder` 同款规则下沉）：仅
/// Craft/Expert、任务文本含写意图、本回合（含已跑过的提醒回合）从未成功执行
/// 写/执行工具、且本回合尚未提醒过——四者全满足才提醒。只在自然收敛
/// （`StepOutcome::Converged`）处调用；缴械与被拒终止不走本判定。
fn needs_deliverable_reminder(
    mode: Mode,
    user_input: &str,
    write_executed: bool,
    reminder_used: bool,
) -> bool {
    !reminder_used
        && !write_executed
        && matches!(mode, Mode::Craft | Mode::Expert)
        && DELIVERABLE_KEYWORDS.iter().any(|k| user_input.contains(k))
}
