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
