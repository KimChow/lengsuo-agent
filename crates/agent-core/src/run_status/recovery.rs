use serde::{Deserialize, Serialize};

use super::status::RunStatus;
use super::types::{ExecutionStage, RunState};

/// 重启恢复决策（wb 26 册 §1.5；三种决策全部可观测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    TerminalPreserved,
    StaleInflightToTerminated,
    FallbackToInitial,
}

impl Recovery {
    /// 决策名（wb 的 onDecision 日志串）。
    pub fn as_str(self) -> &'static str {
        match self {
            Recovery::TerminalPreserved => "terminal-preserved",
            Recovery::StaleInflightToTerminated => "stale-inflight-to-terminated",
            Recovery::FallbackToInitial => "fallback-to-initial",
        }
    }
}

/// 重启恢复决策（wb 26 册 §1.5）。
///
/// 终态 ⇒ `TerminalPreserved`；`planning|working` 且 `now - updated_at > threshold`
/// ⇒ `StaleInflightToTerminated`（**严格大于**，恰好等于阈值不算过期）；
/// 其余 ⇒ `FallbackToInitial`。
pub fn recover(
    status: &RunStatus,
    updated_at: u64,
    now_ms: u64,
    stale_threshold_ms: u64,
) -> Recovery {
    if status.is_terminal() {
        return Recovery::TerminalPreserved;
    }
    let inflight = status.run_state == RunState::Running
        && matches!(
            status.execution_stage,
            Some(ExecutionStage::Planning) | Some(ExecutionStage::Working)
        );
    if inflight && now_ms.saturating_sub(updated_at) > stale_threshold_ms {
        return Recovery::StaleInflightToTerminated;
    }
    Recovery::FallbackToInitial
}
