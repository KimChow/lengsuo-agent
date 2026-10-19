//! 会话重建边界与降级：旧日志、畸形 payload、分页取全量、重复重建幂等。
//!
//! 主路径用例见 `session_rebuild.rs`；共享 harness 见 `session_rebuild_support`。

mod session_rebuild_support;
use agent_core::runtime::CONTENT_PREVIEW_CHARS;
use session_rebuild_support::*;

/// 例 5：畸形 payload（`tool_use` 缺 `name`、另一条 `text` 非字符串）⇒ 跳过该条，
/// 其余正常重建；启动不 panic、不中断。
#[test]
fn malformed_payloads_are_skipped_without_aborting_startup() {
    let dir = workdir("malformed");
    let sid = Uuid::new_v4();
    let lines = [
        header_line(sid),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"user","payload":{{"text":"正常用户"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"tool_use","payload":{{"args":{{"path":"a.txt"}}}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"text","payload":{{"text":123}},"tsMs":30}}"#
        ),
        format!(
            r#"{{"seq":4,"sessionId":"{sid}","kind":"text","payload":{{"text":"坏事件之后仍要重建"}},"tsMs":40}}"#
        ),
        format!(
            r#"{{"seq":5,"sessionId":"{sid}","kind":"tool_result","payload":{{"name":"fs.read","preview":"ok","resultRef":"blob-1"}},"tsMs":50}}"#
        ),
    ];
    write_log(&dir, sid, &lines);

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("坏事件不得让运行时起不来");
    assert_eq!(
        shape(&restored.messages),
        vec![
            (Role::User, "正常用户".to_string(), 10),
            (Role::Assistant, "坏事件之后仍要重建".to_string(), 40),
            (Role::ToolResult, "ok".to_string(), 50),
        ],
        "坏事件跳过，其余按 seq 顺序照常重建"
    );
    let last = restored.messages.last().expect("有 ToolResult");
    assert_eq!(tool_of(last).name, "fs.read");
    assert_eq!(tool_of(last).result_ref.as_deref(), Some("blob-1"));
}

/// 例 6：`last_seq` 与重启前一致；重建后继续 `send_text` 接在历史之后（不重复旧消息）。
#[test]
fn last_seq_is_preserved_and_new_turn_appends_after_history() {
    let dir = workdir("seq");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "第一句").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let after = reopen(&dir, scripted(1));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        restored.last_seq, before.last_seq,
        "last_seq 仍取 log.last_seq(id)，重启前后一致"
    );

    // 重建后继续对话：历史接在原历史之后，旧消息不重复。
    after
        .send_text(before.id, "第二句")
        .expect("turn after restart");
    let live = after.get_session(before.id).expect("session");
    assert_eq!(
        shape(&live.messages),
        vec![
            (Role::User, "第一句".to_string(), before.messages[0].ts_ms),
            (
                Role::Assistant,
                "重启后应答".to_string(),
                before.messages[1].ts_ms
            ),
            (Role::User, "第二句".to_string(), live.messages[2].ts_ms),
            (
                Role::Assistant,
                "重启后应答".to_string(),
                live.messages[3].ts_ms
            ),
        ],
        "新回合必须接在重建历史之后"
    );
    let users: Vec<&Message> = live
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .collect();
    assert_eq!(users.len(), 2, "旧用户消息不得重复");
    assert_eq!(users[0].content, "第一句");
    assert_eq!(users[1].content, "第二句");
    assert!(live.last_seq > before.last_seq, "last_seq 继续单调前进");
    assert!(
        after.event_log().last_seq(before.id).expect("last_seq") == live.last_seq,
        "会话 last_seq 与日志一致"
    );
}

/// 分页取全量：超过 `DEFAULT_PAGE_LIMIT`（500）条事件的重建不得只取首页。
#[test]
fn rebuild_pages_through_all_events() {
    let dir = workdir("page");
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let mut lines = vec![format!(
        r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#
    )];
    // 600 条 user + 600 条 text 交替（远超单页 500），全部必须重建。
    let total = 600u64;
    for i in 0..total {
        let user_seq = 2 * i + 1;
        let text_seq = 2 * i + 2;
        lines.push(format!(
            r#"{{"seq":{user_seq},"sessionId":"{sid}","kind":"user","payload":{{"text":"u{i}"}},"tsMs":{user_seq}}}"#
        ));
        lines.push(format!(
            r#"{{"seq":{text_seq},"sessionId":"{sid}","kind":"text","payload":{{"text":"a{i}"}},"tsMs":{text_seq}}}"#
        ));
    }
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write long log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("session");
    assert_eq!(
        restored.messages.len(),
        (total * 2) as usize,
        "必须分页取全量，不得只取 500 条就停"
    );
    assert_eq!(restored.messages[0].content, "u0");
    assert_eq!(
        restored.messages.last().expect("last").content,
        format!("a{}", total - 1),
        "分页边界之后的事件也必须重建"
    );
    assert_eq!(restored.last_seq, total * 2);
}

/// 重建是只读操作：启动回放不得为「重建」额外写任何事件。
#[test]
fn rebuild_writes_no_events() {
    let dir = workdir("readonly");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "只读回放").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let log_seq_before = before.last_seq;
    let _after = reopen(&dir, scripted(0));
    let events = log_events(&dir, before.id);
    assert_eq!(
        events.len() as u64,
        log_seq_before,
        "重建是只读操作：事件条数不得变化"
    );
    assert!(
        events
            .iter()
            .all(|e| e["kind"] != "rebuild" && e["kind"] != "replay"),
        "不得为重建写事件"
    );
}

/// 大输出（> `CONTENT_PREVIEW_CHARS`）⇒ 重启后 `content` 仍是 preview、
/// `result_ref` 仍指向 blob（与运行期同一口径）。
#[test]
fn truncated_result_preview_survives_restart() {
    let big = "x".repeat(CONTENT_PREVIEW_CHARS + 100);
    let dir = workdir("trunc");
    let before = {
        let rt = runtime_with_executor(
            &dir,
            ScriptedProvider::new(vec![
                tool_step("fs.read", json!({"path": "big.txt"})),
                text_step("读完"),
            ]),
            ScriptedExecutor::ok(json!({ "data": big })),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "读 big.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let expected = before
        .messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("前置：运行期有 ToolResult")
        .clone();
    assert!(
        expected.content.contains("截断"),
        "前置：运行期只留 preview"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("session");
    let got = restored
        .messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("ToolResult");
    assert_eq!(got.content, expected.content, "preview 逐字一致");
    assert_eq!(
        tool_of(got).result_ref,
        tool_of(&expected).result_ref,
        "result_ref 指向同一 blob"
    );
}

/// 未识别的 kind 一律跳过：`thinking` / `done` / `warning` / `approval_*` /
/// `task.*` / `session_update` / `files` / `error` 都不产出消息。
#[test]
fn non_message_kinds_are_not_projected() {
    let dir = workdir("kinds");
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let kinds = [
        ("thinking", json!({"text": "想一下"})),
        ("done", json!({"status": "completed"})),
        ("warning", json!({"kind": "x"})),
        ("approval_request", json!({"summary": "s"})),
        ("approval_resolved", json!({"approved": true})),
        ("task.created", json!({"id": "t1"})),
        ("session_update", json!({"status": "idle"})),
        ("files", json!({"files": []})),
        ("error", json!({"message": "boom"})),
    ];
    let mut lines = vec![format!(
        r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#
    )];
    for (i, (kind, payload)) in kinds.iter().enumerate() {
        lines.push(format!(
            r#"{{"seq":{},"sessionId":"{sid}","kind":"{kind}","payload":{},"tsMs":{}}}"#,
            i + 1,
            payload,
            (i + 1) * 10
        ));
    }
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write kinds log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("session");
    assert!(
        restored.messages.is_empty(),
        "非消息 kind 不得投影出消息: {:?}",
        restored.messages
    );
    assert_eq!(restored.last_seq, kinds.len() as u64, "seq 锚点照常恢复");
}
