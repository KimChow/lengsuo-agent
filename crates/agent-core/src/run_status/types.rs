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
