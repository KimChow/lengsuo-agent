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
use crate::schema::{ApprovalResolution, Event, Role, Session, ToolInfo};

use super::turns::StepOutcome;
use super::turns_settle::TOOLS_INTERRUPTED;
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
        // 第二槽位（x-cancel-intra）：会话登记了可取消执行器 ⇒ 工具步走可取消
        // 路径；普通槽位仍可注入（给没有可取消实现的工具用）。两槽并存，会话
        // 登记了可取消槽位时它优先。
        let cancellable = self.cancellable_executor(id);
        if executor.is_none() && cancellable.is_none() {
            // 未注入执行器：运行时不执行任何工具，无门检必要，保持兼容占位。
            self.push_tool_result(id, name, args.clone(), PENDING_TOOL_RESULT)?;
            return Ok(StepOutcome::ToolSettled);
        }
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
                // 二段口径（x-cancel-intra）：**执行区间可打断**、**结算段不可
                // 打断**。结算段不因取消标志跳过（金标 `step.ts:141`「Keep the
                // final tool and Step events uninterruptible」）；被取消打断的那次
                // 调用也不丢——它由回合收口段 `settle_cancelled` 结算（见下）。
                // MCP 路由（C1）：`mcp:<server>:<tool>` 形状且宿主注入了 MCP 调用
                // 端口时，不走既有执行器，改由宿主端口投递到
                // [`agent_mcp::TrustedClient`]（wb 24 册 规则 A24-1 的寻址格式）。
                // 不匹配前缀 / 未注入端口 ⇒ 既有路径逐字节不变（零回归）。
                if let Some(dispatch) = self.mcp_route(name) {
                    return self.settle_mcp_call(id, session, name, args, dispatch, events);
                }
                // 第二槽位优先：会话登记了可取消执行器 ⇒ 走可取消路径，取消信号
                // 在**执行区间内**可达（`wait_cancelled` 由 Condvar 唤醒，不轮询）。
                if let Some(exec) = cancellable {
                    // 任务工具步先入语境（见 `tasks.rs` 的 `TASK_RUNTIME`）：可取消
                    // 适配层的签名同样不含会话，会话语境仍由这里提供。
                    let signal = self.cancel_signal(id)?;
                    let outcome = self
                        .with_task_runtime(|| exec.execute_cancellable(name, args, &signal))
                        .unwrap_or_else(|e| Err(e.to_string()));
                    // 执行区间内观察到取消 ⇒ 本次调用**被打断**：此处**不结算**，
                    // 把未配对的 ToolCall 留给回合收口段（`settle_cancelled`，它
                    // 不查取消标志 ⇒ 结算段不可打断），由那里按
                    // `TOOLS_INTERRUPTED` 逐字结算成 `{"type":"aborted"}`。
                    // 口径只有一处，也不会出现「先报 error 再报 aborted」两条结果。
                    //
                    // 工具**已正常返回**（`Ok`）⇒ 照常结算真实结果：结算段不可
                    // 打断，真实产出比合成的中断标记更有价值。
                    if outcome.is_err() && signal.is_cancelled() {
                        return Ok(StepOutcome::ToolSettled);
                    }
                    return self.settle_tool_outcome(
                        id,
                        session,
                        name,
                        args,
                        outcome,
                        write_executed,
                        events,
                    );
                }
                let Some(executor) = executor else {
                    // 会话只登记了可取消槽位（上方已处理）⇒ 这里必然有普通槽位；
                    // 防御性保持占位语义，不 panic。
                    self.push_tool_result(id, name, args.clone(), PENDING_TOOL_RESULT)?;
                    return Ok(StepOutcome::ToolSettled);
                };
                // 任务工具步先入语境（见 `tasks.rs` 的 `TASK_RUNTIME`）：`task.*` 适配层
                // 只被赋予工具名与参数（`ToolExecutor` 签名不含会话），会话语境由这里
                // 提供。语境归属用热态指针断言，嵌套别的 Runtime 会被拒。
                let outcome = self
                    .with_task_runtime(|| executor.execute(name, args))
                    .unwrap_or_else(|e| Err(e.to_string()));
                self.settle_tool_outcome(id, session, name, args, outcome, write_executed, events)
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

    /// 一条执行结果的**结算本体**（普通槽位与可取消槽位共用，保证两种执行器的
    /// 历史形态、事件载荷、脱敏与 blob 语义完全一致）。
    ///
    /// **本段不可打断**：调用方不得在这里检查取消标志（取消只影响执行区间）。
    /// `Ok` ⇒ 真实结果，并置位 [`crate::mode::WRITE_TOOLS`] 的成功记账；
    /// `Err` ⇒ `{"type":"error","message":..}` + `isError=true`，回合继续。
    #[allow(clippy::too_many_arguments)]
    fn settle_tool_outcome(
        &self,
        id: Uuid,
        session: &Session,
        name: &str,
        args: &Value,
        outcome: Result<Value, String>,
        write_executed: &mut bool,
        events: &mut Vec<Event>,
    ) -> Result<StepOutcome, CoreError> {
        match outcome {
            Ok(value) => {
                // 交付物记账：仅「真实执行成功」置位（deny / 执行失败 / pending 不算）。
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

    /// 会话重建后的陈旧调用补齐（宿主在 `build()` 之后调用，公开面）。
    ///
    /// 扫描**全部**历史：上一进程死在工具执行中时，日志里只有 `tool_use`、
    /// 没有 `tool_result`，重建出的会话挂着一条「running」工具。本方法给每个
    /// 未配对的调用补一条 [`TOOLS_INTERRUPTED`] 结果（消息 + `isError` 事件），
    /// 返回补齐条数。幂等：已配对的调用不动，重复调用返回 0。
    ///
    /// 与回合起点自动扫描（[`Runtime::settle_previous_turn_tool_calls`]）的区别
    /// 只在**是否发事件**：重建后没有回合在跑，事件直接落盘、不会被任何 receipt
    /// 漏掉，故顺带给出可观测的 `tool_result`；回合起点的自动扫描**只落消息**，
    /// 见那边的说明。
    pub fn settle_stale_tool_calls(&self, id: Uuid) -> Result<usize, CoreError> {
        self.settle_open_tool_calls(id, None, true)
    }

    /// 回合起跑前的陈旧扫描（`cancel_token` 调用；范围**只含最后一条 user 消息
    /// 之前**的调用，本回合在途的那次在它之后，绝不误伤）。
    ///
    /// 只落消息、**不发事件**：本方法在 `run_turn` 内、`user` 事件之后运行，
    /// 而 receipt 的事件向量由调用方收集；此处直接 `emit` 会让日志里多出一条
    /// receipt 不含的事件，破坏「receipt ≡ 持久日志」不变量。代价是这次补齐
    /// 仅对当前进程可见（重建后由日志投影自然消失，下一回合再补）；需要持久 /
    /// 可观测的补齐走 [`Runtime::settle_stale_tool_calls`]。
    pub(super) fn settle_previous_turn_tool_calls(&self, id: Uuid) -> Result<usize, CoreError> {
        let session = self.get_session(id)?;
        let bound = session
            .messages
            .iter()
            .rposition(|message| message.role == Role::User);
        self.settle_open_tool_calls(id, bound, false)
    }

    /// 陈旧扫描本体：按历史顺序用栈配对 ToolCall / ToolResult，对剩下的未配对
    /// 调用逐条补 [`TOOLS_INTERRUPTED`] 结果。`bound` 为扫描上界（不含）。
    fn settle_open_tool_calls(
        &self,
        id: Uuid,
        bound: Option<usize>,
        emit_events: bool,
    ) -> Result<usize, CoreError> {
        let session = self.get_session(id)?;
        let mut pending: Vec<ToolInfo> = Vec::new();
        for message in session.messages.iter().take(bound.unwrap_or(usize::MAX)) {
            match message.role {
                Role::ToolCall => {
                    if let Some(tool) = &message.tool {
                        pending.push(tool.clone());
                    }
                }
                // 结果与最近一次未结算调用配对（工具步串行，一次一个）。
                Role::ToolResult => {
                    pending.pop();
                }
                Role::User | Role::Assistant => {}
            }
        }
        let error: Value =
            serde_json::from_str(TOOLS_INTERRUPTED).expect("TOOLS_INTERRUPTED 必须是合法 JSON");
        let mut settled = 0;
        for tool in pending {
            let args = tool
                .args_ref
                .as_deref()
                .and_then(|reference| self.blobs.get(reference).ok())
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or(Value::Null);
            let (_message, mut payload) =
                self.push_tool_result(id, &tool.name, args, TOOLS_INTERRUPTED)?;
            if emit_events {
                payload["result"] = error.clone();
                payload["isError"] = json!(true);
                let session = self.get_session(id)?;
                self.emit(&session, "tool_result", payload)?;
            }
            settled += 1;
        }
        Ok(settled)
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
