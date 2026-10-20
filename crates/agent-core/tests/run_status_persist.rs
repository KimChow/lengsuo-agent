//! 运行状态机的落盘与启动恢复（h3-run-status-persist）。
//!
//! 参考：wb 26 册《会话持久化与运行状态机》§1.5（崩溃恢复决策）、规则 A26-1
//! （终态保护）、A26-2（30 分钟 stale 判定）；卡面口径：**每次状态迁移都落盘**
//! （不只终态）、**非终态 ⇒ 异常中断 ⇒ 迁移 terminated 并记中断原因**、
//! **终态不改**、恢复**不产生多余事件**（除那条「中断」事件）。

use agent_core::run_status::{
    persist_transition, restore_on_startup, DisplayStatus, PersistedRunStatus, Recovery,
    RestoreReport, RunEvent, RunState, RunStatus, SessionStatusStore, TerminalOutcome,
    INTERRUPTED_REASON, STALE_THRESHOLD_MS,
};
use serde_json::json;
use uuid::Uuid;

/// wb 26 册 §1.2 的九事件全量。
const NINE_EVENTS: [RunEvent; 9] = [
    RunEvent::TurnStarted,
    RunEvent::PlanningStarted,
    RunEvent::WorkingStarted,
    RunEvent::AwaitingInput,
    RunEvent::InputResolved,
    RunEvent::TurnCompleted,
    RunEvent::TurnFailed,
    RunEvent::RuntimeErrored,
    RunEvent::RuntimeTerminated,
];

/// 落盘目录（`SessionStatusStore` 的根）。
fn store_at(dir: &tempfile::TempDir) -> SessionStatusStore {
    SessionStatusStore::new(dir.path().join("run-status"))
}

/// 读到 running/working 的在途态（崩在这类态上的恢复最关键）。
fn inflight() -> RunStatus {
    let mut s = RunStatus::initial(0);
    s.apply(RunEvent::TurnStarted, 1_000);
    s.apply(RunEvent::WorkingStarted, 1_100);
    s
}

/// 落盘一份「在途」状态，返回实际的 `updated_at`（末次落盘时刻 = 事件时刻）。
fn persist_inflight(dir: &tempfile::TempDir, id: Uuid, at: u64) -> u64 {
    let store = store_at(dir);
    let mut status = RunStatus::initial(0);
    persist_transition(&store, id, &mut status, RunEvent::TurnStarted, at).expect("落盘");
    persist_transition(&store, id, &mut status, RunEvent::WorkingStarted, at + 1).expect("落盘");
    at + 1
}

/// 恢复入口的样板：有持久化状态时的回执。
fn restore(store: &SessionStatusStore, id: Uuid, now: u64) -> RestoreReport {
    restore_on_startup(store, id, now)
        .expect("恢复")
        .expect("有持久化状态")
}

/// 直接写一份快照（构造特定形态的持久化状态）。
fn save_raw(store: &SessionStatusStore, id: Uuid, status: RunStatus, updated_at: u64) {
    let rec = PersistedRunStatus {
        session_id: id,
        run_status: status,
        updated_at,
    };
    store.save(&rec).expect("落盘");
}

#[test]
fn every_transition_updates_the_persisted_file_and_survives_a_restart() {
    for (i, event) in NINE_EVENTS.iter().enumerate() {
        let dir = tempfile::tempdir().expect("临时目录");
        let id = Uuid::new_v4();
        let mut status = RunStatus::initial(0);
        let now = 5_000 + i as u64 * 10;

        // 「旧进程」：落盘。
        {
            let store = store_at(&dir);
            let record = persist_transition(&store, id, &mut status, *event, now).expect("落盘");
            assert!(record.changed, "{event:?} 必须是一次真实迁移");
            assert_eq!(record.event, *event);
            assert_eq!(record.status, status);

            // 落盘文件内容随之更新。
            let raw = std::fs::read_to_string(store.path_of(id)).expect("读落盘文件");
            let value: serde_json::Value = serde_json::from_str(&raw).expect("落盘文件是合法 JSON");
            assert_eq!(value["sessionId"], json!(id.to_string()));
            assert_eq!(value["updatedAt"], json!(now));
            assert_eq!(
                value["runStatus"]["runState"],
                json!(status.run_state),
                "{event:?}"
            );
            assert_eq!(
                value["runStatus"]["lastTransitionAt"],
                json!(now),
                "{event:?}"
            );
        }
        // 「新进程」：另一个 store 实例读同一目录 ⇒ 逐字段一致。
        let store = store_at(&dir);
        let stored = store.load(id).expect("读回").expect("有持久化状态");
        assert_eq!(stored.run_status, status, "{event:?} 恢复后必须逐字段一致");
        assert_eq!(stored.updated_at, now);
        assert_eq!(stored.session_id, id);
    }
}

#[test]
fn restart_after_running_migrates_to_terminated_with_reason() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    // 「旧进程」：写完 running 就死。
    persist_inflight(&dir, id, 1_000);

    // 「新进程」：读到 running ⇒ 自动迁移为 terminated + 记中断原因。
    let store = store_at(&dir);
    let report = restore(&store, id, 2_000);
    assert!(report.interrupted, "非终态 ⇒ 异常中断");
    assert_eq!(
        report.interruption_reason.as_deref(),
        Some(INTERRUPTED_REASON)
    );
    assert_eq!(report.status.run_state, RunState::Terminated);
    assert_eq!(
        report.status.terminal_outcome,
        Some(TerminalOutcome::Terminated)
    );
    assert_eq!(
        report.status.termination_reason.as_deref(),
        Some(INTERRUPTED_REASON),
        "必须记录中断原因"
    );
    assert!(report.status.is_terminal());
    assert_eq!(report.status.execution_stage, None);

    // 终态已落盘：再恢复一次不再产生事件（幂等，不重复报中断）。
    let again = restore(&store, id, 3_000);
    assert!(!again.interrupted);
    assert!(again.events.is_empty());
    assert_eq!(again.decision, Recovery::TerminalPreserved);
    assert_eq!(again.status.run_state, RunState::Terminated);
}

#[test]
fn awaiting_input_session_is_also_migrated_to_terminated() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    {
        let store = store_at(&dir);
        let mut status = inflight();
        persist_transition(&store, id, &mut status, RunEvent::AwaitingInput, 1_200).expect("落盘");
        assert_eq!(status.run_state, RunState::AwaitingInput);
    }
    let store = store_at(&dir);
    let report = restore(&store, id, 1_300);
    assert!(report.interrupted, "挂起输入的会话同样算异常中断");
    assert_eq!(report.status.run_state, RunState::Terminated);
    assert_eq!(report.status.pending_input_kind, None, "终态不挂起输入");
}

#[test]
fn terminal_session_is_preserved_and_not_rewritten() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let id = Uuid::new_v4();
    let mut status = inflight();
    persist_transition(&store, id, &mut status, RunEvent::TurnCompleted, 2_000).expect("落盘");
    let before = std::fs::read_to_string(store.path_of(id)).expect("读落盘文件");

    let report = restore(&store, id, 9_000);
    assert!(!report.interrupted, "终态不被误改");
    assert!(report.events.is_empty(), "终态恢复不产生事件");
    assert_eq!(report.decision, Recovery::TerminalPreserved);
    assert_eq!(report.status, status, "终态逐字段不变");
    assert_eq!(
        report.status.terminal_outcome,
        Some(TerminalOutcome::Completed)
    );

    let after = std::fs::read_to_string(store.path_of(id)).expect("读落盘文件");
    assert_eq!(before, after, "终态恢复不得改写落盘文件");
}

#[test]
fn recovery_produces_exactly_one_interruption_event() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let id = Uuid::new_v4();
    let mut status = inflight();
    persist_transition(&store, id, &mut status, RunEvent::WorkingStarted, 1_000).expect("落盘");

    let report = restore(&store, id, 2_000);
    assert_eq!(
        report.events,
        vec![RunEvent::RuntimeTerminated],
        "恢复只产生那一条「中断」事件"
    );
}

#[test]
fn terminal_protection_neither_changes_nor_writes() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let id = Uuid::new_v4();
    let mut status = inflight();
    persist_transition(&store, id, &mut status, RunEvent::TurnCompleted, 1_000).expect("落盘");
    let before = std::fs::read_to_string(store.path_of(id)).expect("读落盘文件");

    // A26-1：终态收到 awaiting_input ⇒ 原样返回 ⇒ 没有迁移 ⇒ 不落盘。
    let record =
        persist_transition(&store, id, &mut status, RunEvent::AwaitingInput, 2_000).expect("落盘");
    assert!(!record.changed, "终态保护下不发生迁移");
    assert_eq!(record.status, status);
    let after = std::fs::read_to_string(store.path_of(id)).expect("读落盘文件");
    assert_eq!(before, after, "没有迁移就没有落盘");
}

#[test]
fn missing_persisted_state_yields_no_report() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let report = restore_on_startup(&store, Uuid::new_v4(), 1_000).expect("恢复");
    assert!(report.is_none(), "没有持久化状态就没什么可恢复的");
    assert!(store.load(Uuid::new_v4()).expect("读回").is_none());
}

#[test]
fn corrupt_or_foreign_persisted_file_is_reported_not_silently_ignored() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let id = Uuid::new_v4();
    std::fs::create_dir_all(store.path_of(id).parent().expect("父目录")).expect("建目录");
    std::fs::write(store.path_of(id), b"{ not json").expect("写坏文件");
    let err = store.load(id).expect_err("坏文件必须报错");
    assert!(err.to_string().contains("JSON"), "坏文件报 JSON 错: {err}");

    // 会话 id 与文件名不匹配 ⇒ 报错（不把别的会话的状态当本会话的）。
    let mut record = inflight();
    record.apply(RunEvent::TurnCompleted, 1_000);
    let foreign = json!({
        "sessionId": Uuid::new_v4().to_string(),
        "runStatus": record,
        "updatedAt": 1_000u64,
    });
    std::fs::write(
        store.path_of(id),
        serde_json::to_vec(&foreign).expect("序列化"),
    )
    .expect("写文件");
    let err = store.load(id).expect_err("会话不匹配必须报错");
    assert!(err.to_string().contains("会话不匹配"), "{err}");
}

#[test]
fn store_scans_and_removes_persisted_sessions() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = store_at(&dir);
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let mut status = RunStatus::initial(0);
    persist_transition(&store, a, &mut status, RunEvent::TurnStarted, 1_000).expect("落盘");
    persist_transition(&store, b, &mut status, RunEvent::TurnStarted, 1_000).expect("落盘");

    let mut listed = store.sessions();
    listed.sort();
    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(listed, expected, "启动扫描要能列出全部落盘会话");

    store.remove(a).expect("删除");
    assert!(store.load(a).expect("读回").is_none());
    assert_eq!(store.sessions(), vec![b]);
    store.remove(a).expect("重复删除视为已清理");
}

#[test]
fn symlinked_store_dir_and_file_are_refused() {
    let dir = tempfile::tempdir().expect("临时目录");
    let outside = tempfile::tempdir().expect("外部目录");
    let link = dir.path().join("run-status");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), &link).expect("造 symlink 目录");
        let store = SessionStatusStore::new(&link);
        let mut status = RunStatus::initial(0);
        let err = persist_transition(
            &store,
            Uuid::new_v4(),
            &mut status,
            RunEvent::TurnStarted,
            1,
        )
        .expect_err("symlink 目录不授权写入");
        assert!(err.to_string().contains("真实目录"), "{err}");
    }
}

#[test]
fn stale_inflight_uses_updated_at_as_termination_timestamp() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    let updated = persist_inflight(&dir, id, 10_000);

    let store = store_at(&dir);
    // 超过 30 分钟 ⇒ wb §1.5 的 `stale-inflight-to-terminated`：时间戳用 updatedAt。
    let report = restore(&store, id, updated + STALE_THRESHOLD_MS + 1);
    assert_eq!(report.decision, Recovery::StaleInflightToTerminated);
    assert!(report.interrupted);
    assert_eq!(
        report.status.last_transition_at, updated,
        "stale 分支的时间戳取 updatedAt（wb 原文）"
    );
    assert_eq!(report.status.last_activity_at, Some(updated));
    assert_eq!(report.status.derive(), DisplayStatus::Terminated);
}

#[test]
fn fresh_inflight_still_interrupts_but_keeps_decision_class() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    let updated = persist_inflight(&dir, id, 10_000);

    let store = store_at(&dir);
    // 未过 30 分钟 ⇒ wb 分类是 `fallback-to-initial`，但卡面口径仍要中断终结。
    let report = restore(&store, id, updated + 1);
    assert_eq!(report.decision, Recovery::FallbackToInitial);
    assert!(report.interrupted, "在途态一律按异常中断收口");
    assert_eq!(
        report.status.last_transition_at,
        updated + 1,
        "未过阈值时终结时刻取恢复时刻"
    );
}

#[test]
fn never_started_session_is_left_alone() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    {
        // 从未开始过的会话：idle 初态（wb 的 `fallback-to-initial` 形态）。
        let store = store_at(&dir);
        save_raw(&store, id, RunStatus::initial(1_000), 1_000);
    }
    let store = store_at(&dir);
    let report = restore(&store, id, 2_000);
    assert!(!report.interrupted, "没有在途工作可中断");
    assert!(report.events.is_empty(), "不产生多余事件");
    assert_eq!(report.decision, Recovery::FallbackToInitial);
    assert_eq!(report.status.run_state, RunState::Idle);
    assert_eq!(report.status.turn_seq, 0);
}

#[test]
fn archived_session_is_not_terminated() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    {
        let store = store_at(&dir);
        let mut status = RunStatus::initial(0);
        status.archived_at = Some(1_000);
        save_raw(&store, id, status, 1_000);
    }
    let store = store_at(&dir);
    let report = restore(&store, id, 2_000);
    assert!(!report.interrupted, "归档会话不是「在途」，不该被中断终结");
    assert_eq!(report.decision, Recovery::TerminalPreserved);
    assert_eq!(report.status.derive(), DisplayStatus::Archived);
    assert!(report.status.archived_at.is_some(), "归档标记不得被清掉");
}

/// A26-1 的终态判定是「`terminalOutcome !== null` **或** `runState ∈ {errored, terminated}`」
/// ——两者**任一**成立即终态。这条用例钉住「outcome 为终态但 runState 仍在途」的
/// 混合态：它必须按终态处理（不被中断终结），否则恢复会把一个已完成的会话
/// 改写成 terminated。
#[test]
fn terminal_outcome_with_inflight_run_state_is_still_terminal() {
    let dir = tempfile::tempdir().expect("临时目录");
    let id = Uuid::new_v4();
    {
        let store = store_at(&dir);
        let mut status = inflight();
        // run_state 仍是 running，但 outcome 已定 ⇒ A26-1 判终态。
        status.terminal_outcome = Some(TerminalOutcome::Completed);
        assert!(status.is_terminal());
        save_raw(&store, id, status, 1_100);
    }
    let store = store_at(&dir);
    let report = restore(&store, id, 9_000);
    assert!(!report.interrupted, "终态判定看 outcome，不只看 runState");
    assert_eq!(report.decision, Recovery::TerminalPreserved);
    assert_eq!(
        report.status.terminal_outcome,
        Some(TerminalOutcome::Completed)
    );
    assert_eq!(
        report.status.run_state,
        RunState::Running,
        "原样返回，不改状态"
    );
}
