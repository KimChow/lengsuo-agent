//! Runtime 任务工具族接线（W1）：`TaskBoard` 经 `*_for_session` 会话门接入
//! Runtime 域层能力面，对齐 参考客户端 TaskCreate/TaskGet/TaskUpdate/TaskList 行为面。
//!
//! 红线（docs/task-owned-access-status.md 末段冻结约束）：
//! - 越权与不存在返回同一 `CoreError::Task`（文案同 `TaskError::NotFound`）；
//! - 拒绝路径零副作用（不改状态、不刷新 `updated_at_ms`、不动会话索引）；
//! - 状态机与时间戳语义完全复用 `agent-tasks`，Runtime 不另造一份。

use std::cell::Cell;
use std::sync::Arc;

use agent_core::provider::EchoProvider;
use agent_core::runtime::{CreateSession, Runtime, Task, TaskStatus};
use agent_core::schema::Isolation;
use agent_core::CoreError;

// 线程本地受控时钟（Unix 毫秒）：测试内 `set_clock` 推进，时间戳断言确定。
// 每个测试跑在独立线程，互不干扰。
thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

fn clock() -> u64 {
    CLOCK.with(|c| c.get())
}

fn set_clock(ms: u64) {
    CLOCK.with(|c| c.set(ms));
}

fn rt(tag: &str) -> Runtime {
    let dir = std::env::temp_dir().join(format!("runtime-tasks-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .task_board(agent_tasks::TaskBoard::with_clock(clock))
        .build()
        .expect("runtime")
}

fn session(rt: &Runtime, title: &str) -> uuid::Uuid {
    rt.create_session(&CreateSession::plain(title, Isolation::local_default()))
        .expect("create session")
        .id
}

fn err_text(r: Result<Task, CoreError>) -> String {
    r.expect_err("应当失败").to_string()
}

#[test]
fn create_then_list_is_scoped_to_session() {
    set_clock(1_000);
    let rt = rt("scope");
    let a = session(&rt, "A");
    let b = session(&rt, "B");
    let t1 = rt
        .create_task(a, "跑构建", "cargo test 全绿")
        .expect("create");
    let t2 = rt
        .create_task(a, "修类型", "修 10 个类型错误")
        .expect("create");
    rt.create_task(b, "别家任务", "不应可见").expect("create");

    let ids_a: Vec<uuid::Uuid> = rt.list_tasks(a).iter().map(|t| t.id).collect();
    assert_eq!(ids_a.len(), 2, "本会话应看到自己的两个任务");
    assert!(ids_a.contains(&t1.id) && ids_a.contains(&t2.id));

    let ids_b: Vec<uuid::Uuid> = rt.list_tasks(b).iter().map(|t| t.id).collect();
    assert_eq!(ids_b.len(), 1, "另一会话的 list 不得看到本会话任务");
    assert!(!ids_b.contains(&t1.id) && !ids_b.contains(&t2.id));
}

#[test]
fn cross_session_access_is_not_found_and_leaves_task_untouched() {
    set_clock(2_000);
    let rt = rt("cross");
    let a = session(&rt, "A");
    let b = session(&rt, "B");
    let task = rt.create_task(a, "机密", "他人不可见").expect("create");
    let not_found = agent_tasks::TaskError::NotFound.to_string();

    // 越权读与「不存在」返回同一文案，调用方无法区分。
    assert_eq!(err_text(rt.get_task(b, task.id)), not_found);
    assert_eq!(err_text(rt.get_task(a, uuid::Uuid::new_v4())), not_found);
    // 越权状态更新同样只报 NotFound（不暴露 InvalidTransition，那会泄露当前状态）。
    assert_eq!(
        err_text(rt.update_task_status(b, task.id, TaskStatus::InProgress)),
        not_found
    );
    // 越权文本更新（Runtime 侧组合门）同报 NotFound。
    assert_eq!(
        err_text(rt.update_task_text(b, task.id, Some("篡改"), Some("篡改"))),
        not_found
    );

    let after = rt.get_task(a, task.id).expect("owner 仍可读");
    assert_eq!(after, task, "拒绝路径零副作用：状态与 updated_at_ms 均未变");
}

#[test]
fn legal_transition_chain_and_invalid_rejected_without_side_effect() {
    set_clock(3_000);
    let rt = rt("transition");
    let s = session(&rt, "S");
    let task = rt.create_task(s, "上线", "部署").expect("create");

    let wip = rt
        .update_task_status(s, task.id, TaskStatus::InProgress)
        .expect("pending→in_progress 合法");
    assert_eq!(wip.status, TaskStatus::InProgress);
    assert!(wip.updated_at_ms >= task.updated_at_ms);

    let done = rt
        .update_task_status(s, task.id, TaskStatus::Done)
        .expect("in_progress→done 合法");
    assert_eq!(done.status, TaskStatus::Done);

    // 同状态幂等：不报错、不刷新时间。
    let again = rt
        .update_task_status(s, task.id, TaskStatus::Done)
        .expect("done→done 幂等");
    assert_eq!(again, done);

    // 终态迁出非法：CoreError::Task 且任务原样。
    let err = rt
        .update_task_status(s, task.id, TaskStatus::InProgress)
        .expect_err("终态不可迁出");
    assert!(matches!(err, CoreError::Task(_)));
    assert_eq!(rt.get_task(s, task.id).expect("reread"), done);

    // pending→done 跳步非法且零副作用。
    let t2 = rt.create_task(s, "跳步", "pending→done").expect("create");
    set_clock(3_500);
    let err = rt
        .update_task_status(s, t2.id, TaskStatus::Done)
        .expect_err("跳步非法");
    assert!(matches!(err, CoreError::Task(_)));
    assert_eq!(
        rt.get_task(s, t2.id).expect("reread"),
        t2,
        "非法迁移不得改状态也不得刷新 updated_at_ms"
    );
}

#[test]
fn update_task_text_rejects_blank_and_keeps_timestamp_on_noop() {
    set_clock(4_000);
    let rt = rt("text");
    let s = session(&rt, "S");
    let task = rt.create_task(s, "标题", "描述").expect("create");

    // 空白 subject 拒绝且零副作用。
    let err = rt
        .update_task_text(s, task.id, Some("   "), None)
        .expect_err("空白 subject");
    assert_eq!(
        err.to_string(),
        agent_tasks::TaskError::EmptySubject.to_string()
    );
    assert_eq!(rt.get_task(s, task.id).expect("reread"), task);

    // 同值写入：成功但不得刷新 updated_at_ms。
    set_clock(4_100);
    let same = rt
        .update_task_text(s, task.id, Some(&task.subject), Some(&task.description))
        .expect("同值写入");
    assert_eq!(same, task, "字段未变化不得刷新时间");

    // 真实变更：subject 换新、时间只前进。
    set_clock(4_200);
    let changed = rt
        .update_task_text(s, task.id, Some("新标题"), None)
        .expect("改标题");
    assert_eq!(changed.subject, "新标题");
    assert_eq!(changed.description, task.description);
    assert!(changed.updated_at_ms > task.updated_at_ms);
}

#[test]
fn list_order_is_stable_by_created_then_id() {
    let rt = rt("order");
    let s = session(&rt, "S");
    set_clock(5_000);
    let t1 = rt.create_task(s, "a", "").expect("create");
    let t2 = rt.create_task(s, "b", "").expect("create");
    set_clock(5_100);
    let t3 = rt.create_task(s, "c", "").expect("create");
    set_clock(5_200);
    let t4 = rt.create_task(s, "d", "").expect("create");
    assert_eq!(
        t1.created_at_ms, t2.created_at_ms,
        "前提：受控时钟下 t1/t2 同毫秒"
    );

    let listed = rt.list_tasks(s);
    // 期望全序：created_at_ms 升序，同毫秒按 id 升序。
    let mut expected = [t1, t2, t3, t4];
    expected.sort_by(|a, b| {
        a.created_at_ms
            .cmp(&b.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    let got: Vec<(u64, uuid::Uuid)> = listed.iter().map(|t| (t.created_at_ms, t.id)).collect();
    let want: Vec<(u64, uuid::Uuid)> = expected.iter().map(|t| (t.created_at_ms, t.id)).collect();
    assert_eq!(got, want, "list 顺序必须与 (created_at_ms, id) 全序一致");
}

#[test]
fn create_rejects_blank_subject_without_leaving_trace() {
    set_clock(6_000);
    let rt = rt("blank");
    let s = session(&rt, "S");
    let err = rt.create_task(s, "  ", "描述").expect_err("空白 subject");
    assert_eq!(
        err.to_string(),
        agent_tasks::TaskError::EmptySubject.to_string()
    );
    assert!(rt.list_tasks(s).is_empty(), "拒绝创建不得在看板留痕");
}
