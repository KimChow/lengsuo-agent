//! 任务事件测试：apply 幂等规则、状态机不被绕过、replay 确定性、serde 契约。

mod support;

use agent_tasks::{Task, TaskBoard, TaskEvent, TaskStatus};
use serde_json::json;
use uuid::Uuid;

/// 直接构造任务快照：事件路径的时间戳全部显式给出，不依赖看板时钟。
fn task(id: Uuid, session: Uuid, subject: &str, at_ms: u64) -> Task {
    Task {
        id,
        session_id: session,
        subject: subject.to_string(),
        description: "描述".to_string(),
        status: TaskStatus::Pending,
        created_at_ms: at_ms,
        updated_at_ms: at_ms,
    }
}

fn created(task: &Task) -> TaskEvent {
    TaskEvent::Created {
        task_id: task.id,
        occurred_at_ms: task.created_at_ms,
        snapshot: task.clone(),
    }
}

fn updated(
    task_id: Uuid,
    at_ms: u64,
    subject: Option<&str>,
    status: Option<TaskStatus>,
) -> TaskEvent {
    TaskEvent::Updated {
        task_id,
        occurred_at_ms: at_ms,
        subject: subject.map(str::to_string),
        status,
    }
}

/// 看板可观测状态：每个会话的任务快照列表（list 已按 (created_at, id) 排序）。
fn observable(b: &TaskBoard, sessions: &[Uuid]) -> Vec<Vec<Task>> {
    sessions
        .iter()
        .map(|s| b.list_by_session(*s).into_iter().cloned().collect())
        .collect()
}

// ---- 1. created 可见 + 重复幂等 ----

#[test]
fn apply_created_makes_task_visible_and_duplicate_is_idempotent() {
    let mut b = support::board();
    let id = Uuid::new_v4();
    let sid = Uuid::new_v4();
    let first = task(id, sid, "首个快照", 1_000);

    assert!(b.apply(&created(&first)), "首次 created 应被接受");
    assert_eq!(b.get(id).unwrap(), &first, "按 id 可见且逐字段等于快照");
    assert_eq!(b.list_by_session(sid).len(), 1, "会话索引含该任务");

    // 重复投递携带不同快照（更早 created_at、不同 subject）：不得覆盖既有内容。
    let mut stale = task(id, sid, "迟到的重复快照", 500);
    stale.status = TaskStatus::Done;
    assert!(!b.apply(&created(&stale)), "重复 created 幂等返回 false");
    let kept = b.get(id).unwrap();
    assert_eq!(kept.subject, "首个快照", "不覆盖既有 subject");
    assert_eq!(kept.created_at_ms, 1_000, "不得覆盖既有 created_at");
    assert_eq!(kept.status, TaskStatus::Pending, "不覆盖既有 status");
    assert_eq!(b.list_by_session(sid).len(), 1, "会话索引不重复追加");
}

// ---- 2. updated 未知 id 丢弃 ----

#[test]
fn apply_updated_unknown_id_is_dropped_without_insert() {
    let mut b = support::board();
    let existing = support::create(&mut b, "既有任务");
    let sid = existing.session_id;
    let ghost = Uuid::new_v4();

    assert!(
        !b.apply(&updated(ghost, 9_000, Some("幽灵"), Some(TaskStatus::Done))),
        "未知 id 的 updated 返回 false"
    );
    assert_eq!(b.get(ghost).err(), Some(agent_tasks::TaskError::NotFound));
    assert_eq!(b.list_by_session(sid).len(), 1, "条目数不变");
    assert_eq!(
        b.list_by_session(Uuid::new_v4()).len(),
        0,
        "不凭空插入到任何会话"
    );
}

// ---- 3. 事件路径不得绕过状态机 ----

#[test]
fn apply_updated_cannot_bypass_state_machine() {
    let mut b = support::board();
    let t = task(Uuid::new_v4(), Uuid::new_v4(), "s", 1_000);
    b.apply(&created(&t));

    // 非法迁移 Pending→Done：拒绝且状态不变。
    assert!(!b.apply(&updated(t.id, 1_100, None, Some(TaskStatus::Done))));
    assert_eq!(b.get(t.id).unwrap().status, TaskStatus::Pending);

    // 合法路径逐步推进后，终态不可再出。
    assert!(b.apply(&updated(t.id, 1_200, None, Some(TaskStatus::InProgress))));
    assert!(b.apply(&updated(t.id, 1_300, None, Some(TaskStatus::Done))));
    assert!(!b.apply(&updated(t.id, 1_400, None, Some(TaskStatus::InProgress))));
    assert_eq!(b.get(t.id).unwrap().status, TaskStatus::Done);

    // 空白 subject 与 update_text 同罪：整事件拒绝，无部分更新。
    let before = b.get(t.id).unwrap().clone();
    assert!(!b.apply(&updated(t.id, 1_500, Some("  "), None)));
    assert_eq!(b.get(t.id).unwrap(), &before, "拒绝路径零副作用");
}

// ---- 4. 只改给出字段 + updated_at 只前进 ----

#[test]
fn apply_updated_touches_only_given_fields_and_bumps_monotonically() {
    let mut b = support::board();
    let t = task(Uuid::new_v4(), Uuid::new_v4(), "s0", 1_000);
    b.apply(&created(&t));

    // 只给 subject：status 与 created_at 不动，updated_at 取事件时间。
    assert!(b.apply(&updated(t.id, 2_000, Some("s1"), None)));
    let after = b.get(t.id).unwrap();
    assert_eq!(after.subject, "s1");
    assert_eq!(after.status, TaskStatus::Pending);
    assert_eq!(after.created_at_ms, 1_000);
    assert_eq!(after.updated_at_ms, 2_000);

    // 乱序迟到的旧事件：内容仍应用，updated_at 不回拨。
    assert!(b.apply(&updated(t.id, 500, Some("s2"), None)));
    let stale = b.get(t.id).unwrap();
    assert_eq!(stale.subject, "s2");
    assert_eq!(stale.updated_at_ms, 2_000, "只前进不回拨");

    // 同值/同状态写入：接受但不刷新 updated_at（与 update_text/update_status 一致）。
    assert!(b.apply(&updated(t.id, 9_000, Some("s2"), Some(TaskStatus::Pending))));
    assert_eq!(b.get(t.id).unwrap().updated_at_ms, 2_000, "无变化不刷新");
}

// ---- 5. replay 与逐条 apply 等价 ----

#[test]
fn replay_equals_sequential_apply() {
    let sid = Uuid::new_v4();
    let a = task(Uuid::new_v4(), sid, "A", 1_000);
    let c = task(Uuid::new_v4(), sid, "C", 3_000);
    let other = task(Uuid::new_v4(), Uuid::new_v4(), "X", 2_000);
    let events = vec![
        created(&a),
        created(&other),
        updated(a.id, 1_500, None, Some(TaskStatus::InProgress)),
        created(&c),
        updated(a.id, 1_600, Some("A2"), None),
        updated(a.id, 1_700, None, Some(TaskStatus::Done)),
        updated(other.id, 2_500, Some("X2"), None),
    ];

    let mut sequential = support::board();
    for e in &events {
        sequential.apply(e);
    }
    let replayed = TaskBoard::replay(&events);

    let sessions = [sid, other.session_id];
    assert_eq!(
        observable(&replayed, &sessions),
        observable(&sequential, &sessions),
        "replay 与逐条 apply 逐字段相等"
    );
    for id in [a.id, c.id, other.id] {
        assert_eq!(replayed.get(id), sequential.get(id));
    }
}

// ---- 6. 乱序输入的确定性 ----

#[test]
fn replay_is_deterministic_regardless_of_input_order() {
    let sid = Uuid::new_v4();
    let a = task(Uuid::new_v4(), sid, "A", 100); // 与 c 同刻：按 id 分先后
    let c = task(Uuid::new_v4(), sid, "C", 100);
    let d = task(Uuid::new_v4(), sid, "D", 600); // created 与 updated 同刻同任务
    let (a_first, c_first) = if a.id < c.id {
        (a.id, c.id)
    } else {
        (c.id, a.id)
    };
    let events = vec![
        created(&a),
        created(&c),
        updated(a.id, 300, Some("A2"), None),
        created(&d),
        updated(d.id, 600, Some("D2"), None),
    ];
    let sessions = [sid];

    let canonical = TaskBoard::replay(&events);
    let sorted_ids: Vec<Uuid> = canonical
        .list_by_session(sid)
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(sorted_ids, vec![a_first, c_first, d.id], "同刻按 id 升序");
    assert_eq!(
        canonical.get(d.id).unwrap().subject,
        "D2",
        "同刻 created 先于 updated"
    );

    // 乱序变体：倒序 + 同刻对调 + updated 提前到 created 之前。
    let mut reversed = events.clone();
    reversed.reverse();
    let swapped = vec![
        events[1].clone(),
        events[3].clone(),
        events[4].clone(),
        events[0].clone(),
        events[2].clone(),
    ];
    let early_update = vec![
        events[4].clone(),
        events[0].clone(),
        events[1].clone(),
        events[2].clone(),
        events[3].clone(),
    ];
    for label in ["reversed", "swapped", "early_update"] {
        let perm = match label {
            "reversed" => &reversed,
            "swapped" => &swapped,
            _ => &early_update,
        };
        assert_eq!(
            observable(&TaskBoard::replay(perm), &sessions),
            observable(&canonical, &sessions),
            "输入顺序 {label} 不影响结果"
        );
    }
}

// ---- 7. serde 契约 ----

#[test]
fn event_json_roundtrip_keeps_tags_and_fields() {
    let t = task(Uuid::new_v4(), Uuid::new_v4(), "标题", 1_000);

    let ce = created(&t);
    let cv = serde_json::to_value(&ce).unwrap();
    assert_eq!(cv["type"], json!("task.created"), "变体标签稳定");
    assert_eq!(cv["task_id"], json!(t.id.to_string()));
    assert_eq!(cv["occurred_at_ms"], json!(1_000));
    let snapshot = cv["snapshot"].as_object().unwrap();
    let mut keys: Vec<&str> = snapshot.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "created_at_ms",
            "description",
            "id",
            "session_id",
            "status",
            "subject",
            "updated_at_ms"
        ],
        "snapshot 字段名复用 Task"
    );
    assert_eq!(snapshot["status"], json!("pending"));
    assert_eq!(
        serde_json::from_value::<TaskEvent>(cv).unwrap(),
        ce,
        "created 往返"
    );

    let ue = updated(t.id, 2_000, Some("新标题"), Some(TaskStatus::InProgress));
    let uv = serde_json::to_value(&ue).unwrap();
    assert_eq!(uv["type"], json!("task.updated"));
    assert_eq!(uv["subject"], json!("新标题"));
    assert_eq!(uv["status"], json!("in_progress"), "状态词复用 TaskStatus");
    assert_eq!(
        serde_json::from_value::<TaskEvent>(uv).unwrap(),
        ue,
        "updated 往返"
    );

    // 缺省字段不序列化（对照参考件 optional 语义），往返后仍为 None。
    let bare = updated(t.id, 2_001, None, None);
    let bv = serde_json::to_value(&bare).unwrap();
    assert!(bv.get("subject").is_none(), "None 字段省略: {bv}");
    assert!(bv.get("status").is_none());
    assert_eq!(serde_json::from_value::<TaskEvent>(bv).unwrap(), bare);
}

// ---- 8. created 的 task_id 与快照不一致时拒绝 ----

#[test]
fn apply_created_with_foreign_snapshot_id_is_rejected() {
    let mut b = support::board();
    let sid = Uuid::new_v4();
    let snapshot = task(Uuid::new_v4(), sid, "s", 1_000);
    let mismatched = TaskEvent::Created {
        task_id: Uuid::new_v4(),
        occurred_at_ms: 1_000,
        snapshot: snapshot.clone(),
    };
    assert!(
        !b.apply(&mismatched),
        "task_id 与 snapshot.id 不一致 ⇒ 拒绝"
    );
    assert_eq!(
        b.get(snapshot.id).err(),
        Some(agent_tasks::TaskError::NotFound)
    );
    assert!(b.list_by_session(sid).is_empty(), "两个 id 都不落盘");
}
