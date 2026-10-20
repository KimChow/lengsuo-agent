//! 运行状态的落盘与启动恢复（h3-run-status-persist）。
//!
//! 规格：wb 26 册 §1.5（崩溃恢复决策 + 30 分钟 stale 阈值，规则 A26-2）、§3.1
//! （孤儿 pending 终结）、规则 A26-1（终态保护）；卡面口径：**每次状态迁移都落盘**
//! （不只终态）、**非终态 ⇒ 异常中断 ⇒ 迁移 terminated 并记中断原因**、**终态不改**、
//! **恢复不产生多余事件**（除那条「中断」事件）。
//!
//! 与 `session_store.rs` 的分工：那边只管**存**（文件读写、原子替换、授权外沿），
//! 这边只管**什么时候存、恢复时怎么判**。

use uuid::Uuid;

use crate::error::CoreError;

use super::recovery::{recover, Recovery};
use super::session_store::{PersistedRunStatus, SessionStatusStore};
use super::status::RunStatus;
use super::types::{RunEvent, RunState, STALE_THRESHOLD_MS};

/// 异常中断的原因串。
///
/// 取 wb 26 册 §3.3 状态列值域里的 `interrupted`（error 组成员），与
/// `TERMINATED_REASON`（正常终止）区分开：前者是「进程死了留下的残留态」。
pub const INTERRUPTED_REASON: &str = "interrupted";

/// 一次状态迁移的落盘回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionRecord {
    /// 驱动这次迁移的事件。
    pub event: RunEvent,
    /// 是否**真的**发生了迁移。A26-1 终态保护下为 `false`（原样返回 ⇒ 不落盘）。
    pub changed: bool,
    /// 迁移后的运行态。
    pub status: RunStatus,
}

/// 启动恢复的回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// 会话 id。
    pub session_id: Uuid,
    /// wb §1.5 的**决策分类**（可观测）：`terminal-preserved` /
    /// `stale-inflight-to-terminated` / `fallback-to-initial`。
    ///
    /// 注意分类与动作是两回事：本卡在 `fallback-to-initial`（未过 30 分钟阈值）
    /// 这一支**也**执行中断终结（见「偏离清单 D1」），动作看 `interrupted`。
    pub decision: Recovery,
    /// 是否按「异常中断」处理（迁移到 terminated 并记中断原因）。
    pub interrupted: bool,
    /// 中断原因（仅中断时有值）。
    pub interruption_reason: Option<String>,
    /// 恢复过程产生的事件。正常只有那一条「中断」事件；终态恢复为空。
    pub events: Vec<RunEvent>,
    /// 恢复后的运行态。
    pub status: RunStatus,
}

/// 落盘一次状态迁移。
///
/// **每次状态迁移都落盘**（不只终态）——崩在写的中间也要能恢复到最后已知状态。
/// A26-1 终态保护下的「原样返回」不是迁移 ⇒ 不写文件。
pub fn persist_transition(
    store: &SessionStatusStore,
    session_id: Uuid,
    status: &mut RunStatus,
    event: RunEvent,
    now_ms: u64,
) -> Result<TransitionRecord, CoreError> {
    let changed = !status.is_protected(event);
    status.apply(event, now_ms);
    if changed {
        store.save(&PersistedRunStatus {
            session_id,
            run_status: status.clone(),
            updated_at: now_ms,
        })?;
    }
    Ok(TransitionRecord {
        event,
        changed,
        status: status.clone(),
    })
}

/// 启动恢复：读回持久化的运行态，按 wb §1.5 判定并收口。
///
/// - **终态**（`is_terminal()`，或已归档）⇒ `terminal-preserved`：**原样返回**，
///   不改状态、不写文件、不发事件（卡面口径「终态不改」）。
/// - **在途 / 挂起**（`running` / `awaiting_input`）⇒ **异常中断**：迁移到
///   `terminated` 并记录中断原因（`interrupted`），产生**那一条**中断事件。
///   wb §1.5 的 30 分钟阈值在这里决定**时间戳**：过期（stale）⇒ 终结时刻取
///   `updatedAt`（wb 原文「时间戳用 updatedAt」）；未过期 ⇒ 取恢复时刻。
/// - **其余**（idle 初态：从未开始过）⇒ 没有在途工作可中断，保持原样，不发事件。
///
/// 返回 `None` 表示**没有持久化状态**（该会话此前从未落盘）。
pub fn restore_on_startup(
    store: &SessionStatusStore,
    session_id: Uuid,
    now_ms: u64,
) -> Result<Option<RestoreReport>, CoreError> {
    let Some(record) = store.load(session_id)? else {
        return Ok(None);
    };
    let mut status = record.run_status;
    let updated_at = record.updated_at;

    // wb §1.5 的终态组含 `archived`：归档会话不是「在途」，不该被中断终结
    // （终结会把 archivedAt 清掉）。卡片口径用既有 `is_terminal()`，归档另并入。
    if status.is_terminal() || status.archived_at.is_some() {
        return Ok(Some(RestoreReport {
            session_id,
            decision: Recovery::TerminalPreserved,
            interrupted: false,
            interruption_reason: None,
            events: Vec::new(),
            status,
        }));
    }

    // 在途（running）或挂起（awaiting_input）⇒ 异常中断。
    if !matches!(
        status.run_state,
        RunState::Running | RunState::AwaitingInput
    ) {
        return Ok(Some(RestoreReport {
            session_id,
            decision: recover(&status, updated_at, now_ms, STALE_THRESHOLD_MS),
            interrupted: false,
            interruption_reason: None,
            events: Vec::new(),
            status,
        }));
    }

    // wb §1.5 / A26-2：在途态按 30 分钟阈值分类（stale ⇒ 时间戳用 updatedAt）。
    let decision = recover(&status, updated_at, now_ms, STALE_THRESHOLD_MS);
    let at = match decision {
        Recovery::StaleInflightToTerminated => updated_at,
        _ => now_ms,
    };
    status.clamp_to_terminated(at);
    status.termination_reason = Some(INTERRUPTED_REASON.into());
    // 每次状态迁移都落盘：中断迁移同样落盘（否则下次启动又读回 running）。
    store.save(&PersistedRunStatus {
        session_id,
        run_status: status.clone(),
        updated_at: at,
    })?;
    Ok(Some(RestoreReport {
        session_id,
        decision,
        interrupted: true,
        interruption_reason: Some(INTERRUPTED_REASON.into()),
        events: vec![RunEvent::RuntimeTerminated],
        status,
    }))
}
