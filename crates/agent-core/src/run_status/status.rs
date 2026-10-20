use serde::{Deserialize, Serialize};

use super::types::{
    DisplayStatus, ExecutionStage, RunEvent, RunState, TerminalOutcome, TERMINATED_REASON,
};

/// 运行态（wb 26 册 §1.1）。
///
/// 前三项是状态三元组，其余为轮次/挂起/终局原因与三个观测时间戳。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    pub run_state: RunState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_stage: Option<ExecutionStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<TerminalOutcome>,
    #[serde(default)]
    pub turn_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_input_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,
    /// 最近一次转移时间（wb `lastTransitionAt`）。
    pub last_transition_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<u64>,
}

impl RunStatus {
    /// 初始运行态（wb 26 册 §1.5 的 `fallback-to-initial`：回 idle 初态，下轮事件驱动）。
    pub fn initial(now_ms: u64) -> Self {
        Self {
            run_state: RunState::Idle,
            execution_stage: None,
            terminal_outcome: None,
            turn_seq: 0,
            pending_input_kind: None,
            termination_reason: None,
            last_transition_at: now_ms,
            archived_at: None,
            last_activity_at: None,
        }
    }

    /// 逐事件转移（wb 26 册 §1.2，九个事件全量）。
    ///
    /// 每次转移都把 `last_transition_at` 前推到 `now_ms`；未改状态的转移
    /// （A26-1 的终态保护）连时间戳也不动——「原样返回」。
    pub fn apply(&mut self, event: RunEvent, now_ms: u64) {
        // 规则 A26-1：终态会话收到 awaiting_input / input_resolved 原样返回。
        // wb 源注释：「已终态则状态不变只清 pendingInputKind」——终态转移本身会清
        // pendingInputKind（终态不挂起输入），所以终态下这两条事件的净效果就是
        // 「逐字段不变」。
        if self.is_protected(event) {
            return;
        }
        match event {
            RunEvent::TurnStarted => {
                self.run_state = RunState::Running;
                self.execution_stage = Some(ExecutionStage::Planning);
                self.terminal_outcome = None;
                self.turn_seq += 1;
                self.termination_reason = None;
            }
            RunEvent::PlanningStarted => {
                self.run_state = RunState::Running;
                self.execution_stage = Some(ExecutionStage::Planning);
            }
            RunEvent::WorkingStarted => {
                self.run_state = RunState::Running;
                self.execution_stage = Some(ExecutionStage::Working);
            }
            RunEvent::AwaitingInput => {
                self.run_state = RunState::AwaitingInput;
                self.pending_input_kind = Some("user_input".into());
            }
            RunEvent::InputResolved => {
                self.pending_input_kind = None;
                self.run_state = RunState::Running;
                // 「保持或 fallback working」：原阶段是 planning 则保持 planning，
                // 否则回落到 working。
                if self.execution_stage != Some(ExecutionStage::Planning) {
                    self.execution_stage = Some(ExecutionStage::Working);
                }
            }
            RunEvent::TurnCompleted => {
                self.run_state = RunState::Idle;
                self.terminal_outcome = Some(TerminalOutcome::Completed);
            }
            RunEvent::TurnFailed => {
                self.run_state = RunState::Idle;
                self.terminal_outcome = Some(TerminalOutcome::Failed);
            }
            RunEvent::RuntimeErrored => {
                self.run_state = RunState::Errored;
                self.terminal_outcome = Some(TerminalOutcome::Error);
            }
            RunEvent::RuntimeTerminated => {
                self.run_state = RunState::Terminated;
                self.terminal_outcome = Some(TerminalOutcome::Terminated);
                self.termination_reason = Some(TERMINATED_REASON.into());
            }
        }
        // 终态不挂起输入：进入终态时一并清掉（wb 终态对象无 pendingInputKind）。
        if self.is_terminal() {
            self.pending_input_kind = None;
        }
        self.last_transition_at = now_ms;
    }

    /// 规则 A26-1 的终态判定：`terminalOutcome !== null || runState ∈ {errored, terminated}`。
    pub fn is_terminal(&self) -> bool {
        self.terminal_outcome.is_some()
            || matches!(self.run_state, RunState::Errored | RunState::Terminated)
    }

    /// 规则 A26-1 的**保护谓词**：这条事件在当前态下是否被「原样返回」拦下。
    ///
    /// 终态会话收到 `awaiting_input` / `input_resolved` ⇒ 状态逐字段不变
    /// （挂起输入打不醒死会话）。落盘侧据此判定「这次调用**没有**发生迁移」——
    /// 没有迁移就不写文件（`persist.rs` 的 `persist_transition`）。
    pub fn is_protected(&self, event: RunEvent) -> bool {
        self.is_terminal() && matches!(event, RunEvent::AwaitingInput | RunEvent::InputResolved)
    }

    /// 派生显示状态（wb 26 册 §1.3，按优先级序逐条判定）。
    pub fn derive(&self) -> DisplayStatus {
        if self.archived_at.is_some() {
            return DisplayStatus::Archived;
        }
        if self.run_state == RunState::AwaitingInput {
            return DisplayStatus::Pending;
        }
        if self.run_state == RunState::Running {
            return if self.execution_stage == Some(ExecutionStage::Planning) {
                DisplayStatus::Planning
            } else {
                DisplayStatus::Working
            };
        }
        if self.terminal_outcome == Some(TerminalOutcome::Terminated)
            || self.run_state == RunState::Terminated
        {
            return DisplayStatus::Terminated;
        }
        if self.run_state == RunState::Errored
            || self.terminal_outcome == Some(TerminalOutcome::Error)
        {
            return DisplayStatus::Error;
        }
        if self.terminal_outcome == Some(TerminalOutcome::Failed) {
            return DisplayStatus::Failed;
        }
        if self.terminal_outcome == Some(TerminalOutcome::Completed) {
            return DisplayStatus::Completed;
        }
        if self.turn_seq == 0 {
            // 从未开始过。
            return DisplayStatus::Pending;
        }
        DisplayStatus::Completed
    }

    /// 终态钳制（wb 26 册 §1.5）：把在途态终结为 terminated，时间戳取 `updated_at`
    /// （即「末次可见活动时刻」），`last_activity_at` 与 `last_transition_at` 同步前推。
    pub fn clamp_to_terminated(&mut self, updated_at: u64) {
        if self.is_terminal() {
            return;
        }
        self.run_state = RunState::Terminated;
        self.terminal_outcome = Some(TerminalOutcome::Terminated);
        self.termination_reason = Some(TERMINATED_REASON.into());
        self.pending_input_kind = None;
        self.execution_stage = None;
        self.archived_at = None;
        self.last_transition_at = updated_at;
        self.last_activity_at = Some(updated_at);
    }
}
