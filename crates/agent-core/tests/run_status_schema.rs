//! 运行状态机 schema 与九事件转移表（wb 26 册 §1.1–§1.5、A26-1/A26-2）。
//!
//! 本文件从 crate 外部（`agent_core::run_status`）验证契约面：serde 字段名/取值、
//! 九事件转移、终态保护、stale 恢复的三决策边界，以及 `Session.run_status` 新字段
//! 与既有 `SessionStatus` 的并存兼容。

use agent_core::run_status::{
    recover, ExecutionStage, Recovery, RunEvent, RunState, RunStatus, TerminalOutcome,
    STALE_THRESHOLD_MS,
};
use serde_json::json;

/// 走到「running/working」的在途态。
fn inflight() -> RunStatus {
    let mut s = RunStatus::initial(0);
    s.apply(RunEvent::TurnStarted, 1_000);
    s.apply(RunEvent::WorkingStarted, 1_001);
    s
}

#[test]
fn serde_field_names_are_frozen_camel_case() {
    let mut s = RunStatus::initial(1_000);
    s.apply(RunEvent::TurnStarted, 1_100);
    s.last_activity_at = Some(1_200);
    let v = serde_json::to_value(&s).expect("serialize");
    assert_eq!(v["runState"], json!("running"));
    assert_eq!(v["executionStage"], json!("planning"));
    assert_eq!(v["turnSeq"], json!(1));
    assert_eq!(v["lastTransitionAt"], json!(1_100));
    assert_eq!(v["lastActivityAt"], json!(1_200));
    // 未设置的观测字段按既有 schema 风格省略（不写 null）。
    assert!(v.get("terminalOutcome").is_none());
    assert!(v.get("pendingInputKind").is_none());
    assert!(v.get("terminationReason").is_none());
    assert!(v.get("archivedAt").is_none());

    let back: RunStatus = serde_json::from_value(v).expect("deserialize");
    assert_eq!(back, s);
    // 旧日志缺省字段必须能解码（`#[serde(default)]`）。
    let minimal: RunStatus =
        serde_json::from_value(json!({"runState": "idle", "lastTransitionAt": 7}))
            .expect("minimal");
    assert_eq!(minimal, RunStatus::initial(7));
}

#[test]
fn nine_events_are_all_covered() {
    // 逐条断言转移后的三元组（runState / executionStage / terminalOutcome）。
    type Case = (
        &'static str,
        RunEvent,
        RunState,
        Option<ExecutionStage>,
        Option<TerminalOutcome>,
    );
    let cases: [Case; 9] = [
        (
            "turn_started",
            RunEvent::TurnStarted,
            RunState::Running,
            Some(ExecutionStage::Planning),
            None,
        ),
        (
            "planning_started",
            RunEvent::PlanningStarted,
            RunState::Running,
            Some(ExecutionStage::Planning),
            None,
        ),
        (
            "working_started",
            RunEvent::WorkingStarted,
            RunState::Running,
            Some(ExecutionStage::Working),
            None,
        ),
        (
            "awaiting_input",
            RunEvent::AwaitingInput,
            RunState::AwaitingInput,
            Some(ExecutionStage::Working),
            None,
        ),
        (
            "input_resolved",
            RunEvent::InputResolved,
            RunState::Running,
            Some(ExecutionStage::Working),
            None,
        ),
        (
            "turn_completed",
            RunEvent::TurnCompleted,
            RunState::Idle,
            Some(ExecutionStage::Working),
            Some(TerminalOutcome::Completed),
        ),
        (
            "turn_failed",
            RunEvent::TurnFailed,
            RunState::Idle,
            Some(ExecutionStage::Working),
            Some(TerminalOutcome::Failed),
        ),
        (
            "runtime_errored",
            RunEvent::RuntimeErrored,
            RunState::Errored,
            Some(ExecutionStage::Working),
            Some(TerminalOutcome::Error),
        ),
        (
            "runtime_terminated",
            RunEvent::RuntimeTerminated,
            RunState::Terminated,
            Some(ExecutionStage::Working),
            Some(TerminalOutcome::Terminated),
        ),
    ];
    assert_eq!(cases.len(), 9, "九事件全量覆盖");
    for (name, event, state, stage, outcome) in cases {
        assert_eq!(event.as_str(), name, "事件名与 wb §1.2 转移表一致");
        let mut s = inflight();
        if event == RunEvent::InputResolved {
            s.apply(RunEvent::AwaitingInput, 1_002);
        }
        s.apply(event, 2_000);
        assert_eq!(s.run_state, state, "{name}");
        assert_eq!(s.execution_stage, stage, "{name}");
        assert_eq!(s.terminal_outcome, outcome, "{name}");
        assert_eq!(s.last_transition_at, 2_000, "{name} 必须前推转移时间");
    }
}

#[test]
fn turn_started_bumps_turn_seq_and_clears_termination_reason() {
    let mut s = inflight();
    s.apply(RunEvent::RuntimeTerminated, 2_000);
    assert_eq!(s.termination_reason.as_deref(), Some("terminated"));
    assert_eq!(s.turn_seq, 1);

    s.apply(RunEvent::TurnStarted, 3_000);
    assert_eq!(s.turn_seq, 2, "turnSeq 必须递增");
    assert_eq!(s.termination_reason, None, "必须清 terminationReason");
    assert_eq!(s.terminal_outcome, None);
    assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
}

#[test]
fn is_terminal_covers_run_state_by_outcome_matrix() {
    // 5 个 RunState × 有无 terminalOutcome：任一条件成立即为终态（A26-1）。
    for state in [
        RunState::Idle,
        RunState::Running,
        RunState::AwaitingInput,
        RunState::Errored,
        RunState::Terminated,
    ] {
        for outcome in [None, Some(TerminalOutcome::Completed)] {
            let mut s = RunStatus::initial(0);
            s.run_state = state;
            s.terminal_outcome = outcome;
            let by_state = matches!(state, RunState::Errored | RunState::Terminated);
            let by_outcome = outcome.is_some();
            assert_eq!(
                s.is_terminal(),
                by_state || by_outcome,
                "{state:?}/{outcome:?}"
            );
        }
    }
    // 非终态运行态单列。
    let mut s = RunStatus::initial(0);
    s.run_state = RunState::Running;
    s.execution_stage = Some(ExecutionStage::Working);
    assert!(!s.is_terminal());
}

#[test]
fn terminal_session_ignores_pending_input_events() {
    for terminal in [
        RunEvent::TurnCompleted,
        RunEvent::TurnFailed,
        RunEvent::RuntimeErrored,
        RunEvent::RuntimeTerminated,
    ] {
        let mut s = inflight();
        s.apply(terminal, 5_000);
        let frozen = s.clone();
        s.apply(RunEvent::AwaitingInput, 6_000);
        assert_eq!(s, frozen, "终态下 awaiting_input 不改状态");
        s.apply(RunEvent::InputResolved, 7_000);
        assert_eq!(s, frozen, "终态下 input_resolved 不改状态");
        assert!(s.is_terminal());
    }
}

#[test]
fn derive_priority_archived_first_then_state_then_outcome() {
    use agent_core::run_status::DisplayStatus as D;

    // archivedAt 最高优先：即便在途也判 archived。
    let mut s = inflight();
    s.archived_at = Some(9_000);
    assert_eq!(s.derive(), D::Archived);

    let mut s = inflight();
    assert_eq!(s.derive(), D::Working);
    s.apply(RunEvent::AwaitingInput, 2_000);
    assert_eq!(s.derive(), D::Pending);

    let mut s = RunStatus::initial(0);
    assert_eq!(s.derive(), D::Pending, "turnSeq=0 从未开始过");
    s.apply(RunEvent::TurnStarted, 1_000);
    assert_eq!(s.derive(), D::Planning);
    s.apply(RunEvent::TurnCompleted, 2_000);
    assert_eq!(s.derive(), D::Completed);
    s.apply(RunEvent::TurnFailed, 3_000);
    assert_eq!(s.derive(), D::Failed);
    s.apply(RunEvent::RuntimeErrored, 4_000);
    assert_eq!(s.derive(), D::Error);
    s.apply(RunEvent::RuntimeTerminated, 5_000);
    assert_eq!(s.derive(), D::Terminated);
}

#[test]
fn recover_terminal_preserved_regardless_of_age() {
    let mut s = inflight();
    s.apply(RunEvent::TurnCompleted, 1_000);
    assert_eq!(
        recover(&s, 1_000, u64::MAX, STALE_THRESHOLD_MS),
        Recovery::TerminalPreserved
    );
    assert_eq!(Recovery::TerminalPreserved.as_str(), "terminal-preserved");
}

#[test]
fn recover_stale_inflight_to_terminated_just_over_threshold() {
    let s = inflight();
    let updated = 10_000;
    let decision = recover(
        &s,
        updated,
        updated + STALE_THRESHOLD_MS + 1,
        STALE_THRESHOLD_MS,
    );
    assert_eq!(decision, Recovery::StaleInflightToTerminated);
    assert_eq!(decision.as_str(), "stale-inflight-to-terminated");

    // planning 阶段同样算在途。
    let mut s = RunStatus::initial(0);
    s.apply(RunEvent::TurnStarted, 1_000);
    assert_eq!(s.execution_stage, Some(ExecutionStage::Planning));
    assert_eq!(
        recover(
            &s,
            1_000,
            1_000 + STALE_THRESHOLD_MS + 1,
            STALE_THRESHOLD_MS
        ),
        Recovery::StaleInflightToTerminated
    );
}

#[test]
fn recover_exactly_at_threshold_is_not_stale() {
    // 判据是 `>` 而非 `>=`（照抄 wb）：恰好 30 分钟仍回初态。
    let s = inflight();
    let updated = 10_000;
    let decision = recover(
        &s,
        updated,
        updated + STALE_THRESHOLD_MS,
        STALE_THRESHOLD_MS,
    );
    assert_eq!(decision, Recovery::FallbackToInitial);
    assert_eq!(decision.as_str(), "fallback-to-initial");
    assert_eq!(STALE_THRESHOLD_MS, 1_800_000, "wb 原值 30 分钟");
}

#[test]
fn recover_non_inflight_falls_back_to_initial() {
    let s = RunStatus::initial(0);
    assert_eq!(
        recover(
            &s,
            1_000,
            1_000 + STALE_THRESHOLD_MS * 100,
            STALE_THRESHOLD_MS
        ),
        Recovery::FallbackToInitial
    );
    // 未来时间戳（now < updated_at）不得判 stale。
    let s = inflight();
    assert_eq!(
        recover(&s, 10_000, 5_000, STALE_THRESHOLD_MS),
        Recovery::FallbackToInitial
    );
}

#[test]
fn session_run_status_field_is_additive_and_optional() {
    use agent_core::schema::{Isolation, Mode, PermissionMode, Session, SessionStatus};
    use uuid::Uuid;

    let session = Session {
        id: Uuid::nil(),
        title: "t".into(),
        workspace_root: "/tmp".into(),
        mode: Mode::Ask,
        permission_mode: PermissionMode::ReadOnly,
        env_binding: None,
        isolation: Isolation::local_default(),
        messages: Vec::new(),
        created_at_ms: 1,
        updated_at_ms: 2,
        status: SessionStatus::Idle,
        last_seq: 0,
        memory_digest: None,
        run_status: None,
        expert_id: None,
    };
    // 旧日志（无 runStatus）仍能解码为 None；既有 status 字段原样保留。
    let back: Session = serde_json::to_value(&session)
        .and_then(serde_json::from_value)
        .expect("roundtrip");
    assert_eq!(back, session);
    assert_eq!(back.status, SessionStatus::Idle, "SessionStatus 变体不变");
    assert_eq!(
        serde_json::to_value(SessionStatus::Running).unwrap(),
        json!("running")
    );
    assert_eq!(
        serde_json::to_value(SessionStatus::Done).unwrap(),
        json!("done")
    );
    assert_eq!(
        serde_json::to_value(SessionStatus::Failed).unwrap(),
        json!("failed")
    );

    // None 时省略字段；有值时落 camelCase 的 runStatus。
    let with = Session {
        run_status: Some(inflight()),
        ..session
    };
    let v = serde_json::to_value(&with).expect("serialize");
    assert_eq!(v["runStatus"]["runState"], json!("running"));
    assert_eq!(v["runStatus"]["executionStage"], json!("working"));
}
