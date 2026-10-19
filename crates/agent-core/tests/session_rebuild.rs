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
    // 重建的 `ts_ms` 取的是**事件 payload 里的 `tsMs`**（消息时间真值，生产端与消息
    // 本身用同一次时钟读数），而不是事件自身的落盘时刻 `tsMs` 之外的东西：逐条比对
    // payload 真值，证明重建没有拿落盘时刻冒充消息时间。
    let projected_ts = projected_payload_tsms(&dir, before.id);
    assert_eq!(
        shape(&restored.messages)
            .iter()
            .map(|(_, _, ts)| *ts)
            .collect::<Vec<_>>(),
        projected_ts,
        "重建的 ts_ms 必须逐条等于事件 payload.tsMs（消息时间真值）"
    );
}

/// 例 1b：同一会话内**连续两轮** ⇒ 两轮消息的 ts_ms 各自独立（不是同一时刻），
/// 重启后仍逐条相等（防「全部取同一时刻」的偷懒实现）。
#[test]
fn two_turns_keep_independent_timestamps_across_restart() {
    let dir = workdir("two-turns");
    let before = {
        let rt = reopen(&dir, scripted(2));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "第一轮").expect("turn 1");
        rt.send_text(s.id, "第二轮").expect("turn 2");
        rt.get_session(s.id).expect("session")
    };
    assert_eq!(before.messages.len(), 4, "前置：两轮 user/assistant");

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        shape(&restored.messages),
        shape(&before.messages),
        "两轮重启后逐条相等（role / content / ts_ms）"
    );

    // 重建的 ts_ms 逐条等于各自事件 payload 的 tsMs（两轮各取各的真值）。
    let projected_ts = projected_payload_tsms(&dir, before.id);
    assert_eq!(
        shape(&restored.messages)
            .iter()
            .map(|(_, _, ts)| *ts)
            .collect::<Vec<_>>(),
        projected_ts,
        "每一轮的 ts_ms 必须是该轮自己的 payload.tsMs"
    );
    assert_eq!(projected_ts.len(), 4, "两轮共四条可投影事件");
}

/// 例 2：工具回合后重启 ⇒ ToolCall + ToolResult 成对保留，`ToolResult.content`
/// 为 preview、`result_ref` 与重启前一致；`ToolCall.args_ref` 也随 `tool_use`
/// 事件的 `argsRef` 落盘并取回（不再是 None 缺口）。
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
        calls[0].tool.as_ref().and_then(|t| t.args_ref.clone()),
        "args_ref 随 `tool_use` 事件的 `argsRef` 落盘，重启后必须与重启前一致"
    );
    assert!(
        rebuilt_calls[0]
            .tool
            .as_ref()
            .and_then(|t| t.args_ref.clone())
            .is_some(),
        "前置：blob 写入成功时 args_ref 非空（否则上一条断言形同虚设）"
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

/// 例 4b：旧日志 payload **没有** `tsMs` ⇒ 仍能重建成功，`ts_ms` 回退为事件
/// 自身的打点（`Event.ts_ms`），不 panic（守住降级门）。
#[test]
fn legacy_payload_without_tsms_falls_back_to_event_timestamp() {
    let dir = workdir("legacy-tsms");
    // 手写旧格式日志：可投影事件的 payload 里**只有**内容字段，没有 `tsMs`。
    // 事件行自身仍带 `tsMs`（那是事件落盘时刻，本卡未改），回退即取它。
    let sid = Uuid::new_v4();
    let lines = [
        header_line(sid),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"session_update","payload":{{"status":"idle","title":"旧时间戳","mode":"ask"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"user","payload":{{"text":"旧输入"}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"text","payload":{{"text":"旧回复"}},"tsMs":30}}"#
        ),
        format!(
            r#"{{"seq":4,"sessionId":"{sid}","kind":"tool_use","payload":{{"name":"fs.read","args":{{"path":"a.txt"}}}},"tsMs":40}}"#
        ),
        format!(
            r#"{{"seq":5,"sessionId":"{sid}","kind":"tool_result","payload":{{"name":"fs.read","preview":"ok"}},"tsMs":50}}"#
        ),
    ];
    write_log(&dir, sid, &lines);

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("旧会话必须被启动回放认出来");
    assert_eq!(
        shape(&restored.messages),
        vec![
            (Role::User, "旧输入".to_string(), 20),
            (Role::Assistant, "旧回复".to_string(), 30),
            // `summarize_args({"path":"a.txt"})` = "path:string"（只保留键名与值类型）。
            (Role::ToolCall, "path:string".to_string(), 40),
            (Role::ToolResult, "ok".to_string(), 50),
        ],
        "payload 无 tsMs 时回退为事件打点，逐条不 panic"
    );
    // 缺 `argsRef` 的旧事件同样降级为 None，而不是判坏事件。
    assert_eq!(
        tool_of(&restored.messages[2]).args_ref,
        None,
        "旧 tool_use 无 argsRef ⇒ args_ref 为 None"
    );
}
