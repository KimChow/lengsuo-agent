//! 审批流机械结构：Slot/Pending 挂起、超时→deny、一次性应答、过期应答降级，
//! 外加 P3 的 `tool_call_id` 关联、取消传播与审批人字段。判定见 mod.rs/resource.rs。

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::error::CoreError;
use crate::schema::{ApprovalActor, ApprovalOutcome, ApprovalRequest, PendingToolCall};

use super::{now_ms, PermissionGate};

/// 取消监听轮询间隔：取消标志不会唤醒条件变量，只能有界切片轮询（仅 `wait_until`）。
const CANCEL_POLL: Duration = Duration::from_millis(10);

/// 单个审批的应答槽（wait/respond 汇合点）。
#[derive(Default)]
struct SlotState {
    /// 是否已应答/已终结（应答、取消、超时均占用一次性位）。
    answered: bool,
    /// 应答/超时/取消结果；`wait` 取走副本后仍保留给续跑面判读。
    outcome: Option<ApprovalOutcome>,
}

struct Slot {
    state: Mutex<SlotState>,
    changed: Condvar,
    deadline: Instant,
}

impl Slot {
    fn new(deadline: Option<Instant>) -> Self {
        Self {
            state: Mutex::new(SlotState::default()),
            changed: Condvar::new(),
            deadline: deadline.unwrap_or_else(Instant::now),
        }
    }

    fn answered(&self) -> bool {
        self.state.lock().map(|s| s.answered).unwrap_or(true)
    }

    fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn cancel(&self) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        if state.answered {
            return false;
        }
        state.answered = true;
        state.outcome = Some(ApprovalOutcome::cancelled(None));
        drop(state);
        self.changed.notify_all();
        true
    }
}

/// 挂起审批的索引项。
pub(crate) struct Pending {
    pub(crate) session_id: String,
    /// 会话 UUID（取消传播检查键；直连 `PermissionGate` 的旧路径为 `None`）。
    session_uuid: Option<Uuid>,
    /// 对外可见的请求（`list_pending`/`pending_requests` 的单一来源）。
    request: ApprovalRequest,
    /// 被审工具调用快照：批准后据此真正执行并回传。
    tool: Option<PendingTool>,
    slot: Arc<Slot>,
}

struct PendingTool {
    tool_call_id: Uuid,
    tool_name: String,
    args: Value,
}

impl PermissionGate {
    pub fn request_approval(&self, session_id: &str, tool: &str, args: &Value) -> ApprovalRequest {
        self.register(session_id, None, tool, args, None)
    }

    /// 发起**带 tool_call_id 关联**的审批（P3 缺项①）。
    pub fn request_approval_for_tool_call(
        &self,
        session_id: &str,
        session_uuid: Option<Uuid>,
        tool: &str,
        args: &Value,
        tool_call_id: Uuid,
    ) -> ApprovalRequest {
        self.register(session_id, session_uuid, tool, args, Some(tool_call_id))
    }

    /// 登记入口：会话键 + 会话 UUID + 可选工具调用关联。
    pub(crate) fn register(
        &self,
        session_id: &str,
        session_uuid: Option<Uuid>,
        tool: &str,
        args: &Value,
        tool_call_id: Option<Uuid>,
    ) -> ApprovalRequest {
        let id = Uuid::new_v4();
        let summary = self.redactor.summarize_args(args);
        let reason = format!("{tool} 需要用户确认");
        let expires_at_ms = now_ms().saturating_add(self.approval_timeout.as_millis() as u64);
        // 单调截止点：超大超时无法表示时 fail-closed（立即视为过期）。
        let deadline = Instant::now().checked_add(self.approval_timeout);
        let request = ApprovalRequest {
            id,
            session_id: session_id.to_string(),
            tool_call_id,
            tool_name: tool.to_string(),
            summary,
            reason,
            expires_at_ms,
        };
        let entry = Pending {
            session_id: session_id.to_string(),
            session_uuid,
            request: request.clone(),
            tool: tool_call_id.map(|tool_call_id| PendingTool {
                tool_call_id,
                tool_name: tool.to_string(),
                args: args.clone(),
            }),
            slot: Arc::new(Slot::new(deadline)),
        };
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id, entry);
        }
        request
    }

    /// 某会话仍未应答的审批 id（升序）。
    pub fn list_pending(&self, session_id: &str) -> Vec<Uuid> {
        let Ok(pending) = self.pending.lock() else {
            return Vec::new();
        };
        let mut ids: Vec<Uuid> = pending
            .iter()
            .filter(|(_, entry)| entry.session_id == session_id && !entry.slot.answered())
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// 某会话仍未应答的审批请求（按 id 稳定排序；含 `tool_call_id`）。
    pub fn pending_requests(&self, session_id: &str) -> Vec<ApprovalRequest> {
        let Ok(pending) = self.pending.lock() else {
            return Vec::new();
        };
        let mut requests: Vec<ApprovalRequest> = pending
            .values()
            .filter(|entry| entry.session_id == session_id && !entry.slot.answered())
            .map(|entry| entry.request.clone())
            .collect();
        requests.sort_by_key(|request| request.id);
        requests
    }

    /// 审批关联的会话 UUID（取消传播检查用；无 UUID 的旧路径返回 `None`）。
    pub fn approval_session(&self, id: Uuid) -> Option<Uuid> {
        self.pending
            .lock()
            .ok()
            .and_then(|pending| pending.get(&id).and_then(|entry| entry.session_uuid))
    }

    /// 应答审批（一次性；审批人默认 `User`）。
    pub fn respond(&self, id: Uuid, approved: bool) -> Result<ApprovalOutcome, CoreError> {
        self.respond_as(id, approved, ApprovalActor::User)
    }

    /// 应答审批并落审批人（P3 只落字段，查询/导出面留给 P7）。
    pub fn respond_as(
        &self,
        id: Uuid,
        approved: bool,
        actor: ApprovalActor,
    ) -> Result<ApprovalOutcome, CoreError> {
        let slot = {
            let pending = self
                .pending
                .lock()
                .map_err(|_| CoreError::InvalidConfig("审批锁毒化".into()))?;
            let Some(entry) = pending.get(&id) else {
                return Err(CoreError::ApprovalNotFound(id));
            };
            Arc::clone(&entry.slot)
        };
        let requested = ApprovalOutcome::answered(approved, Some(actor));
        let outcome = {
            let mut state = slot
                .state
                .lock()
                .map_err(|_| CoreError::InvalidConfig("审批槽锁毒化".into()))?;
            if state.answered {
                return Err(CoreError::ApprovalAnswered(id));
            }
            state.answered = true;
            // 晚于截止点的应答不得批准：落为过期 deny，仍占掉一次性应答位。
            let final_outcome = if slot.expired() {
                ApprovalOutcome::timed_out()
            } else {
                requested
            };
            state.outcome = Some(final_outcome);
            final_outcome
        };
        slot.changed.notify_all();
        Ok(outcome)
    }

    /// 等待审批结果，带超时；超时按 deny 语义返回并清理旧路径挂起项。
    pub fn wait(&self, id: Uuid, timeout: Duration) -> Result<ApprovalOutcome, CoreError> {
        self.wait_inner(id, timeout, None)
    }

    /// 带取消监听的等待（P3 缺项②）：监听器为真时把槽落为 cancelled 并立即返回。
    ///
    /// 监听器由 `Runtime` 提供（会话取消标志 / Done|Failed）；普通 `PermissionGate`
    /// 调用方用 [`Self::wait`] 即可（无取消源，行为与既有语义逐字一致）。
    pub fn wait_until(
        &self,
        id: Uuid,
        timeout: Duration,
        halted: &dyn Fn() -> bool,
    ) -> Result<ApprovalOutcome, CoreError> {
        self.wait_inner(id, timeout, Some(halted))
    }

    fn wait_inner(
        &self,
        id: Uuid,
        timeout: Duration,
        halted: Option<&dyn Fn() -> bool>,
    ) -> Result<ApprovalOutcome, CoreError> {
        let (slot, linked) = {
            let pending = self
                .pending
                .lock()
                .map_err(|_| CoreError::InvalidConfig("审批锁毒化".into()))?;
            let Some(entry) = pending.get(&id) else {
                return Err(CoreError::ApprovalNotFound(id));
            };
            (Arc::clone(&entry.slot), entry.tool.is_some())
        };
        // 等待上限取「调用方 timeout」与「创建时截止点剩余」的较短值；
        // 加法溢出时 fail-closed（按已到期处理，立即 deny）。
        let budget = timeout.min(slot.remaining());
        let deadline = Instant::now()
            .checked_add(budget)
            .unwrap_or_else(Instant::now);
        let mut guard = slot
            .state
            .lock()
            .map_err(|_| CoreError::InvalidConfig("审批槽锁毒化".into()))?;
        loop {
            if let Some(outcome) = guard.outcome {
                drop(guard);
                // 带 tool_call_id 的项保留到 `resume_approved_tool_call` 取走，
                // 否则续跑面就查不到「这次调用该怎么处理」。
                if !linked {
                    self.forget(id);
                }
                return Ok(outcome);
            }
            if halted.is_some_and(|check| check()) {
                drop(guard);
                slot.cancel();
                guard = slot
                    .state
                    .lock()
                    .map_err(|_| CoreError::InvalidConfig("审批槽锁毒化".into()))?;
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 超时也是终态：占掉一次性位，晚到的批准不得生效。
                let outcome = ApprovalOutcome::timed_out();
                if !guard.answered {
                    guard.answered = true;
                    guard.outcome = Some(outcome);
                }
                let outcome = guard.outcome.unwrap_or(outcome);
                drop(guard);
                if !linked {
                    self.forget(id);
                }
                return Ok(outcome);
            }
            // 有取消监听时按固定小片轮询；无监听时保持原「一觉到超时」语义。
            let slice = if halted.is_some() {
                remaining.min(CANCEL_POLL)
            } else {
                remaining
            };
            let (next, _wait_result) = slot
                .changed
                .wait_timeout(guard, slice)
                .map_err(|_| CoreError::InvalidConfig("审批条件变量毒化".into()))?;
            guard = next;
        }
    }

    /// 取消单个审批（返回是否真的作废）。
    pub fn cancel(&self, id: Uuid) -> bool {
        let slot = self
            .pending
            .lock()
            .ok()
            .and_then(|pending| pending.get(&id).map(|entry| Arc::clone(&entry.slot)));
        slot.is_some_and(|slot| slot.cancel())
    }

    /// 取消传播：会话取消时把该会话所有未决审批一并作废（返回被作废的 id）。
    pub fn cancel_session(&self, session_id: &str) -> Vec<Uuid> {
        let slots: Vec<(Uuid, Arc<Slot>)> = self
            .pending
            .lock()
            .map(|pending| {
                pending
                    .iter()
                    .filter(|(_, entry)| entry.session_id == session_id && !entry.slot.answered())
                    .map(|(id, entry)| (*id, Arc::clone(&entry.slot)))
                    .collect()
            })
            .unwrap_or_default();
        let mut cancelled = Vec::new();
        for (id, slot) in slots {
            if slot.cancel() {
                cancelled.push(id);
            }
        }
        cancelled.sort_unstable();
        cancelled
    }

    /// 取走 `tool_call_id` 对应的待审批工具快照（续跑的一次性入口）。
    pub fn take_pending_tool_call(&self, tool_call_id: Uuid) -> Option<PendingToolCall> {
        let mut pending = self.pending.lock().ok()?;
        let approval_id = pending
            .iter()
            .find(|(_, entry)| {
                entry
                    .tool
                    .as_ref()
                    .is_some_and(|tool| tool.tool_call_id == tool_call_id)
            })
            .map(|(id, _)| *id)?;
        let entry = pending.remove(&approval_id)?;
        let tool = entry.tool?;
        let (cancelled, approved, timed_out) = entry
            .slot
            .state
            .lock()
            .map(|state| {
                let outcome = state.outcome;
                (
                    outcome.is_some_and(|o| o.cancelled),
                    outcome.is_some_and(|o| o.approved),
                    outcome.is_some_and(|o| o.timed_out),
                )
            })
            .unwrap_or((false, false, false));
        Some(PendingToolCall {
            approval_id,
            session_id: entry.session_uuid,
            tool_call_id: tool.tool_call_id,
            tool_name: tool.tool_name,
            args: tool.args,
            cancelled,
            approved,
            timed_out,
        })
    }

    /// 挂起审批总数（观测用）。
    pub fn pending_count(&self) -> usize {
        self.pending.lock().map(|p| p.len()).unwrap_or_default()
    }

    fn forget(&self, id: Uuid) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }
}
