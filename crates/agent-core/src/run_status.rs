//! 会话运行状态机（wb 26 册 §1）。
//!
//! 本模块只有纯域逻辑：状态三元组 + 九事件转移表 + 两条规则（A26-1 终态保护、
//! A26-2 stale 恢复判定）。不做持久化、不做回放、不发射事件——那是 P0-5。
//!
//! 与既有 [`crate::schema::SessionStatus`] 的关系：`SessionStatus` 是已冻结的
//! 4 值会话状态（`session_update` 事件与网关信封依赖它），本模块的 [`RunStatus`]
//! 是 wb 26 册的 5 状态运行态，挂到 `Session.run_status` 新字段上，两者并存不互改。

use serde::{Deserialize, Serialize};

/// 运行态（wb 26 册 §1.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Idle,
    Running,
    AwaitingInput,
    Errored,
    Terminated,
}

/// 执行阶段（planning / working 两态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStage {
    Planning,
    Working,
}

/// 终态结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Completed,
    Failed,
    Error,
    Terminated,
}

/// 派生显示状态（wb 26 册 §1.3 的优先级序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStatus {
    Archived,
    Pending,
    Planning,
    Working,
    Terminated,
    Error,
    Failed,
    Completed,
}

/// 驱动状态机的事件（逐条照抄 wb 26 册 §1.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEvent {
    /// → running/planning，`terminalOutcome=null`，`turnSeq+1`，清 `terminationReason`。
    TurnStarted,
    /// → running/planning。
    PlanningStarted,
    /// → running/working。
    WorkingStarted,
    /// → awaiting_input，记 `pendingInputKind`。
    AwaitingInput,
    /// 非终态 → running（保持或 fallback working）；已终态则原样返回。
    InputResolved,
    /// → idle / `terminalOutcome=completed`。
    TurnCompleted,
    /// → idle / `terminalOutcome=failed`。
    TurnFailed,
    /// → errored / `terminalOutcome=error`。
    RuntimeErrored,
    /// → terminated / `terminalOutcome=terminated` + `terminationReason`。
    RuntimeTerminated,
}

impl RunEvent {
    /// 事件名（snake_case，与 wb 26 册 §1.2 的转移表逐字对应）。
    pub fn as_str(self) -> &'static str {
        match self {
            RunEvent::TurnStarted => "turn_started",
            RunEvent::PlanningStarted => "planning_started",
            RunEvent::WorkingStarted => "working_started",
            RunEvent::AwaitingInput => "awaiting_input",
            RunEvent::InputResolved => "input_resolved",
            RunEvent::TurnCompleted => "turn_completed",
            RunEvent::TurnFailed => "turn_failed",
            RunEvent::RuntimeErrored => "runtime_errored",
            RunEvent::RuntimeTerminated => "runtime_terminated",
        }
    }
}

/// 终态钳制的默认原因（wb 26 册 §1.5 `o()` 构建 terminated 态的默认值）。
pub const TERMINATED_REASON: &str = "terminated";

/// 重启恢复的 stale 判定阈值：30 分钟（wb 26 册 §1.5 的 `1_800_000` 原值）。
pub const STALE_THRESHOLD_MS: u64 = 1_800_000;

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
        if self.is_terminal() && matches!(event, RunEvent::AwaitingInput | RunEvent::InputResolved)
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn running(stage: ExecutionStage) -> RunStatus {
        let mut s = RunStatus::initial(0);
        s.apply(RunEvent::TurnStarted, 1_000);
        s.apply(
            match stage {
                ExecutionStage::Planning => RunEvent::PlanningStarted,
                ExecutionStage::Working => RunEvent::WorkingStarted,
            },
            1_001,
        );
        s
    }

    #[test]
    fn turn_started_increments_seq_and_clears_reason() {
        let mut s = RunStatus::initial(0);
        s.termination_reason = Some("boom".into());
        s.apply(RunEvent::TurnStarted, 100);
        assert_eq!(s.run_state, RunState::Running);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
        assert_eq!(s.terminal_outcome, None);
        assert_eq!(s.turn_seq, 1);
        assert_eq!(s.termination_reason, None);
        s.apply(RunEvent::TurnStarted, 200);
        assert_eq!(s.turn_seq, 2);
        assert_eq!(s.last_transition_at, 200);
    }

    #[test]
    fn nine_events_transitions() {
        // planning_started
        let mut s = RunStatus::initial(0);
        s.apply(RunEvent::PlanningStarted, 10);
        assert_eq!(s.run_state, RunState::Running);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
        assert_eq!(s.terminal_outcome, None);

        // working_started
        let mut s = RunStatus::initial(0);
        s.apply(RunEvent::WorkingStarted, 10);
        assert_eq!(s.run_state, RunState::Running);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Working));
        assert_eq!(s.terminal_outcome, None);

        // awaiting_input
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::AwaitingInput, 20);
        assert_eq!(s.run_state, RunState::AwaitingInput);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Working));
        assert_eq!(s.terminal_outcome, None);
        assert_eq!(s.pending_input_kind.as_deref(), Some("user_input"));

        // input_resolved
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::AwaitingInput, 20);
        s.apply(RunEvent::InputResolved, 30);
        assert_eq!(s.run_state, RunState::Running);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Working));
        assert_eq!(s.terminal_outcome, None);
        assert_eq!(s.pending_input_kind, None);

        // turn_completed
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::TurnCompleted, 40);
        assert_eq!(s.run_state, RunState::Idle);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Working));
        assert_eq!(s.terminal_outcome, Some(TerminalOutcome::Completed));

        // turn_failed
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::TurnFailed, 50);
        assert_eq!(s.run_state, RunState::Idle);
        assert_eq!(s.terminal_outcome, Some(TerminalOutcome::Failed));

        // runtime_errored
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::RuntimeErrored, 60);
        assert_eq!(s.run_state, RunState::Errored);
        assert_eq!(s.terminal_outcome, Some(TerminalOutcome::Error));

        // runtime_terminated
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::RuntimeTerminated, 70);
        assert_eq!(s.run_state, RunState::Terminated);
        assert_eq!(s.terminal_outcome, Some(TerminalOutcome::Terminated));
        assert_eq!(s.termination_reason.as_deref(), Some(TERMINATED_REASON));
    }

    #[test]
    fn input_resolved_from_planning_keeps_planning() {
        let mut s = running(ExecutionStage::Planning);
        s.apply(RunEvent::AwaitingInput, 20);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
        s.apply(RunEvent::InputResolved, 30);
        assert_eq!(s.run_state, RunState::Running);
        assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
    }

    #[test]
    fn terminal_protection_keeps_state_verbatim() {
        for terminal in [
            RunEvent::TurnCompleted,
            RunEvent::TurnFailed,
            RunEvent::RuntimeErrored,
            RunEvent::RuntimeTerminated,
        ] {
            let mut s = running(ExecutionStage::Working);
            s.apply(terminal, 100);
            let before = s.clone();
            s.apply(RunEvent::AwaitingInput, 200);
            assert_eq!(s, before, "终态下 awaiting_input 必须逐字段不变");
            s.apply(RunEvent::InputResolved, 300);
            assert_eq!(s, before, "终态下 input_resolved 必须逐字段不变");
        }
    }

    #[test]
    fn is_terminal_boundary_over_states_and_outcomes() {
        let cases = [
            (RunState::Idle, None, false),
            (RunState::Idle, Some(TerminalOutcome::Completed), true),
            (RunState::Running, None, false),
            (RunState::Running, Some(TerminalOutcome::Failed), true),
            (RunState::AwaitingInput, None, false),
            (RunState::AwaitingInput, Some(TerminalOutcome::Error), true),
            (RunState::Errored, None, true),
            (RunState::Errored, Some(TerminalOutcome::Error), true),
            (RunState::Terminated, None, true),
            (
                RunState::Terminated,
                Some(TerminalOutcome::Terminated),
                true,
            ),
        ];
        for (state, outcome, expected) in cases {
            let mut s = RunStatus::initial(0);
            s.run_state = state;
            s.terminal_outcome = outcome;
            assert_eq!(s.is_terminal(), expected, "{state:?}/{outcome:?}");
        }
    }

    #[test]
    fn derive_follows_priority_order() {
        let mut s = RunStatus::initial(0);
        assert_eq!(s.derive(), DisplayStatus::Pending, "turnSeq=0 从未开始");

        s.apply(RunEvent::TurnStarted, 10);
        assert_eq!(s.derive(), DisplayStatus::Planning);
        s.apply(RunEvent::WorkingStarted, 20);
        assert_eq!(s.derive(), DisplayStatus::Working);
        s.apply(RunEvent::AwaitingInput, 30);
        assert_eq!(s.derive(), DisplayStatus::Pending);
        s.apply(RunEvent::InputResolved, 40);
        assert_eq!(s.derive(), DisplayStatus::Working);
        s.apply(RunEvent::TurnCompleted, 50);
        assert_eq!(s.derive(), DisplayStatus::Completed);
        s.apply(RunEvent::TurnFailed, 60);
        assert_eq!(s.derive(), DisplayStatus::Failed);
        s.apply(RunEvent::RuntimeErrored, 70);
        assert_eq!(s.derive(), DisplayStatus::Error);
        s.apply(RunEvent::RuntimeTerminated, 80);
        assert_eq!(s.derive(), DisplayStatus::Terminated);

        // archived 优先级最高：盖过一切运行态。
        let mut s = running(ExecutionStage::Working);
        s.archived_at = Some(90);
        assert_eq!(s.derive(), DisplayStatus::Archived);
    }

    #[test]
    fn recover_three_decisions_with_exact_threshold_boundary() {
        // 终态 ⇒ terminal_preserved（即便早已过期）。
        let mut s = running(ExecutionStage::Working);
        s.apply(RunEvent::TurnCompleted, 100);
        assert_eq!(
            recover(&s, 100, u64::MAX, STALE_THRESHOLD_MS),
            Recovery::TerminalPreserved
        );

        // 在途且超阈值 ⇒ stale_inflight_to_terminated。
        let s = running(ExecutionStage::Working);
        assert_eq!(
            recover(
                &s,
                1_000,
                1_000 + STALE_THRESHOLD_MS + 1,
                STALE_THRESHOLD_MS
            ),
            Recovery::StaleInflightToTerminated
        );

        // 恰好等于阈值不算过期（> 而非 >=）⇒ fallback_to_initial。
        assert_eq!(
            recover(&s, 1_000, 1_000 + STALE_THRESHOLD_MS, STALE_THRESHOLD_MS),
            Recovery::FallbackToInitial
        );

        // 非在途（idle）⇒ fallback_to_initial。
        let s = RunStatus::initial(0);
        assert_eq!(
            recover(
                &s,
                1_000,
                1_000 + STALE_THRESHOLD_MS * 10,
                STALE_THRESHOLD_MS
            ),
            Recovery::FallbackToInitial
        );
        assert_eq!(STALE_THRESHOLD_MS, 1_800_000);
    }

    #[test]
    fn clamp_to_terminated_is_idempotent_and_preserves_terminal() {
        let mut s = running(ExecutionStage::Working);
        s.clamp_to_terminated(5_000);
        assert_eq!(s.run_state, RunState::Terminated);
        assert_eq!(s.terminal_outcome, Some(TerminalOutcome::Terminated));
        assert_eq!(s.last_transition_at, 5_000);
        assert_eq!(s.last_activity_at, Some(5_000));

        let before = s.clone();
        s.clamp_to_terminated(9_000);
        assert_eq!(s, before, "已终态不被二次钳制");
    }
}
