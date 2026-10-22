//! 回合执行：send_text（单队列投递）、run_turn（步驱循环）、单步原语
//! [`Runtime::step`](StepOutcome)；结算与记账一族在 `turns_settle.rs`
//! （纯搬移、零行为变更），本文件只保留回合政策：投递、步循环、逐步分派与
//! `done` 收口。投递面的两层语义（steer 插话 / queue 排队 / owner 过期判定）
//! 在 `turn_queue.rs`。
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
//! - 工具步结算（金标 opencode `step.ts:91-134`）：注入执行器时按
//!   [`Runtime::check_tool`] 判定执行；未注入/需审批时保持「待宿主执行」占位，
//!   真实结果由宿主经 [`Runtime::record_tool_result`] 回填；
//! - 到限 ⇒ 末步缴械（`toolChoice=none` 总结步）⇒ `done{max_steps_reached}`；
//! - 自然收敛 ⇒ 交付物提醒（Craft/Expert + 写意图 + 未执行写工具，至多一次）。
//!
//! 分层（对齐金标 `step.ts` / `llm.ts`）：[`Runtime::step`] 是「一步」的完整
//! 原语（读会话 → 组上下文 → 恰好一次 provider 调用 → 消费产出 → 工具结算 →
//! 返回 [`StepOutcome`]）；`run_turn` 只做回合政策——首步历史快照、步循环、
//! 每步 outcome 的分派与 `done` 收口、末步 `TurnReceipt`。步计数、事件累积器
//! 与 deny 记账都由调用方持有，原语不藏全局态。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use uuid::Uuid;

use crate::error::CoreError;
use crate::provider::{StepOptions, ToolChoice};
use crate::schema::{Event, Message, Mode, Role, SessionStatus};

use super::{Runtime, TurnReceipt};

/// 交付物意图关键词（卡面冻结：创建/写入/修改）。
///
/// **单一来源（缺口 B3 的解法）**：本表与 [`should_remind`] 是全仓唯一的提醒规则；
/// `loop/run.rs` 不再持有自己那份副本，改为经 `crate::runtime` 的 re-export
/// （`pub(crate) use`）调用本表与 [`Runtime::should_remind`]。
///
/// [`should_remind`]: Runtime::should_remind
pub(crate) const DELIVERABLE_KEYWORDS: [&str; 3] = ["创建", "写入", "修改"];

/// 交付物提醒文案（**单一来源**：loop 层经 `crate::runtime` re-export 取用）。
pub(crate) const DELIVERABLE_REMINDER_NOTE: &str =
    "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

/// 交付物提醒的记账位（`loop` 层跨路径复用的入参）。
///
/// 有了它，两条驱动路径问的是**同一份规则**：本地路径自持历史与执行，故自行
/// 记账后经本结构传入；`Runtime` 路径的历史与执行都在会话上，记账位由本层从
/// 会话读回的 [`Settled`](crate::r#loop) 面维护。判定本身只有
/// [`Runtime::should_remind`] 一处。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriteLedger {
    /// 本回合（含已跑过的提醒回合）是否成功执行过写/执行类工具。
    pub write_executed: bool,
    /// 提醒回合是否已用过（整个回合至多一次）。
    pub reminder_used: bool,
}

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
    ///
    /// 投递面（落 user 消息 + 发 user 事件 + **回合边界队列项**的取用）在
    /// `turn_queue.rs`：本方法只负责取会话投递锁并把整段投递交出去；回合中
    /// 的插话（steer）与回合后的排队（queue）由该模块的队列承接。
    pub fn send_text(&self, id: Uuid, text: &str) -> Result<TurnReceipt, CoreError> {
        let drain = self.drain_lock(id)?;
        let _queued = drain.lock().expect("会话投递锁不得毒化");
        self.deliver_and_run(id, text)
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
    pub(super) fn run_turn(
        &self,
        id: Uuid,
        text: &str,
        history: &[Message],
        first_event: Event,
    ) -> Result<TurnReceipt, CoreError> {
        // 取消令牌：回合开始复位（防上一轮取消吃掉本轮）；`_cancel_reset` 在
        // 正常/取消/失败（含 `?` 提前返回）退出时都复位。
        let cancel = self.cancel_token(id)?;
        cancel.store(false, Ordering::SeqCst);
        let _cancel_reset = CancelReset(cancel.clone());
        let turn_start = history.len();
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
            // 取消只在步与步之间生效：本步正在执行的工具必须跑完结算
            // （金标 step.ts:141「Keep the final tool and Step events uninterruptible」）。
            if cancel.load(Ordering::SeqCst) {
                self.settle_cancelled(id, &mut events, steps, turn_start)?;
                break;
            }
            // 步边界取一次插话队列项（steer 的消费点）：注入进历史 ⇒ 本步可见。
            self.consume_steer(id, &mut events)?;
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
                    if self.should_remind(
                        session.mode,
                        text,
                        WriteLedger {
                            write_executed,
                            reminder_used,
                        },
                    ) {
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
                        let ts = self.turn_ts();
                        let session =
                            self.push_message(id, Role::User, reminder_text.clone(), None, ts)?;
                        events.push(self.emit(
                            &session,
                            "user",
                            json!({ "text": reminder_text, "tsMs": ts }),
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
    /// `MAX_STEPS_PROMPT` 副本」且 `tool_choice=None`。`tool_choice=None` 时
    /// 本步工具请求不执行、按金标 `step.ts:91-94` 回错误结果（不落 ToolCall
    /// 消息），走向为 [`StepOutcome::Surrendered`]。
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
    pub(super) fn step_with_write_ledger(
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
        self.record_provider_usage(id, events)?;

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
        let ts = self.turn_ts();
        let (session, args_ref) = self.push_tool_call(id, &name, &args, ts)?;
        let args = self.redactor.redact_json(&args);
        // payload 自带消息时间（`tsMs`，与历史消息同一次读数）与 blob 引用
        // （`argsRef`，可为 null）：事件因此自足——重启重建无需回读运行期内存。
        events.push(self.emit(
            &session,
            "tool_use",
            json!({
                "name": name.clone(),
                "args": args.clone(),
                "argsRef": args_ref,
                "tsMs": ts,
            }),
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

    /// 交付物提醒文案（**单一来源**：`loop` 层两条路径都取这里这一份）。
    pub(crate) fn deliverable_reminder_note(&self) -> &'static str {
        DELIVERABLE_REMINDER_NOTE
    }
}

/// 回合退出（正常/取消/失败）复位取消标志；`Drop` 覆盖 `?` 提前返回路径。
struct CancelReset(Arc<AtomicBool>);

impl Drop for CancelReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 交付物提醒规则（**单一来源**，缺口 B3 的解法）：仅 Craft/Expert、任务文本
/// 含写意图、本回合（含已跑过的提醒回合）从未成功执行写/执行工具、且本回合尚未
/// 提醒过——四者全满足才提醒。只在自然收敛（`StepOutcome::Converged`）处调用；
/// 缴械与被拒终止不走本判定。
///
/// `pub(crate)` 的**纯函数**：[`Runtime::should_remind`] 与 `loop` 层两条驱动路径
/// 都调它，关键词表与规则因此**只有本处一份**。
pub(crate) fn deliverable_reminder(mode: Mode, user_input: &str, write: WriteLedger) -> bool {
    !write.reminder_used
        && !write.write_executed
        && matches!(mode, Mode::Craft | Mode::Expert)
        && DELIVERABLE_KEYWORDS.iter().any(|k| user_input.contains(k))
}

impl Runtime {
    /// [`deliverable_reminder`] 的方法形式（`loop` 的 `Runtime` 路径经此判定）。
    pub(crate) fn should_remind(&self, mode: Mode, user_input: &str, write: WriteLedger) -> bool {
        deliverable_reminder(mode, user_input, write)
    }
}
