//! 工具步执行结算（W1 接线）：金标 opencode `packages/core/src/session/runner/step.ts`
//! 的步内执行段——门检 → `executeTool` → 截断入历史 → 发布结果。
//!
//! 失败不中断回合：执行错误与权限拒绝都**结算成结果**（`step.ts:129-131` 的
//! `failTool` 语义），让下一步的模型看到失败并自行纠正。
//!
//! 交付物记账（W1-core）：本文件的执行成功分支（`Decision::Allow` 且执行器
//! 返回 `Ok`）是「成功执行过写/执行工具」的唯一记账点——deny / 执行失败 /
//! pending 都不算；记账结果供 `turns.rs` 的交付物提醒判定消费。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::host::{HostServices, McpToolDispatcher};
use crate::permission::Decision;
use crate::schema::{ApprovalResolution, Event, Session};

use super::turns::StepOutcome;
use super::Runtime;

/// 工具步的占位结算结果：未注入执行器或需宿主审批时，先把「待宿主执行」写进
/// 历史，使续步重读到的历史里 ToolCall/ToolResult 成对；真实结果由宿主回填。
pub(super) const PENDING_TOOL_RESULT: &str =
    r#"{"status":"pending","note":"工具调用已持久化，执行由宿主承担"}"#;

impl Runtime {
    /// 执行并结算工具步（`step.ts:91-134`：门检 → 执行 → 结算 → 发布；
    /// 执行失败走 failTool 结算成错误结果，回合继续不中断）。
    ///
    /// - 未注入执行器：保持 pending 占位（执行由宿主承担），不查门、不发事件；
    /// - [`Decision::Deny`]：不执行，按既有 deny 语义结算
    ///   `{"status":"denied","reason":..}`（reason 为权限门文案）并发 `isError`
    ///   事件；同一工具二次被拒 ⇒ [`StepOutcome::Aborted`] 终止回合；
    /// - [`Decision::Allow`]：执行器真实执行——`Ok` 走既有
    ///   [`Runtime::push_tool_result`]（脱敏内建、大输出进 blob 只留 preview）
    ///   并发 `tool_result` 事件，且**成功执行** `crate::mode::WRITE_TOOLS` 中的
    ///   工具时置位 `write_executed`（交付物提醒的记账点：deny / 执行失败 /
    ///   pending 都不算）；`Err` 结算 `{"type":"error","message":..}`
    ///   且 `isError=true` 并发事件，回合继续；
    ///   **例外（C1）**：名字形如 `mcp:<server>:<tool>` 且宿主注入了
    ///   [`McpToolDispatcher`] 时改走 MCP 路由（见 [`Runtime::settle_mcp_call`]），
    ///   不调用注入的 [`ToolExecutor`]；不匹配前缀／未注入端口时下面这条路径
    ///   逐字节不变；
    /// - [`Decision::Ask`]：不在步内阻塞等待审批（宿主审批链承担），落 pending
    ///   占位续步，真实结果由宿主审批后经 [`Runtime::record_tool_result`] 回填。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn execute_tool(
        &self,
        id: Uuid,
        session: &Session,
        name: &str,
        args: &Value,
        denied: &mut BTreeSet<String>,
        write_executed: &mut bool,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        // 执行器槽位锁定在「取句柄」的瞬间（`Arc::clone` 量级）：执行期间另一次
        // `set_tool_executor` 不影响已开始的这一步（步内自洽，不出现半途换执行器）。
        let executor = self.tool_executor();
        let Some(executor) = executor else {
            // 未注入执行器：运行时不执行任何工具，无门检必要，保持兼容占位。
            self.push_tool_result(id, name, args.clone(), PENDING_TOOL_RESULT)?;
            return Ok(StepOutcome::ToolSettled);
        };
        match self.check_tool(id, name, args)? {
            Decision::Deny(reason) => {
                // 同一工具被拒后再次请求 ⇒ 终止（既有语义，防对同一被拒工具空转）。
                if denied.contains(name) {
                    return Ok(StepOutcome::Aborted {
                        tool: name.to_string(),
                    });
                }
                denied.insert(name.to_string());
                let result = json!({ "status": "denied", "reason": reason });
                self.push_tool_result(id, name, args.clone(), &result.to_string())?;
                events.push(self.emit(
                    session,
                    "tool_result",
                    json!({ "name": name, "result": result, "isError": true }),
                )?);
                Ok(StepOutcome::ToolSettled)
            }
            Decision::Allow => {
                // 不可中断段（金标 `step.ts:141`「Keep the final tool and Step events
                // uninterruptible」）：ToolCall 已写进会话历史，本工具必须在本步内跑
                // 完并结算；此处**不**检查取消标志——取消只在 `run_turn` 的步边界生效。
                // MCP 路由（C1）：`mcp:<server>:<tool>` 形状且宿主注入了 MCP 调用
                // 端口时，不走既有 [`ToolExecutor`]，改由宿主端口投递到
                // [`agent_mcp::TrustedClient`]（wb 24 册 规则 A24-1 的寻址格式）。
                // 不匹配前缀 / 未注入端口 ⇒ 既有路径逐字节不变（零回归）。
                if let Some(dispatch) = self.mcp_route(name) {
                    return self.settle_mcp_call(id, session, name, args, dispatch, events);
                }
                // 任务工具步先入语境（见 `tasks.rs` 的 `TASK_RUNTIME`）：`task.*` 适配层
                // 只被赋予工具名与参数（`ToolExecutor` 签名不含会话），会话语境由这里
                // 提供。语境归属用热态指针断言，嵌套别的 Runtime 会被拒。
                let outcome = self
                    .with_task_runtime(|| executor.execute(name, args))
                    .unwrap_or_else(|e| Err(e.to_string()));
                match outcome {
                    Ok(value) => {
                        // 交付物记账：仅「真实执行成功」置位（deny / 执行失败 /
                        // pending 不算）。
                        if crate::mode::WRITE_TOOLS.contains(&name) {
                            *write_executed = true;
                        }
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
                Ok(StepOutcome::ToolSettled)
            }
            Decision::Ask => {
                // P3 缺项①：先登记「是哪一次调用」再落 pending 占位。
                // `tool_call_id` 同时出现在审批请求与占位结果里，宿主据此在
                // `respond` 之后调用 `resume_approved_tool_call` 闭环回传。
                let approval = self.register_pending_tool_call(id, session, name, args);
                let pending = json!({
                    "status": "pending",
                    "note": "工具调用已持久化，等待用户审批",
                    "toolCallId": approval.tool_call_id,
                    "approvalId": approval.id,
                });
                self.push_tool_result(id, name, args.clone(), &pending.to_string())?;
                Ok(StepOutcome::ToolSettled)
            }
        }
    }

    /// 审批回传闭环（P3 缺项③）：批准 ⇒ 真正执行被挂起的调用并回传结果；
    /// 拒绝/超时/取消 ⇒ 回传 `isError` 结果（文案分别为用户拒绝、超时、取消）。
    ///
    /// 关联键是 `tool_call_id`（缺项①）；挂起快照由 `execute_tool` 的 Ask 分支
    /// 登记，本方法一次性取走。取消传播（缺项②）优先于入参结论：会话已取消、
    /// 已结束，或审批槽已被取消时，即使 `resolution` 为 `Approved` 也不执行。
    ///
    /// 与 `execute_tool` 一样**不在步内阻塞**（参考 §9.3：宿主在
    /// `approval:respond` 后调用本方法）：结果落进会话历史并发 `tool_result`
    /// 事件，下一步重读历史即可拿到与 `tool_call_id` 关联的真实结果。
    pub fn resume_approved_tool_call(
        &self,
        session_id: Uuid,
        tool_call_id: Uuid,
        resolution: ApprovalResolution,
    ) -> Result<Event, CoreError> {
        let pending = self
            .gate
            .take_pending_tool_call(tool_call_id)
            .ok_or(CoreError::ApprovalNotFound(tool_call_id))?;
        if pending.session_id != Some(session_id) {
            return Err(CoreError::ApprovalNotFound(tool_call_id));
        }
        // 取消/超时/未批准三态归一：取消最高优先，超时其次；调用方声称批准
        // 但审批槽从未批准 ⇒ 拒绝执行（fail-closed，防绕过 respond）。
        let effective = if pending.cancelled || self.session_halted(session_id) {
            ApprovalResolution::Cancelled
        } else if pending.timed_out {
            ApprovalResolution::TimedOut
        } else if matches!(resolution, ApprovalResolution::Approved { .. }) && !pending.approved {
            return Err(CoreError::InvalidConfig(
                "工具调用未获批准，拒绝续跑".into(),
            ));
        } else {
            resolution
        };

        let (result, is_error, executed) = match effective {
            ApprovalResolution::Approved { .. } => {
                match self.invoke_pending_tool(&pending.tool_name, &pending.args) {
                    Ok(value) => (value, false, true),
                    Err(message) => (json!({ "type": "error", "message": message }), true, false),
                }
            }
            ApprovalResolution::Denied { .. } => (
                json!({
                    "type": "error",
                    "reason": "user_denied",
                    "message": "用户拒绝执行该工具调用",
                }),
                true,
                false,
            ),
            ApprovalResolution::TimedOut => (
                json!({
                    "type": "error",
                    "reason": "timed_out",
                    "message": "审批等待超时，已按拒绝处理",
                }),
                true,
                false,
            ),
            ApprovalResolution::Cancelled => (
                json!({
                    "type": "error",
                    "reason": "cancelled",
                    "message": "会话已取消，审批作废，工具调用未执行",
                }),
                true,
                false,
            ),
        };

        let (_message, mut payload) = self.push_tool_result(
            session_id,
            &pending.tool_name,
            pending.args.clone(),
            &result.to_string(),
        )?;
        payload["result"] = result;
        payload["isError"] = json!(is_error);
        payload["executed"] = json!(executed);
        payload["toolCallId"] = json!(tool_call_id.to_string());
        payload["approvalId"] = json!(pending.approval_id.to_string());
        let session = self.get_session(session_id)?;
        self.emit(&session, "tool_result", payload)
    }

    /// 续跑一次真实工具调用（与 `Decision::Allow` 分支同源：MCP 路由优先，
    /// 其次注入的执行器；任务工具在调用期间持有语境）。
    fn invoke_pending_tool(&self, name: &str, args: &Value) -> Result<Value, String> {
        if let Some(route) = self.mcp_route(name) {
            return route.dispatch.call_tool(route.server, route.tool, args);
        }
        let Some(executor) = self.tool_executor() else {
            return Err("工具执行器未注入，无法恢复执行".to_string());
        };
        self.with_task_runtime(|| executor.execute(name, args))
            .unwrap_or_else(|error| Err(error.to_string()))
    }

    /// 判定某工具名是否应走 MCP 路由；命中则返回解析出的地址与端口。
    ///
    /// 两条同时成立才路由：① 名称形如 `mcp:<server>:<tool>`（
    /// [`HostServices::parse_mcp_tool_id`]）；② 宿主注入了 [`McpToolDispatcher`]。
    /// 任一不成立 ⇒ `None`，调用方走既有执行器路径（零回归）。
    ///
    /// 端口句柄在此处 `Arc::clone` 锁定：执行期间替换注入面不影响已开始的这一步。
    fn mcp_route<'a>(&'a self, name: &'a str) -> Option<McpRoute<'a>> {
        let (server, tool) = HostServices::parse_mcp_tool_id(name)?;
        let dispatch = self.host_services().mcp_dispatch.as_ref().map(Arc::clone)?;
        Some(McpRoute {
            server,
            tool,
            dispatch,
        })
    }

    /// 结算一次 MCP 工具调用：成功 ⇒ 既有成功结算，失败 ⇒ 既有 `Err(message)`
    /// 结算路径（回合继续、`isError=true`、不 panic）。
    ///
    /// 与既有 `Decision::Allow` 分支的成功/失败结算**共用同一对代码路径**
    /// （[`Runtime::push_tool_result`] + `tool_result` 事件），因此历史形态、事件
    /// payload、脱敏与 blob 语义与普通工具步完全一致，不引入第二套结算。
    fn settle_mcp_call(
        &self,
        id: Uuid,
        session: &Session,
        name: &str,
        args: &Value,
        route: McpRoute<'_>,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        let McpRoute {
            server,
            tool,
            dispatch,
        } = route;
        match dispatch.call_tool(server, tool, args) {
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
        Ok(StepOutcome::ToolSettled)
    }
}

/// 一次命中的 MCP 路由：拆解后的地址与锁定后的端口句柄。
///
/// 收在一个具名结构里，避免把 `((&str, &str), Arc<dyn ..>)` 这类复合类型散在
/// 签名上（`clippy::type_complexity`）。
struct McpRoute<'a> {
    /// `mcp:<server>:<tool>` 里的 `<server>`。
    server: &'a str,
    /// `mcp:<server>:<tool>` 里的 `<tool>`（可含 `:`）。
    tool: &'a str,
    /// 宿主注入的 MCP 调用端口（此处已 `Arc::clone` 锁定）。
    dispatch: Arc<dyn McpToolDispatcher>,
}
