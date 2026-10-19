//! 工具步执行结算（W1 接线）：金标 opencode `packages/core/src/session/runner/step.ts`
//! 的步内执行段——门检 → `executeTool` → 截断入历史 → 发布结果。
//!
//! 失败不中断回合：执行错误与权限拒绝都**结算成结果**（`step.ts:129-131` 的
//! `failTool` 语义），让下一步的模型看到失败并自行纠正。

use std::collections::BTreeSet;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::permission::Decision;
use crate::schema::{Event, Session};

use super::Runtime;

/// 工具步的占位结算结果：未注入执行器或需宿主审批时，先把「待宿主执行」写进
/// 历史，使续步重读到的历史里 ToolCall/ToolResult 成对；真实结果由宿主回填。
pub(super) const PENDING_TOOL_RESULT: &str =
    r#"{"status":"pending","note":"工具调用已持久化，执行由宿主承担"}"#;

/// 工具步结算后的回合走向（对齐 `loop` 既有 deny 语义的记账粒度）。
pub(super) enum ToolStepOutcome {
    /// 已结算成结果（真实/失败/deny/pending），回合续跑。
    Settled,
    /// 同一工具二次被拒 ⇒ 回合终止。
    Denied,
}

impl Runtime {
    /// 执行并结算工具步（`step.ts:91-134`：门检 → 执行 → 结算 → 发布；
    /// 执行失败走 failTool 结算成错误结果，回合继续不中断）。
    ///
    /// - 未注入执行器：保持 pending 占位（执行由宿主承担），不查门、不发事件；
    /// - [`Decision::Deny`]：不执行，按既有 deny 语义结算
    ///   `{"status":"denied","reason":..}`（reason 为权限门文案）并发 `isError`
    ///   事件；同一工具二次被拒 ⇒ [`ToolStepOutcome::Denied`] 终止回合；
    /// - [`Decision::Allow`]：执行器真实执行——`Ok` 走既有
    ///   [`Runtime::push_tool_result`]（脱敏内建、大输出进 blob 只留 preview）
    ///   并发 `tool_result` 事件；`Err` 结算 `{"type":"error","message":..}`
    ///   且 `isError=true` 并发事件，回合继续；
    /// - [`Decision::Ask`]：不在步内阻塞等待审批（宿主审批链承担），落 pending
    ///   占位续步，真实结果由宿主审批后经 [`Runtime::record_tool_result`] 回填。
    pub(super) fn execute_tool(
        &self,
        id: Uuid,
        session: &Session,
        name: &str,
        args: &Value,
        denied: &mut BTreeSet<String>,
        events: &mut Vec<Event>,
    ) -> Result<ToolStepOutcome, CoreError> {
        let Some(executor) = self.executor.as_ref() else {
            // 未注入执行器：运行时不执行任何工具，无门检必要，保持兼容占位。
            self.push_tool_result(id, name, args.clone(), PENDING_TOOL_RESULT)?;
            return Ok(ToolStepOutcome::Settled);
        };
        match self.check_tool(id, name, args)? {
            Decision::Deny(reason) => {
                // 同一工具被拒后再次请求 ⇒ 终止（既有语义，防对同一被拒工具空转）。
                if denied.contains(name) {
                    return Ok(ToolStepOutcome::Denied);
                }
                denied.insert(name.to_string());
                let result = json!({ "status": "denied", "reason": reason });
                self.push_tool_result(id, name, args.clone(), &result.to_string())?;
                events.push(self.emit(
                    session,
                    "tool_result",
                    json!({ "name": name, "result": result, "isError": true }),
                )?);
                Ok(ToolStepOutcome::Settled)
            }
            Decision::Allow => {
                match executor.execute(name, args) {
                    Ok(value) => {
                        let (_, payload) =
                            self.push_tool_result(id, name, args.clone(), &value.to_string())?;
                        events.push(self.emit(session, "tool_result", payload)?);
                    }
                    Err(message) => {
                        let error = json!({ "type": "error", "message": message });
                        self.push_tool_result(id, name, args.clone(), &error.to_string())?;
                        events.push(self.emit(
                            session,
                            "tool_result",
                            json!({ "name": name, "result": error, "isError": true }),
                        )?);
                    }
                }
                Ok(ToolStepOutcome::Settled)
            }
            Decision::Ask => {
                self.push_tool_result(id, name, args.clone(), PENDING_TOOL_RESULT)?;
                Ok(ToolStepOutcome::Settled)
            }
        }
    }
}
