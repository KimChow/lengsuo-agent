//! 会话重建主路径：一轮回合后重启，消息序列与重启前逐条相等。
//!
//! 边界与降级用例见 `session_rebuild_edges.rs`；共享 harness 见 `session_rebuild_support`。

mod session_rebuild_support;
use session_rebuild_support::*;

#[test]
fn plain_turn_survives_restart() {
    let dir = workdir("plain");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "你好").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    assert_eq!(
        before.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        vec![Role::User, Role::Assistant],
        "前置：重启前历史形状"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        shape(&restored.messages),
        shape(&before.messages),
        "重启后消息必须与重启前逐条相等（role / content / ts_ms）"
    );
    // `ts_ms` 来源（既有语义，本卡不改）：运行期消息用内存打点 `now_ms()`，事件用
    // `EventLog::append` 自己的打点——同一毫秒级上下文但不是同一次时钟读数。重建
    // 一律取事件 `ts_ms`，故前置断言为「事件打点」，并另断两者同毫秒级。
    let event_ts = projected_event_timestamps(&dir, before.id);
    assert_eq!(
        restored.messages.len(),
        event_ts.len(),
        "事件条数与消息条数一致"
    );
    for (message, ts) in restored.messages.iter().zip(event_ts.iter()) {
        assert_eq!(
            message.ts_ms, *ts,
            "重建的 ts_ms 取事件打点（{}，非内存消息打点）",
            message.content
        );
    }
    for message in &before.messages {
        assert!(
            restored
                .messages
                .iter()
                .any(|m| m.ts_ms >= message.ts_ms && m.ts_ms - message.ts_ms <= 5),
            "重建 ts_ms 与重启前内存打点同毫秒级上下文: {}",
            message.content
        );
    }
}

/// 例 2：工具回合后重启 ⇒ ToolCall + ToolResult 成对保留，`ToolResult.content`
/// 为 preview、`result_ref` 与重启前一致；`ToolCall.args_ref` 为 `None`
/// （`tool_use` 事件 payload 无 `args_ref` 的已知缺口，显式断言）。
#[test]
fn tool_turn_rebuilds_paired_call_and_result() {
    let dir = workdir("tool");
    let before = {
        let rt = runtime_with_executor(
            &dir,
            tool_turn_provider(),
            ScriptedExecutor::ok(json!({"lines": 3})),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "读 a.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let calls: Vec<&Message> = before
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolCall)
        .collect();
    let results: Vec<&Message> = before
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    assert_eq!(
        (calls.len(), results.len()),
        (1, 1),
        "前置：一对 ToolCall/ToolResult"
    );
    assert_eq!(tool_of(results[0]).name, "fs.read");

    let after = runtime_with_executor(&dir, scripted(0), ScriptedExecutor::ok(json!({})));
    let restored = after.get_session(before.id).expect("restored session");
    let rebuilt_calls: Vec<&Message> = restored
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolCall)
        .collect();
    let rebuilt_results: Vec<&Message> = restored
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    let (c, r) = (rebuilt_calls.len(), rebuilt_results.len());
    assert_eq!((c, r), (1, 1), "成对重建: {:?}", restored.messages);
    assert_eq!(
        tool_of(rebuilt_calls[0]).name,
        "fs.read",
        "ToolCall 的工具名取自 `tool_use` 事件 payload"
    );
    assert_eq!(
        rebuilt_calls[0].content, calls[0].content,
        "ToolCall.content 经同一 `summarize_args` 重建，必须逐字相同"
    );
    assert_eq!(
        rebuilt_calls[0]
            .tool
            .as_ref()
            .and_then(|t| t.args_ref.clone()),
        None,
        "已知缺口：`tool_use` 事件没有 args_ref，重建只能留 None"
    );
    assert_eq!(
        rebuilt_results[0].content, results[0].content,
        "ToolResult.content 取自事件 payload.preview"
    );
    assert_eq!(
        tool_of(rebuilt_results[0]).result_ref,
        tool_of(results[0]).result_ref,
        "result_ref 必须与重启前一致"
    );
}

/// 例 3：提醒回合（两条 `user` 事件）⇒ 重启后顺序为 user / assistant / user / assistant。
#[test]
fn reminder_turn_keeps_message_order_across_restart() {
    let dir = workdir("reminder");
    let before = {
        let rt = reopen(
            &dir,
            ScriptedProvider::new(vec![text_step("好的，我完成了"), text_step("这次真的写了")]),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "创建 a.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let roles: Vec<Role> = before.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant],
        "前置：提醒回合的消息序列"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        shape(&restored.messages),
        shape(&before.messages),
        "提醒回合重启后顺序必须为 user/assistant/user/assistant"
    );
    assert_eq!(
        restored.messages[2].content, REMINDER_NOTE,
        "第二条 user 是提醒文案"
    );
}

/// 例 4：旧日志缺 `user` 事件 ⇒ 只重建可还原的部分，不 panic、不伪造用户行。
#[test]
fn legacy_log_without_user_events_rebuilds_partial_history() {
    let dir = workdir("legacy");
    // 直接手写旧格式日志：只有 `text` 事件（本卡之前的日志没有 `user` 事件）。
    let sid = Uuid::new_v4();
    let lines = [
        header_line(sid),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"session_update","payload":{{"status":"idle","title":"旧会话","mode":"ask"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"text","payload":{{"text":"旧回复一"}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"text","payload":{{"text":"旧回复二"}},"tsMs":30}}"#
        ),
    ];
    write_log(&dir, sid, &lines);

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("旧会话必须被启动回放认出来");
    assert_eq!(
        shape(&restored.messages),
        vec![
            (Role::Assistant, "旧回复一".to_string(), 20),
            (Role::Assistant, "旧回复二".to_string(), 30),
        ],
        "缺 user 事件的旧日志只重建可还原部分，不伪造用户行"
    );
    assert_eq!(restored.title, "旧会话", "既有标题回放行为不变");
}
