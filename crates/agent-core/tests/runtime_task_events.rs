//! Runtime 任务事件接线（W1-core）：任务变更落事件日志 + 启动重建（TDD）。
//!
//! 对齐 opencode「状态变更先成为 durable 事件、读侧由事件投影、重启从事件重建」：
//! - 写侧：create / update_status / update_task_text 在状态确实改变时落一条
//!   `task.created` / `task.updated`，payload 直接复用 `agent_tasks::TaskEvent` 的
//!   serde（字段名与 agent-tasks 完全一致，不另造第二套）；
//! - 同值更新与被拒绝的变更（非法迁移）不落事件；
//! - 启动：未注入看板时按事件 seq 顺序重建看板；注入看板优先、不重建。
//!
//! 详见 docs/runtime-task-events-status.md。

use std::cell::Cell;
use std::sync::Arc;

use agent_core::provider::EchoProvider;
use agent_core::runtime::{CreateSession, Runtime, TaskStatus};
use agent_core::schema::Isolation;

// 线程本地受控时钟（Unix 毫秒）：保证事件 occurred_at_ms 断言确定。
thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

fn clock() -> u64 {
    CLOCK.with(|c| c.get())
}

fn set_clock(ms: u64) {
    CLOCK.with(|c| c.set(ms));
}

fn board() -> agent_tasks::TaskBoard {
    agent_tasks::TaskBoard::with_clock(clock)
}

fn runtime(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .task_board(board())
        .build()
        .expect("runtime")
}

fn tempdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "runtime-task-events-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

fn session(rt: &Runtime, title: &str) -> uuid::Uuid {
    rt.create_session(&CreateSession::plain(title, Isolation::local_default()))
        .expect("create session")
        .id
}

/// 该会话的全部任务事件（kind 前缀 `task.`），按日志 seq 升序。
fn task_events(rt: &Runtime, session_id: uuid::Uuid) -> Vec<agent_core::schema::Event> {
    rt.event_log()
        .replay(session_id, 0, 500)
        .expect("replay")
        .into_iter()
        .filter(|event| event.kind.starts_with("task."))
        .collect()
}

fn payload_type(event: &agent_core::schema::Event) -> String {
    event
        .payload
        .get("type")
        .and_then(|value| value.as_str())
        .expect("payload 应带 type 标签")
        .to_string()
}

#[test]
fn create_task_logs_created_event_matching_board() {
    set_clock(10_000);
    let rt = runtime(&tempdir("create"));
    let s = session(&rt, "S");
    let task = rt
        .create_task(s, "接线事件", "落一条 task.created")
        .expect("create");

    let events = task_events(&rt, s);
    assert_eq!(events.len(), 1, "create 恰好落一条任务事件");
    let event = &events[0];
    assert_eq!(event.kind, "task.created", "kind 与事件体 type 一致");
    assert_eq!(payload_type(event), "task.created");

    let parsed = serde_json::from_value::<agent_tasks::TaskEvent>(event.payload.clone())
        .expect("payload 应能反序列化回 TaskEvent");
    match parsed {
        agent_tasks::TaskEvent::Created {
            task_id,
            occurred_at_ms,
            snapshot,
        } => {
            assert_eq!(task_id, task.id);
            assert_eq!(occurred_at_ms, task.created_at_ms);
            assert_eq!(snapshot, task, "事件快照与看板任务逐字段一致");
        }
        other => panic!("应为 task.created 变体: {other:?}"),
    }
}

#[test]
fn update_status_logs_updated_event_with_only_changed_fields() {
    set_clock(11_000);
    let rt = runtime(&tempdir("status"));
    let s = session(&rt, "S");
    let task = rt.create_task(s, "状态事件", "").expect("create");

    set_clock(11_100);
    let updated = rt
        .update_task_status(s, task.id, TaskStatus::InProgress)
        .expect("update");

    let events = task_events(&rt, s);
    assert_eq!(events.len(), 2, "created + updated");
    let event = &events[1];
    assert_eq!(event.kind, "task.updated");
    assert_eq!(payload_type(event), "task.updated");

    let object = event.payload.as_object().expect("payload 为对象");
    assert_eq!(
        object.get("status").and_then(|value| value.as_str()),
        Some("in_progress"),
        "只带变化字段 status"
    );
    assert!(
        !object.contains_key("subject"),
        "未变化的 subject 不得出现在 payload"
    );

    let parsed = serde_json::from_value::<agent_tasks::TaskEvent>(event.payload.clone())
        .expect("payload 应能反序列化回 TaskEvent");
    match parsed {
        agent_tasks::TaskEvent::Updated {
            task_id,
            occurred_at_ms,
            subject,
            status,
        } => {
            assert_eq!(task_id, task.id);
            assert_eq!(status, Some(TaskStatus::InProgress));
            assert_eq!(subject, None);
            assert_eq!(occurred_at_ms, updated.updated_at_ms);
        }
        other => panic!("应为 task.updated 变体: {other:?}"),
    }
}

#[test]
fn no_change_or_rejected_updates_log_no_event() {
    set_clock(12_000);
    let rt = runtime(&tempdir("noop"));
    let s = session(&rt, "S");
    let task = rt.create_task(s, "无变化", "描述").expect("create");
    assert_eq!(task_events(&rt, s).len(), 1);

    // 同值状态更新：幂等成功，但不落事件。
    set_clock(12_100);
    let same = rt
        .update_task_status(s, task.id, TaskStatus::Pending)
        .expect("同值幂等");
    assert_eq!(same, task, "同值更新不得刷新任务");
    assert_eq!(task_events(&rt, s).len(), 1, "同值更新不落事件");

    // 同值文本更新：成功，不落事件。
    rt.update_task_text(s, task.id, Some(&task.subject), None)
        .expect("同值文本");
    assert_eq!(task_events(&rt, s).len(), 1, "同值文本更新不落事件");

    // 非法迁移（pending→done 跳步）：报错且不落事件。
    set_clock(12_200);
    assert!(
        rt.update_task_status(s, task.id, TaskStatus::Done).is_err(),
        "跳步迁移应被拒绝"
    );
    assert_eq!(task_events(&rt, s).len(), 1, "被拒绝的变更不落事件");

    // 拒绝路径零副作用：任务原样。
    assert_eq!(rt.get_task(s, task.id).expect("reread"), task);
}

#[test]
fn restart_rebuilds_board_from_event_log() {
    let dir = tempdir("restart");
    let s;
    let final_task;
    {
        let rt = runtime(&dir);
        s = session(&rt, "S");
        set_clock(12_000);
        let task = rt.create_task(s, "重启恢复", "描述保持").expect("create");
        set_clock(12_100);
        rt.update_task_status(s, task.id, TaskStatus::InProgress)
            .expect("update status");
        set_clock(12_200);
        final_task = rt
            .update_task_text(s, task.id, Some("重启后新标题"), None)
            .expect("update text");
    }

    // 同一 workdir 重启：未注入看板 ⇒ 从事件日志重建（默认系统时钟看板）。
    let rt2 = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .expect("restart runtime");
    let listed = rt2.list_tasks(s);
    assert_eq!(listed.len(), 1, "重启后任务不得丢失");
    assert_eq!(listed[0].id, final_task.id);
    assert_eq!(listed[0].subject, "重启后新标题");
    assert_eq!(listed[0].status, TaskStatus::InProgress);
    assert_eq!(
        listed[0], final_task,
        "重建结果与停机前终态逐字段一致（含时间戳）"
    );
}

#[test]
fn injected_board_wins_over_startup_rebuild() {
    let dir = tempdir("inject");
    let s;
    {
        let rt = runtime(&dir);
        s = session(&rt, "S");
        set_clock(13_000);
        rt.create_task(s, "日志里的任务", "").expect("create");
    }

    // 注入空看板：不做重建，日志里的任务不得凭空出现。
    let empty = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .task_board(agent_tasks::TaskBoard::new())
        .build()
        .expect("injected empty");
    assert!(
        empty.list_tasks(s).is_empty(),
        "注入看板优先：启动不得用日志重建覆盖注入内容"
    );

    // 注入带任务的自定义看板：原样保留，也不与日志合并。
    set_clock(13_100);
    let mut custom = board();
    let owned = custom.create(s, "注入任务", "注入描述").expect("seed");
    let kept = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .task_board(custom)
        .build()
        .expect("injected custom");
    let listed = kept.list_tasks(s);
    assert_eq!(listed.len(), 1, "只含注入任务，不合并日志重建");
    assert_eq!(listed[0], owned, "注入内容原样保留");
}

#[test]
fn task_events_share_seq_stream_with_existing_events() {
    set_clock(14_000);
    let rt = runtime(&tempdir("seq"));
    let s = session(&rt, "S");
    let task = rt.create_task(s, "同流", "").expect("create");

    // 被拒绝的变更不占 seq：pending→done 跳步报错、不落事件。
    assert!(rt.update_task_status(s, task.id, TaskStatus::Done).is_err());
    // 同值更新也不占 seq。
    rt.update_task_status(s, task.id, TaskStatus::Pending)
        .expect("同值幂等");
    // 合法迁移落事件：seq 接续既有事件单调递增。
    set_clock(14_100);
    rt.update_task_status(s, task.id, TaskStatus::InProgress)
        .expect("update");

    let events = rt.event_log().replay(s, 0, 500).expect("replay");
    let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3], "seq 与既有事件同流且连续单调");
    let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["session_update", "task.created", "task.updated"],
        "任务事件与 session_update 同一分片同流"
    );
}

#[test]
fn update_task_text_logs_updated_event_and_bare_event_for_description_only() {
    set_clock(15_000);
    let rt = runtime(&tempdir("text"));
    let s = session(&rt, "S");
    let task = rt.create_task(s, "文本事件", "旧描述").expect("create");

    // subject 变更：task.updated 只带 subject，不带 status。
    set_clock(15_100);
    rt.update_task_text(s, task.id, Some("新标题"), None)
        .expect("改标题");
    let events = task_events(&rt, s);
    let subject_event = &events[1];
    assert_eq!(subject_event.kind, "task.updated");
    let object = subject_event.payload.as_object().expect("payload 为对象");
    assert_eq!(
        object.get("subject").and_then(|value| value.as_str()),
        Some("新标题")
    );
    assert!(!object.contains_key("status"), "status 未变化不得出现");

    // description-only 变更：TaskEvent 冻结形状不含 description，落一条不带
    // 可选字段的「裸」task.updated 作审计标记（重启回放不可恢复 description，
    // 已知缺口，见 docs/runtime-task-events-status.md）。
    set_clock(15_200);
    rt.update_task_text(s, task.id, None, Some("新描述"))
        .expect("改描述");
    let events = task_events(&rt, s);
    assert_eq!(
        events.len(),
        3,
        "description 变更也必须落事件（有变化必有事件）"
    );
    let bare = &events[2];
    assert_eq!(bare.kind, "task.updated");
    let object = bare.payload.as_object().expect("payload 为对象");
    assert!(
        !object.contains_key("subject") && !object.contains_key("status"),
        "裸事件不带任何可选字段"
    );
    assert!(
        serde_json::from_value::<agent_tasks::TaskEvent>(bare.payload.clone()).is_ok(),
        "裸事件仍是合法 TaskEvent"
    );
}
