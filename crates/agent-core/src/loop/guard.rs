//! 单工具请求的门禁处理：白名单 + 规划态拒绝 + plan.enter/plan.exit + 执行分发。
//!
//! 门禁判定（白名单 / 规划态）两条路径都是本层的产品行为；**执行分发**按
//! [`Driver`](super::Driver) 分派：本地路径调宿主借进来的执行器，`Runtime` 路径把
//! 调用交给 `Runtime` 结算（见 [`AgentLoop::settle_call_via_runtime`]）。

use std::collections::BTreeSet;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::mode::ModePolicy;
use crate::runtime::Runtime;
use crate::schema::{EventKind, Mode};

use super::support::is_write_tool;
use super::types::{ToolCall, ToolExecutor};
use super::{AgentLoop, Driver};

/// 工具请求的处理结果。
pub(super) enum CallOutcome {
    /// 已产生回填结果（本地路径：结果由本层写历史）。
    Handled { result: Value, is_error: bool },
    /// 规划未获批准 ⇒ 终止。
    Aborted(String),
    /// 同一工具重复被拒 ⇒ 终止。
    FatalDenied,
    /// `Runtime` 路径：结果已由 `Runtime` 结算进会话，本层**不再写历史**，
    /// 只补发既有形状的 `tool_result` 事件。
    RuntimeSettled(Settled),
}

/// `Runtime` 路径下，一次由 Runtime 结算的工具步在会话上留下的可观测面。
///
/// 判定面（`is_error` / `write_called`）取自**会话消息本身**，不是本层的副本：
/// 这样「历史真相源在会话」这条不变式对结算结果同样成立。
pub(super) struct Settled {
    /// 结果上是否标了错误（决定 `tool_result` 事件的 `isError`）。
    pub(super) is_error: bool,
    /// 本步是否成功执行了写/执行类工具（交付物提醒的记账位之一）。
    pub(super) write_called: bool,
    /// 会话里那条 `ToolResult` 消息的内容（供本层补发既有形状的事件用；
    /// `Runtime` 已把大输出收窄成 preview，故此处天然是摘要）。
    pub(super) preview: String,
}

impl<'a> AgentLoop<'a> {
    /// 单工具请求：白名单 + 规划门禁 + 执行（按路径分发）。
    pub(super) fn handle_call(
        &mut self,
        call: &ToolCall,
        denied: &mut BTreeSet<String>,
    ) -> Result<CallOutcome, String> {
        // 门禁判定两条路径一致：先按本层白名单 / 规划态算拒绝原因。
        if let Some(reason) = self.refusal(&call.name) {
            // 同一工具被拒后再次请求 ⇒ 终止，避免空转。
            if denied.contains(&call.name) {
                return Ok(CallOutcome::FatalDenied);
            }
            denied.insert(call.name.clone());
            let result = json!({"status": "denied", "reason": reason});
            return self.settle_in_place(call, result, true);
        }
        if call.name == crate::mode::PLAN_ENTER {
            self.enter_plan();
            let result = json!({"status": "planning", "mode": self.mode.as_str()});
            return self.settle_in_place(call, result, false);
        }
        if call.name == crate::mode::PLAN_EXIT {
            let approved = call
                .args
                .get("approved")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.plan_gate.approved = approved;
            if !approved {
                return Ok(CallOutcome::Aborted(
                    "plan.exit approved=false：规划未获批准，本次执行终止".to_string(),
                ));
            }
            self.exit_plan_to_craft();
            let result = json!({"status": "approved", "mode": self.mode.as_str()});
            return self.settle_in_place(call, result, false);
        }
        // 真实工具：按路径执行。
        match &self.driver {
            Driver::Local { .. } => self.execute_locally(call),
            Driver::Runtime { .. } => self.settle_call_via_runtime(call),
        }
    }

    /// 本地路径：调用宿主借进来的执行器。
    fn execute_locally(&mut self, call: &ToolCall) -> Result<CallOutcome, String> {
        let Some(executor) = self.step_executor() else {
            return Ok(CallOutcome::Handled {
                result: json!({"status": "error", "message": "本层没有可用的工具执行器"}),
                is_error: true,
            });
        };
        match executor.execute(&call.name, &call.args) {
            Ok(value) => {
                if is_write_tool(&call.name) {
                    self.write_called = true;
                }
                Ok(CallOutcome::Handled {
                    result: value,
                    is_error: false,
                })
            }
            Err(message) => Ok(CallOutcome::Handled {
                result: json!({"status": "error", "message": message}),
                is_error: true,
            }),
        }
    }

    /// 本层已定结果的落地（拒绝 / plan.enter / plan.exit）。
    ///
    /// 本地路径：结果交回 [`AgentLoop::drive_step`] 经组装口写进本层历史。
    /// `Runtime` 路径：本层历史不是真相源，结果必须写进**会话**——用 Runtime 公开的
    /// 工具消息面（[`Runtime::record_tool_call`] + [`Runtime::record_tool_result`]）
    /// 落成成对的 `ToolCall`/`ToolResult` 消息，下一步重读会话即能看到。此路**不**
    /// 经 Runtime 的权限门（拒绝/规划结果是本层门禁的产品语义，缺口 B2 未解）。
    fn settle_in_place(
        &mut self,
        call: &ToolCall,
        result: Value,
        is_error: bool,
    ) -> Result<CallOutcome, String> {
        match &self.driver {
            Driver::Local { .. } => Ok(CallOutcome::Handled { result, is_error }),
            Driver::Runtime {
                runtime, session, ..
            } => {
                runtime
                    .record_tool_call(*session, &call.name, &call.args)
                    .map_err(|e| e.to_string())?;
                let message = runtime
                    .record_tool_result(
                        *session,
                        &call.name,
                        call.args.clone(),
                        &result.to_string(),
                    )
                    .map_err(|e| e.to_string())?;
                Ok(CallOutcome::RuntimeSettled(Settled {
                    is_error,
                    // 被拒 / 规划态切换都不算「成功执行写工具」。
                    write_called: false,
                    preview: message.content,
                }))
            }
        }
    }

    /// 工具执行面：本地路径是宿主借进来的执行器，`Runtime` 路径是注入 Runtime 的
    /// 同一个执行器（`Option<&dyn ToolExecutor>` 的异构降级只此一处）。
    pub(super) fn step_executor(&self) -> Option<&dyn ToolExecutor> {
        match &self.driver {
            Driver::Local { executor, .. } => Some(*executor),
            Driver::Runtime { step_executor, .. } => *step_executor,
        }
    }

    /// `Runtime` 路径下把一次工具请求交给 `Runtime` 结算。
    ///
    /// 本函数不自己执行、不写本层历史：调 [`Runtime::settle_tool_call`] —— 由它把
    /// 「模型确实发出了这次调用」落进会话（`push_tool_call`）、按权限门判定真实执行
    /// / 拒绝 / 待审批、并把结果结算成会话消息。返回的 [`Settled`] 从**会话消息**读回
    /// 判定面（[`settled_from_session`]），保证本层没有第二份真相。
    fn settle_call_via_runtime(&self, call: &ToolCall) -> Result<CallOutcome, String> {
        let Driver::Runtime {
            runtime, session, ..
        } = &self.driver
        else {
            return Err("settle_call_via_runtime 仅用于 Runtime 路径".to_string());
        };
        runtime
            .settle_tool_call(*session, &call.name, &call.args)
            .map_err(|e| e.to_string())?;
        Ok(CallOutcome::RuntimeSettled(settled_from_session(
            runtime, *session, call,
        )?))
    }

    /// 拒绝原因：`None` 表示允许执行。
    fn refusal(&self, tool: &str) -> Option<String> {
        if !self.policy.allows(tool) {
            return Some(format!(
                "{tool} 不在 {} 模式白名单（未执行）",
                self.mode.as_str()
            ));
        }
        if self.plan_gate.active && is_write_tool(tool) {
            return Some(format!(
                "规划态拒绝写/执行类工具: {tool}（需 plan.exit 批准）"
            ));
        }
        None
    }

    /// 显式进入规划态（Expert 可用；Plan 模式本已 active）。
    fn enter_plan(&mut self) {
        self.plan_gate.active = true;
        self.plan_gate.approved = false;
        if self.mode != Mode::Plan {
            self.set_mode(Mode::Plan);
        }
    }

    /// 批准后退出规划态并切 Craft 继续。
    fn exit_plan_to_craft(&mut self) {
        self.plan_gate.active = false;
        if self.mode != Mode::Craft {
            self.set_mode(Mode::Craft);
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.policy = ModePolicy::of(mode);
        self.emit(
            EventKind::SessionUpdate.as_str(),
            json!({
                "mode": mode.as_str(),
                "planActive": self.plan_gate.active,
                "planApproved": self.plan_gate.approved,
            }),
        );
    }
}

/// 从会话消息读回一次工具结算的判定面（末条 `ToolResult` 即本次结算）。
///
/// `Runtime` 的 `tool_result` 消息把结果收窄成 `content`（大输出只留 preview、
/// `isError` 不进消息），故错误判定按内容形态取：执行失败是
/// `{"type":"error",…}`、权限拒绝是 `{"status":"denied",…}`。该判定只用于**发事件**
/// 的 `isError` 标注与交付物记账，不影响历史与走向（历史以会话为准）。
fn settled_from_session(runtime: &Runtime, id: Uuid, call: &ToolCall) -> Result<Settled, String> {
    let messages = runtime.get_session(id).map_err(|e| e.to_string())?.messages;
    let last = messages
        .last()
        .ok_or_else(|| format!("工具结算后会话历史为空（session={id}, tool={}）", call.name))?;
    let is_error =
        last.content.contains(r#""type":"error""#) || last.content.contains(r#""status":"denied""#);
    // 写/执行记账：只有成功执行（非错误、非拒绝）的写/执行类工具才算。
    let write_called =
        is_write_tool(&call.name) && !is_error && !last.content.contains(r#""status":"pending""#);
    Ok(Settled {
        is_error,
        write_called,
        preview: last.content.clone(),
    })
}
