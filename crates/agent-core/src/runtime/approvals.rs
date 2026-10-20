//! 审批流：请求、响应、等待、取消传播与待审批列表。
//!
//! P3 的三处接线都在本文件：
//! - **tool_call_id 关联**：`register_pending_tool_call` 在 Ask 处登记被审调用，
//!   `pending_approval_requests` 把带 `tool_call_id` 的请求交给宿主；
//! - **取消传播**：`respond_approval`/`wait_approval` 在会话取消（取消标志或
//!   `Done|Failed`）时把审批作废；`cancel_pending_approvals` 是网关取消路径的
//!   显式清理口；
//! - **回传闭环**：批准后的真实执行在 `execute.rs::resume_approved_tool_call`
//!   （本文件只负责把审批登记与取走接口暴露给运行时）。

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

use crate::error::CoreError;
use crate::schema::{ApprovalActor, ApprovalOutcome, ApprovalRequest, Session, SessionStatus};

use super::Runtime;

impl Runtime {
    /// 发起审批（挂起；超时按 deny）。宿主审批链旧入口：不绑具体工具调用。
    pub fn request_approval(
        &self,
        id: Uuid,
        tool: &str,
        args: &Value,
    ) -> Result<ApprovalRequest, CoreError> {
        let session = self.get_session(id)?;
        Ok(self
            .gate
            .register(&session.isolation.key(), Some(id), tool, args, None))
    }

    /// 登记一次待审批的工具调用（P3 缺项①）：生成 `tool_call_id` 并与审批请求绑定。
    ///
    /// 由 `execute_tool` 的 `Decision::Ask` 分支调用；返回的请求里
    /// `tool_call_id` 即 `resume_approved_tool_call` 的关联键。
    pub(super) fn register_pending_tool_call(
        &self,
        id: Uuid,
        session: &Session,
        tool: &str,
        args: &Value,
    ) -> ApprovalRequest {
        let tool_call_id = Uuid::new_v4();
        self.gate.request_approval_for_tool_call(
            &session.isolation.key(),
            Some(id),
            tool,
            args,
            tool_call_id,
        )
    }

    /// 应答审批（`approval:respond`；审批人默认 user）。
    pub fn respond_approval(
        &self,
        approval_id: Uuid,
        approved: bool,
    ) -> Result<ApprovalOutcome, CoreError> {
        self.respond_approval_as(approval_id, approved, ApprovalActor::User)
    }

    /// 应答审批并落审批人（P3 只落字段；审计查询面属 P7）。
    ///
    /// 取消传播优先于应答：会话已取消/已结束时，即便 `approved=true` 也落为
    /// `cancelled` deny，审批请求作废。
    pub fn respond_approval_as(
        &self,
        approval_id: Uuid,
        approved: bool,
        actor: ApprovalActor,
    ) -> Result<ApprovalOutcome, CoreError> {
        let halted = self
            .gate
            .approval_session(approval_id)
            .is_some_and(|session| self.session_halted(session));
        if halted {
            self.gate.cancel(approval_id);
            return Ok(ApprovalOutcome::cancelled(Some(actor)));
        }
        match self.gate.respond_as(approval_id, approved, actor) {
            // 竞态：检查后被取消传播占了位 ⇒ 批准仍须丢弃。
            Err(CoreError::ApprovalAnswered(_))
                if self
                    .gate
                    .approval_session(approval_id)
                    .is_some_and(|session| self.session_halted(session)) =>
            {
                Ok(ApprovalOutcome::cancelled(Some(actor)))
            }
            other => other,
        }
    }

    /// 阻塞等待审批结果（网关在 spawn_blocking 里调用）。
    ///
    /// 绑定会话的审批会带取消监听：会话取消时立即醒来并返回 `cancelled`，
    /// 不再等满调用方超时（P3 缺项②）。
    pub fn wait_approval(
        &self,
        approval_id: Uuid,
        timeout: Duration,
    ) -> Result<ApprovalOutcome, CoreError> {
        let Some(session) = self.gate.approval_session(approval_id) else {
            return self.gate.wait(approval_id, timeout);
        };
        let runtime = self.clone();
        let halted = move || runtime.session_halted(session);
        self.gate.wait_until(approval_id, timeout, &halted)
    }

    /// 某会话隔离键下挂起的审批。
    pub fn pending_approvals(&self, session_id: Uuid) -> Vec<Uuid> {
        let Ok(session) = self.get_session(session_id) else {
            return Vec::new();
        };
        self.gate.list_pending(&session.isolation.key())
    }

    /// 某会话仍未应答的审批请求（带 `tool_call_id`/工具名/参数摘要）。
    pub fn pending_approval_requests(&self, session_id: Uuid) -> Vec<ApprovalRequest> {
        let Ok(session) = self.get_session(session_id) else {
            return Vec::new();
        };
        self.gate.pending_requests(&session.isolation.key())
    }

    /// 取消传播的显式入口：会话取消/关闭时把该会话所有未决审批一并作废。
    ///
    /// 网关的 cancel 路径应调用本方法；即便不调用，`respond_approval` 与
    /// `wait_approval` 也会在边界上检查取消标志并丢弃批准（失效保护）。
    pub fn cancel_pending_approvals(&self, session_id: Uuid) -> Result<Vec<Uuid>, CoreError> {
        let session = self.get_session(session_id)?;
        Ok(self.gate.cancel_session(&session.isolation.key()))
    }

    /// 会话是否已取消/已结束（取消标志置位，或状态为 `Done|Failed`）。
    pub(super) fn session_halted(&self, id: Uuid) -> bool {
        if self
            .cancel_token(id)
            .map(|token| token.load(Ordering::SeqCst))
            .unwrap_or(true)
        {
            return true;
        }
        match self.get_session(id) {
            Ok(session) => matches!(session.status, SessionStatus::Done | SessionStatus::Failed),
            Err(_) => true,
        }
    }
}
