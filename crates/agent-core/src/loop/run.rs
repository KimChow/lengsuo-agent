//! 回合驱动（适配层）：外层只推进步，历史只在一处组装后再派生成 provider 请求。
//!
//! 结构对齐参考实现（本卡照其**结构**改写，不另创等价方案）：
//!
//! | 参考件 | 职责 | 本层落点 |
//! | --- | --- | --- |
//! | `runner/llm.ts`（`drain` 外层 `while`） | 外层只推进步：`advanceToStep` → `runStep` → `step++`；每步 `context.load` **重读**历史 | [`AgentLoop::run`] 的 `loop`：每步经 [`AgentLoop::load_step`] **重读**本轮请求 → [`AgentLoop::drive_step`]（一步一调用 + 结算）→ `steps += 1` |
//! | `runner/step.ts`（`attempt`） | 一步之内做过什么、如何判定续步：返回 `Outcome` 标签，策略归外层 | [`StepFlow`]：`Converged` / `ToolSettled` / `PlanRejected` / `FatalDenied`；本层只分派，不据其再执行工具 |
//! | `session/inbox.ts`（`promote`） | 待办输入提升为下一轮输入 | [`AgentLoop::run`] 的 `user_input` / 提醒注记：首步与提醒步为本轮输入，续步为空串（历史里已有） |
//!
//! 与 [`Runtime::step`](crate::runtime::Runtime::step) 的关系见
//! `docs/loop-adapter-status.md`：本层已按原语的走向词汇收敛结构（四态、一步一
//! 调用、工具结算后开新步），并提供两条执行路径——[`AgentLoop::new`] 的本地自驱，
//! 与 [`AgentLoop::with_runtime`] 的 `Runtime` 驱动（历史落在**会话**上、工具由
//! `Runtime` 结算）。

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::provider::{StepOptions, ToolChoice};
use crate::runtime::WriteLedger;
use crate::schema::EventKind;

use super::guard::CallOutcome;
use super::support::tool_result;
use super::types::{LoopOutcome, LoopStatus, MAX_STEPS_PROMPT};
use super::{AgentLoop, Driver};

/// 一步原语的返回：`(走向, 本步文本, 本步最后一个工具请求的 (id, name))`。
///
/// 文本由外层决定是否作为 `final_text`；最后一个工具请求供终止路径回填被拒结果。
type StepReport = (StepFlow, Option<String>, Option<(String, String)>);

/// 一步的走向（对齐 [`Runtime::step`](crate::runtime::Runtime::step) 的
/// `StepOutcome` 四态：原语只报告本步发生了什么，回合推进/收口归外层）。
enum StepFlow {
    /// 本步无工具请求 ⇒ 回合收敛。
    Converged,
    /// 工具已结算（真实执行/执行失败/首次被拒）⇒ 可开下一步。
    ToolSettled,
    /// 规划未获批准（`plan.exit` 的 `approved != true`）⇒ 终止。
    PlanRejected(String),
    /// 同一工具二次被拒（防对同一被拒工具空转）⇒ 终止。
    FatalDenied { tool: String },
}

/// 本地路径（[`Driver::Local`]）回合历史的**唯一组装口**。
///
/// provider 请求不再由散落各处的 `messages.push` 直接产出，而是先经本结构落成
/// 一条条带角色的历史条目、再由 [`History::request`] 一次性派生「本次请求」。
/// 缴械步的 [`MAX_STEPS_PROMPT`] 副本也收敛在此（它不进历史，只进请求），
/// 与金标 `llm.ts` 在 `runStep` 组 transcript 时注入的位置一致。
///
/// [`Driver::Runtime`] 路径**不用**本结构：那条路径的历史容器就是会话本身，
/// [`AgentLoop::load_step`] 每步从 `runtime.get_session(id).messages` 重读。
pub(super) struct History {
    entries: Vec<Value>,
}

impl History {
    /// 回合起始历史：系统消息（模式模板 + 全模式硬规则）+ 本轮用户输入。
    pub(super) fn start(template: &str, user_input: &str) -> Self {
        Self {
            entries: vec![
                json!({
                    "role": "system",
                    "content": format!("{}\n\n{}", template, crate::mode::HARD_RULES),
                }),
                json!({ "role": "user", "content": user_input }),
            ],
        }
    }

    /// assistant 文本落地（模型确实产出的内容进历史；空文本不进）。
    pub(super) fn push_assistant(&mut self, text: &str) {
        self.entries
            .push(json!({"role": "assistant", "content": text}));
    }

    /// 交付物提醒注记落地（作为 user 注记进历史，与既有行为一致）。
    pub(super) fn push_user_note(&mut self, note: &str) {
        self.entries.push(json!({"role": "user", "content": note}));
    }

    /// 工具结果落地。
    pub(super) fn push_tool_result(
        &mut self,
        id: &str,
        name: &str,
        result: &Value,
        is_error: bool,
    ) {
        self.entries.push(tool_result(id, name, result, is_error));
    }

    /// 本次 provider 请求 = 当前历史；缴械步在末尾临时追加 [`MAX_STEPS_PROMPT`]
    /// 的 assistant 副本（**不进历史**——它不是真实产出，只为让 provider 收口总结）。
    pub(super) fn request(&self, surrender: bool) -> Vec<Value> {
        let mut request = self.entries.clone();
        if surrender {
            request.push(json!({ "role": "assistant", "content": MAX_STEPS_PROMPT }));
        }
        request
    }
}

impl<'a> AgentLoop<'a> {
    /// 跑一个完整回合序列。
    ///
    /// 外层只推进步（对齐金标 `llm.ts` 的 `drain` while）：每步先
    /// [`AgentLoop::load_step`] **重读**本轮请求（本地路径重读组装口，`Runtime`
    /// 路径重读会话消息），再 [`AgentLoop::drive_step`] 执行一步并返回 [`StepFlow`]，
    /// 本函数据走向收口。公开签名与 `LoopOutcome` 形状保持不变。
    pub fn run(&mut self, user_input: &str) -> Result<LoopOutcome, String> {
        // `Runtime` 路径：本轮输入先落会话——历史容器即会话，用户的这条消息也是
        // 历史的一部分（系统提示由 Runtime 组上下文时按会话模式生成）。本地路径无操作。
        self.begin_turn(user_input)?;
        let mut history = History::start(self.policy.template, user_input);
        let mut tool_defs = self.tool_defs();
        let mut steps = 0usize;
        let mut final_text = String::new();
        let mut denied_tools: BTreeSet<String> = BTreeSet::new();

        loop {
            if steps >= self.limits.max_steps {
                // 到限不硬停：走一次缴械步（toolChoice=none 的纯文本总结步）。
                return self.run_surrender_step(&history, &tool_defs, steps, final_text);
            }

            // 每步从历史真相源**重新派生**请求（本地路径=组装口；Runtime 路径=会话）。
            let request = self.load_step(&history, false)?;
            let (flow, text, last_call) =
                self.drive_step(&request, &tool_defs, &mut denied_tools, &mut history)?;
            steps += 1;
            if let Some(text) = text {
                final_text = text;
            }

            match flow {
                // 本步无工具请求 ⇒ 收敛（或转一次交付物提醒回合）。
                StepFlow::Converged => {
                    if self.needs_reminder(user_input) {
                        self.reminder_used = true;
                        let note = self.reminder_note();
                        self.emit(EventKind::Thinking.as_str(), json!({ "reminder": note }));
                        self.append_user_note(note)?;
                        history.push_user_note(note);
                        continue;
                    }
                    self.emit(
                        EventKind::Done.as_str(),
                        json!({ "status": "completed", "steps": steps }),
                    );
                    return Ok(LoopOutcome {
                        status: LoopStatus::Completed,
                        steps,
                        final_text,
                    });
                }
                // 工具已结算 ⇒ 开下一步；门禁/白名单可能在 `plan.enter` /
                // `plan.exit` 后变化：重建可见工具面。
                StepFlow::ToolSettled => {
                    tool_defs = self.tool_defs();
                }
                // 规划未获批准 ⇒ 终止（既有 `aborted` 收口形状）。
                StepFlow::PlanRejected(reason) => {
                    let denied = json!({"status": "denied", "reason": reason});
                    let (id, name) = last_call.expect("PlanRejected 必有当前调用");
                    history.push_tool_result(&id, &name, &denied, true);
                    self.emit(
                        EventKind::ToolResult.as_str(),
                        json!({"id": id, "name": name, "result": denied, "isError": true}),
                    );
                    self.emit(
                        EventKind::Done.as_str(),
                        json!({ "status": "aborted", "steps": steps, "tool": name }),
                    );
                    return Ok(LoopOutcome {
                        status: LoopStatus::Aborted,
                        steps,
                        final_text,
                    });
                }
                // 同一工具二次被拒 ⇒ 终止（既有熔断语义）。
                StepFlow::FatalDenied { tool } => {
                    self.emit(
                        EventKind::Done.as_str(),
                        json!({ "status": "denied", "steps": steps, "tool": tool }),
                    );
                    return Ok(LoopOutcome {
                        status: LoopStatus::Denied { tool },
                        steps,
                        final_text,
                    });
                }
            }
        }
    }

    /// 执行一步（对齐金标 `step.ts` 的 `attempt`）：恰好一次 provider 调用 →
    /// 产出消费（文本落历史与事件；工具请求逐条结算）→ 返回
    /// `(走向, 本步文本, 本步最后一个工具请求的 (id, name))`。
    ///
    /// 本步文本由外层决定是否作为 `final_text`；`last_call` 供终止路径回填被拒结果。
    fn drive_step(
        &mut self,
        request: &[Value],
        tool_defs: &[Value],
        denied_tools: &mut BTreeSet<String>,
        history: &mut History,
    ) -> Result<StepReport, String> {
        let turn = self.chat(request, tool_defs, None)?;
        let text = if turn.text.is_empty() {
            None
        } else {
            self.emit(EventKind::Text.as_str(), json!({"text": turn.text}));
            // `Runtime` 路径：assistant 文本落会话；本地路径无操作（组装口负责）。
            self.append_assistant(&turn.text)?;
            history.push_assistant(&turn.text);
            Some(turn.text.clone())
        };

        if turn.tool_calls.is_empty() {
            return Ok((StepFlow::Converged, text, None));
        }

        let mut last_call = None;
        for call in &turn.tool_calls {
            self.emit(
                EventKind::ToolUse.as_str(),
                json!({"id": call.id, "name": call.name, "args": call.args}),
            );
            last_call = Some((call.id.clone(), call.name.clone()));
            match self.handle_call(call, denied_tools)? {
                CallOutcome::Aborted(reason) => {
                    return Ok((StepFlow::PlanRejected(reason), text, last_call))
                }
                CallOutcome::FatalDenied => {
                    return Ok((
                        StepFlow::FatalDenied {
                            tool: call.name.clone(),
                        },
                        text,
                        last_call,
                    ))
                }
                CallOutcome::Handled { result, is_error } => {
                    history.push_tool_result(&call.id, &call.name, &result, is_error);
                    self.emit(
                        EventKind::ToolResult.as_str(),
                        json!({"id": call.id, "name": call.name, "result": result, "isError": is_error}),
                    );
                }
                // `Runtime` 路径：结果已由 Runtime 结算进会话（本层不再写历史），
                // 本层只补发既有形状的事件。
                CallOutcome::RuntimeSettled(settled) => {
                    self.record_write(&call.name, &settled);
                    self.emit(
                        EventKind::ToolResult.as_str(),
                        json!({
                            "id": call.id,
                            "name": call.name,
                            "result": settled.preview,
                            "isError": settled.is_error,
                        }),
                    );
                }
            }
        }
        Ok((StepFlow::ToolSettled, text, last_call))
    }

    /// 末步缴械（金标 opencode `max-steps.ts` + `llm.ts:226-247` 的
    /// `stepLimitReached` 分支 + `step.ts:91-94`）：到限不硬停，把
    /// [`MAX_STEPS_PROMPT`] 作为 assistant 消息追加进**本次请求**末尾（不进
    /// 进行中的消息历史——它不是真实产出），以 `toolChoice=none` 再发一次真实
    /// provider 调用并计入 steps；工具定义保留在请求里以维持 provider 的缓存
    /// 前缀。该步若仍返回工具调用 ⇒ 不执行，按 `step.ts` 的 `executeTool` 回
    /// 错误结果，随后以 `done{max_steps_reached}` 收口。
    fn run_surrender_step(
        &mut self,
        history: &History,
        tool_defs: &[Value],
        mut steps: usize,
        mut final_text: String,
    ) -> Result<LoopOutcome, String> {
        let request = self.load_step(history, true)?;
        let turn = self.chat(
            &request,
            tool_defs,
            Some(&StepOptions {
                tool_choice: ToolChoice::None,
            }),
        )?;
        steps += 1;
        if !turn.text.is_empty() {
            self.emit(EventKind::Text.as_str(), json!({"text": turn.text}));
            final_text = turn.text.clone();
        }
        for call in &turn.tool_calls {
            self.emit(
                EventKind::ToolUse.as_str(),
                json!({"id": call.id, "name": call.name, "args": call.args}),
            );
            let error = json!({
                "type": "error",
                "message": "Tools are disabled after the maximum agent steps",
            });
            self.emit(
                EventKind::ToolResult.as_str(),
                json!({"id": call.id, "name": call.name, "result": error, "isError": true}),
            );
        }
        self.emit(
            EventKind::Done.as_str(),
            json!({
                "status": "max_steps_reached",
                "steps": steps,
                "maxSteps": self.limits.max_steps,
            }),
        );
        Ok(LoopOutcome {
            status: LoopStatus::MaxStepsReached,
            steps,
            final_text,
        })
    }

    /// 交付物提醒：仅 Craft/Expert、任务文本含写意图、且从未成功调用写/执行工具。
    ///
    /// 两条路径问的是**同一份规则**（缺口 B3）：判定与关键词表只有
    /// [`Runtime::should_remind`](crate::runtime::Runtime::should_remind) 一处，
    /// 本地路径自持记账位、`Runtime` 路径从会话读回记账位，都经它判定。
    ///
    /// 模式取**本层**的当前模式：`plan.exit` 批准会把本层的模式切到 Craft，而会话
    /// 上的模式是建会话时定的、本层驱动期间没有回写（第三处缺口，见
    /// `docs/loop-adapter-status.md`）。
    fn needs_reminder(&self, user_input: &str) -> bool {
        let write = WriteLedger {
            write_executed: self.write_called,
            reminder_used: self.reminder_used,
        };
        match &self.driver {
            // 本地路径无 `Runtime` 句柄，直接调同一份**纯函数**规则（单一来源，
            // 非复刻：关键词表与判定都在 `runtime::deliverable_reminder` 一处）。
            Driver::Local { .. } => {
                crate::runtime::deliverable_reminder(self.mode, user_input, write)
            }
            Driver::Runtime { runtime, .. } => runtime.should_remind(self.mode, user_input, write),
        }
    }

    /// 交付物提醒文案（**单一来源**：两条路径都取 Runtime 那一份常量）。
    fn reminder_note(&self) -> &'static str {
        match &self.driver {
            Driver::Local { .. } => crate::runtime::DELIVERABLE_REMINDER_NOTE,
            Driver::Runtime { runtime, .. } => runtime.deliverable_reminder_note(),
        }
    }

    /// 结算记账：把一次由 Runtime 结算的调用并进本层的写/执行记账位。
    fn record_write(&mut self, name: &str, settled: &super::guard::Settled) {
        if settled.write_called && super::support::is_write_tool(name) {
            self.write_called = true;
        }
    }
}
