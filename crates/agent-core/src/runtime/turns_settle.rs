//! 回合结算与记账一族（自 `turns.rs` 按职责拆出，函数体逐字搬移、零行为变更）。
//!
//! 收口三块职责：
//! - **步产出消费与工具结算**：`consume_step` / `push_tool_call` /
//!   `record_tool_result` / `record_tool_call` / `settle_tool_call` / `check_tool`；
//! - **步限责任结算**：`max_turn_steps` / `surrender_step` / `reject_disabled_tool`
//!   （金标 `step.ts:91-94` 的 `executeTool` 拒绝结算）；
//! - **取消结算**：`settle_cancelled`（金标 `step.ts:205-225` 的
//!   `failUnsettledTools(TOOLS_INTERRUPTED)` + `failAssistant(STEP_INTERRUPTED)`）。
//!
//! 公开路径不变：被搬走的 `pub` / `pub(crate)` 方法仍是 `impl Runtime` 上的同名
//! 方法，外部调用点（`execute.rs` / gateway / 测试）一行不改；模块内互调所需
//! 的私有方法放宽到 `pub(super)`。

use std::collections::BTreeSet;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::permission::Decision;
use crate::provider::{ProviderStep, StepOptions, ToolChoice};
use crate::r#loop::MAX_STEPS_PROMPT;
use crate::schema::{Event, Message, Role, Session, SessionStatus, ToolInfo};

use super::turns::StepOutcome;
use super::{now_ms, Runtime};

/// 被中断工具的固定错误对象（金标 `step.ts:62` 的 `TOOLS_INTERRUPTED` 逐字）。
pub(super) const TOOLS_INTERRUPTED: &str =
    r#"{"type":"aborted","message":"Tool execution interrupted"}"#;

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
    pub(super) fn max_turn_steps(&self) -> usize {
        self.config
            .lock()
            .map(|c| c.max_turn_steps)
            .unwrap_or(crate::config::DEFAULT_MAX_TURN_STEPS)
    }

    /// 末步缴械的政策准备 + 一步原语调用（金标 opencode `max-steps.ts` +
    /// `llm.ts:226-247` 的 `stepLimitReached` 分支 + `step.ts:91-94`）：到限
    /// 不硬停，把 [`MAX_STEPS_PROMPT`] 作为 assistant 消息**临时**拼进本次
    /// 请求的 history 副本（不写会话历史——它不是真实产出），以
    /// `tool_choice=None` 走 [`Runtime::step_with_write_ledger`]。该步的工具
    /// 调用不执行（不调 [`Runtime::record_tool_call`]、不发待执行占位），按
    /// `step.ts` 的 `executeTool` 回错误结果，走向为 [`StepOutcome::Surrendered`]。
    pub(super) fn surrender_step(
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
    pub(super) fn reject_disabled_tool(
        &self,
        id: Uuid,
        _session: &Session,
        name: &str,
        args: Value,
        events: &mut Vec<Event>,
    ) -> Result<(), CoreError> {
        let error = json!({
            "type": "error",
            "message": "Tools are disabled after the maximum agent steps",
        });
        // 结算落消息，取回型载荷（含消息时间）后补事件 kind 发出：消息 `ts_ms` 与
        // 事件 `payload.tsMs` 同一次读数，重建取回真值。
        let (_message, mut payload) = self.push_tool_result(id, name, args, &error.to_string())?;
        payload["result"] = error;
        payload["isError"] = json!(true);
        let session = self.get_session(id)?;
        events.push(self.emit(&session, "tool_result", payload)?);
        Ok(())
    }

    /// 消费一步的产出：逐条落事件，遇工具调用或结束即止（批内尾随项属新一步）。
    /// 返回本步终结处的工具调用；`None` 表示本步无工具调用 ⇒ 回合收敛。
    pub(super) fn consume_step(
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
                    // 同一次读数：消息 `ts_ms` 与事件 `payload.tsMs` 必然相等。
                    let ts = self.turn_ts();
                    let session =
                        self.push_message(id, Role::Assistant, reply.clone(), None, ts)?;
                    events.push(self.emit(
                        &session,
                        "text",
                        json!({ "text": reply, "tsMs": ts }),
                    )?);
                }
                // 本步止于首个工具调用；同批尾随项不在此消费。
                ProviderStep::ToolCall { name, args } => return Ok(Some((name, args))),
                ProviderStep::Finished => return Ok(None),
            }
        }
        Ok(None)
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
        // 消息先落：emit 发生会话快照，取重读后的（`push_tool_result` 内已写入消息）。
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
        Ok(self.push_tool_call(id, tool, args, self.turn_ts())?.0)
    }

    /// 落一条 user / assistant 文本消息进会话（缺口 B4 的解法）。
    ///
    /// `Runtime` 此前只公开了工具类写消息面（[`Runtime::record_tool_call`] /
    /// [`Runtime::record_tool_result`]）；user/assistant 文本只能经
    /// [`Runtime::send_text`] 落，而它会自行跑完整个回合，不适合逐步驱动的宿主。
    /// 本方法把「写一条文本消息」这一最小公开面补上，**内部复用既有
    /// [`Runtime::push_message`]**（签名与可见性均不变），使 `loop` 的
    /// `with_runtime` 路径能把历史真正落在会话上。
    ///
    /// 时间取一次时钟读数（与 [`Runtime::turn_ts`] 同一口径）；消息本身不落事件
    /// （事件由调用方按自己的可见面决定发不发，与 `loop` 既有的 `text` 事件形状解耦）。
    pub fn append_message(
        &self,
        id: Uuid,
        role: Role,
        content: &str,
    ) -> Result<Session, CoreError> {
        self.push_message(id, role, content.to_string(), None, self.turn_ts())
    }

    /// 经 `Runtime` 的 provider 面发起**一次** provider 调用（缺口 B1 的跑通件）。
    ///
    /// `loop` 的 `with_runtime` 路径不直接持有 `Provider`（注入面为
    /// `Arc<dyn Provider>`，隐含 `'static`），故经本方法转调：上下文由
    /// [`Runtime::build_turn_context`] 按 `session` 的模式与记忆组装（与
    /// [`Runtime::send_text`] 的 `step` 同款口径，对齐金标 `session/context.ts`），
    /// 历史取调用方给的 `history`（即 `loop` 每步从会话**重读**的那一份），
    /// `user_text` 为空（续步口径：本轮输入早已作为会话消息在 `history` 里）。
    ///
    /// 返回原始 [`ProviderStep`] 序列，由 `loop` 侧译成其 [`ChatTurn`]
    /// （`ProviderStep::ToolCall` 无 id，故 id 在 `loop` 侧按序合成；缺口 B5）。
    ///
    /// [`ChatTurn`]: crate::r#loop::ChatTurn
    pub(crate) fn chat_turn(
        &self,
        id: Uuid,
        history: &[Message],
        options: Option<&StepOptions>,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let session = self.get_session(id)?;
        if matches!(session.status, SessionStatus::Done | SessionStatus::Failed) {
            return Err(CoreError::SessionClosed(id));
        }
        let context = self.build_turn_context(&session, history, "");
        // `chat_turn` 路径（`loop` 适配器）没有回合事件累加器：`model.retry` 事件
        // 仍落持久日志（durable），但不进任何 receipt（该路径本就不产出 receipt）。
        let mut retry_events = Vec::new();
        self.call_provider_with_retry(&context, options, &mut retry_events)
    }

    /// 结算**一次**工具调用（缺口 B1：`with_runtime` 路径的工具由 Runtime 执行）。
    ///
    /// 与 [`Runtime::step_with_write_ledger`] 的工具分支同款口径，但**不含**
    /// provider 调用——`loop` 侧已在同一步里经 [`Runtime::chat_turn`] 取回工具请求，
    /// 本方法只负责「落调用 → 门检 → 执行 → 结算结果」（金标 `step.ts:91-134`），
    /// 使结果与调用成对进会话历史，下一步重读可复原。返回原语走向（正常为
    /// [`StepOutcome::ToolSettled`]；`loop` 侧按每步一次调用、每次传入空 deny
    /// 账本，故不会在此触发 `Aborted`）。
    ///
    /// `events` 是 Runtime 自己的事件面（落 `EventLog`）；与 `loop` 侧按既有形状
    /// 补发的事件是两个不同的可见面（缺口 B5，见 `docs/loop-adapter-status.md`）。
    pub(crate) fn settle_tool_call(
        &self,
        id: Uuid,
        name: &str,
        args: &Value,
    ) -> Result<StepOutcome, CoreError> {
        let ts = self.turn_ts();
        let (session, args_ref) = self.push_tool_call(id, name, args, ts)?;
        let args = self.redactor.redact_json(args);
        let mut events = vec![self.emit(
            &session,
            "tool_use",
            json!({
                "name": name,
                "args": args.clone(),
                "argsRef": args_ref,
                "tsMs": ts,
            }),
        )?];
        // 单次调用、空 deny 账本：本步只结算一次，引擎侧不会在此熔断。
        let mut denied: BTreeSet<String> = BTreeSet::new();
        let mut write_executed = false;
        self.execute_tool(
            id,
            &session,
            name,
            &args,
            &mut denied,
            &mut write_executed,
            &mut events,
        )
    }

    /// [`Runtime::record_tool_call`] 的实现体：**消息时间由调用方取一次时钟读数并显式
    /// 传入**，与随后 `tool_use` 事件 payload 的 `"tsMs"` 用同一次读数（理由同
    /// [`Runtime::turn_ts`]）。返回 `(会话快照, blob 引用)`，`args_ref` 由调用方随事件
    /// payload 的 `"argsRef"` 一起发出，使事件自足、重建能取回同一个引用。
    pub(super) fn push_tool_call(
        &self,
        id: Uuid,
        tool: &str,
        args: &Value,
        ts_ms: u64,
    ) -> Result<(Session, Option<String>), CoreError> {
        let args = self.redactor.redact_json(args);
        let args_ref = self
            .blobs
            .put(args.to_string().as_bytes())
            .map(|r| r.0)
            .ok();
        let summary = self.redactor.summarize_args(&args);
        let session = self.push_message(
            id,
            Role::ToolCall,
            summary,
            Some(ToolInfo {
                name: tool.to_string(),
                args_ref: args_ref.clone(),
                result_ref: None,
                ms: None,
            }),
            ts_ms,
        )?;
        Ok((session, args_ref))
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

    /// 取消结算（金标 `step.ts:141-153` + `205-225` 的不可中断收口段）：
    /// 1. 本回合 `turn_start` 之后若有**未结算**工具调用，按
    ///    `failUnsettledTools(TOOLS_INTERRUPTED)` 逐条结算成
    ///    `tool_result{isError:true,type:"aborted"}`；
    /// 2. 以 `done{status:"cancelled"}` 收口；
    /// 3. 运行态 `turn_completed → idle`（见 [`Runtime::settle_turn_completed`]）。
    ///
    /// 本仓 provider 与工具执行都是同步调用，步边界上正常不会留下未结算调用；
    /// 该分支按金标保留，用于「调用已进历史、结果未回填」的防御性结算。
    pub(super) fn settle_cancelled(
        &self,
        id: Uuid,
        events: &mut Vec<Event>,
        steps: usize,
        turn_start: usize,
    ) -> Result<(), CoreError> {
        for (name, args) in self.unsettled_tool_calls(id, turn_start)? {
            let error: Value = serde_json::from_str(TOOLS_INTERRUPTED)
                .expect("TOOLS_INTERRUPTED 必须是合法 JSON 对象");
            let (_message, mut payload) =
                self.push_tool_result(id, &name, args, TOOLS_INTERRUPTED)?;
            payload["result"] = error;
            payload["isError"] = json!(true);
            let session = self.get_session(id)?;
            events.push(self.emit(&session, "tool_result", payload)?);
        }
        let session = self.get_session(id)?;
        events.push(self.emit(
            &session,
            "done",
            json!({ "status": "cancelled", "steps": steps }),
        )?);
        self.settle_turn_completed(id);
        Ok(())
    }

    /// 本回合（`turn_start` 消息下标之后）尚未结算的工具调用。
    ///
    /// ToolCall / ToolResult 在历史里按序成对出现；未被 ToolResult 配对的
    /// ToolCall 即 `opencode` 语义里的 unsettled。参数从 `args_ref` 的 blob
    /// 取回（ToolCall 消息只存摘要），取不到时用 `null`（不阻断结算）。
    fn unsettled_tool_calls(
        &self,
        id: Uuid,
        turn_start: usize,
    ) -> Result<Vec<(String, Value)>, CoreError> {
        let session = self.get_session(id)?;
        let mut pending: Vec<ToolInfo> = Vec::new();
        for message in session.messages.iter().skip(turn_start) {
            match message.role {
                Role::ToolCall => {
                    if let Some(tool) = &message.tool {
                        pending.push(tool.clone());
                    }
                }
                // 结果与最近一次未结算调用配对（本仓工具步串行，一次一个）。
                Role::ToolResult => {
                    pending.pop();
                }
                Role::User | Role::Assistant => {}
            }
        }
        Ok(pending
            .into_iter()
            .map(|tool| {
                let args = tool
                    .args_ref
                    .as_deref()
                    .and_then(|reference| self.blobs.get(reference).ok())
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or(Value::Null);
                (tool.name, args)
            })
            .collect())
    }
}
