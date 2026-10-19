//! 回合驱动（适配层）：外层只推进步，历史只在一处组装后再派生成 provider 请求。
//!
//! 结构对齐参考实现（本卡照其**结构**改写，不另创等价方案）：
//!
//! | 参考件 | 职责 | 本层落点 |
//! | --- | --- | --- |
//! | `runner/llm.ts`（`drain` 外层 `while`） | 外层只推进步：`advanceToStep` → `runStep` → `step++`；每步 `context.load` **重读**历史 | [`AgentLoop::run`] 的 `loop`：每步经 [`History`] 组装本次请求 → [`AgentLoop::drive_step`]（一步一调用 + 结算）→ `steps += 1` |
//! | `runner/step.ts`（`attempt`） | 一步之内做过什么、如何判定续步：返回 `Outcome` 标签，策略归外层 | [`StepFlow`]：`Converged` / `ToolSettled` / `Aborted`；本层只分派，不据其再执行工具 |
//! | `session/inbox.ts`（`promote`） | 待办输入提升为下一轮输入 | [`AgentLoop::run`] 的 `user_input` / 提醒注记：首步与提醒步为本轮输入，续步为空串（历史里已有） |
//!
//! 与 [`Runtime::step`](crate::runtime::Runtime::step) 的关系与**尚未承接的缺口**
//! 见 `docs/loop-adapter-status.md`：本层已按原语的走向词汇收敛结构（四态、一步一
//! 调用、工具结算后开新步），但尚不能把 provider/executor 借给 Runtime 执行
//! （`Arc<dyn Provider>` 的 `'static` 约束 + `AgentLoop::new` 的借用签名），
//! 故本层仍是「一步」的执行者。

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::provider::{StepOptions, ToolChoice};
use crate::schema::{EventKind, Mode};

use super::guard::CallOutcome;
use super::support::tool_result;
use super::types::{LoopOutcome, LoopStatus, MAX_STEPS_PROMPT};
use super::AgentLoop;

/// 交付物意图关键词（卡面冻结：创建/写入/修改）。
///
/// **与 `runtime/turns.rs` 的同名表重复**：Runtime 侧那份属 `run_turn` 私有实现，
/// 本层无法改为依赖（见 `docs/loop-adapter-status.md` 缺口 B3），故两处并存；
/// 本卡判明二者不会在同一回合各提醒一次（本层 `run()` 不与 `run_turn` 同时驱动），
/// 故按卡面「若…会导致同一回合提醒两次才删」的条件**保留**本层这份。
pub(super) const DELIVERABLE_KEYWORDS: [&str; 3] = ["创建", "写入", "修改"];

/// 交付物提醒文案（与 `runtime/turns.rs` 的 `DELIVERABLE_REMINDER_NOTE` 逐字一致）。
const DELIVERABLE_REMINDER_NOTE: &str =
    "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

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

/// 回合历史的**唯一组装口**。
///
/// 本层的 provider 请求不再由散落各处的 `messages.push` 直接产出，而是先经本结构
/// 落成一条条带角色的历史条目、再由 [`History::request`] 一次性派生「本次请求」。
/// 缴械步的 [`MAX_STEPS_PROMPT`] 副本也收敛在此（它不进历史，只进请求），
/// 与金标 `llm.ts` 在 `runStep` 组 transcript 时注入的位置一致。
struct History {
    entries: Vec<Value>,
}

impl History {
    /// 回合起始历史：系统消息（模式模板 + 全模式硬规则）+ 本轮用户输入。
    fn start(template: &str, user_input: &str) -> Self {
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
    fn push_assistant(&mut self, text: &str) {
        self.entries
            .push(json!({"role": "assistant", "content": text}));
    }

    /// 交付物提醒注记落地（作为 user 注记进历史，与既有行为一致）。
    fn push_user_note(&mut self, note: &str) {
        self.entries.push(json!({"role": "user", "content": note}));
    }

    /// 工具结果落地。
    fn push_tool_result(&mut self, id: &str, name: &str, result: &Value, is_error: bool) {
        self.entries.push(tool_result(id, name, result, is_error));
    }

    /// 本次 provider 请求 = 当前历史；缴械步在末尾临时追加 [`MAX_STEPS_PROMPT`]
    /// 的 assistant 副本（**不进历史**——它不是真实产出，只为让 provider 收口总结）。
    fn request(&self, surrender: bool) -> Vec<Value> {
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
    /// 外层只推进步（对齐金标 `llm.ts` 的 `drain` while）：每步经 [`History`]
    /// 组装请求，[`AgentLoop::drive_step`] 执行一步并返回 [`StepFlow`]，本函数
    /// 据走向收口。公开签名与 `LoopOutcome` 形状保持不变。
    pub fn run(&mut self, user_input: &str) -> Result<LoopOutcome, String> {
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

            let (flow, text, last_call) = self.drive_step(
                &history.request(false),
                &tool_defs,
                &mut denied_tools,
                &mut history,
            )?;
            steps += 1;
            if let Some(text) = text {
                final_text = text;
            }

            match flow {
                // 本步无工具请求 ⇒ 收敛（或转一次交付物提醒回合）。
                StepFlow::Converged => {
                    if self.needs_reminder(user_input) {
                        self.reminder_used = true;
                        self.emit(
                            EventKind::Thinking.as_str(),
                            json!({ "reminder": DELIVERABLE_REMINDER_NOTE }),
                        );
                        history.push_user_note(DELIVERABLE_REMINDER_NOTE);
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
        let turn = self.provider.chat(request, tool_defs)?;
        let text = if turn.text.is_empty() {
            None
        } else {
            self.emit(EventKind::Text.as_str(), json!({"text": turn.text}));
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
        let turn = self.provider.chat_with_options(
            &history.request(true),
            tool_defs,
            &StepOptions {
                tool_choice: ToolChoice::None,
            },
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
    fn needs_reminder(&self, user_input: &str) -> bool {
        !self.reminder_used
            && !self.write_called
            && matches!(self.mode, Mode::Craft | Mode::Expert)
            && DELIVERABLE_KEYWORDS.iter().any(|k| user_input.contains(k))
    }
}
