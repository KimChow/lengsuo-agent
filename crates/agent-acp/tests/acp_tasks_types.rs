//! tasks 四方法 serde 金向量测试（风格对齐 `golden_vectors.rs`）。
//!
//! 方法名 = 27 册 §2「ACP 任务」族斜杠方法全表（tasks/list、tasks/get、
//! tasks/cancel、tasks/result）；字段拼写按 19 册 bundle 实证 camelCase
//! （taskId，同 toolCallId 族）与 27 册 §1 `codebuddy.ai/*` `_meta` 命名空间。
//!
//! 27/19/18 册未给出 tasks 报文逐字段实证，样例按上述命名约定取最小字段
//! 集还原（每方法 1 个正例：JSON 字符串 → 反序列化 → 字段断言 → 再序列化
//! → 语义相等）。

use agent_acp::{
    decode_message, encode_frame, JsonRpcRequest, Message, RequestId, Task, TaskId, TaskStatus,
    TasksCancelParams, TasksCancelResult, TasksGetParams, TasksGetResult, TasksListParams,
    TasksListResult, TasksResultParams, TasksResultResult,
};
use serde_json::{json, Value};

fn round_trip<T>(v: &T)
where
    T: PartialEq + std::fmt::Debug + serde::Serialize + serde::de::DeserializeOwned,
{
    let text = serde_json::to_string(v).expect("序列化");
    let back: T = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(&back, v);
}

/// 金向量核心：样例 JSON 字符串 → 反序列化 → 再序列化 → 与原样例语义相等。
fn golden_sample<T>(sample: &str) -> T
where
    T: serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug,
{
    let parsed: T = serde_json::from_str(sample).expect("样例反序列化");
    let reserialized = serde_json::to_string(&parsed).expect("再序列化");
    let orig: Value = serde_json::from_str(sample).expect("样例 JSON 合法");
    let back: Value = serde_json::from_str(&reserialized).expect("再序列化 JSON 合法");
    assert_eq!(back, orig, "再序列化应与原样例语义相等");
    parsed
}

/// 请求 round-trip 通用化：params 用强类型装进 Value 走完整帧编解码。
fn request_round_trip(method: &str, params: &impl serde::Serialize) {
    let req = Message::Request(JsonRpcRequest::new(
        RequestId::Number(1),
        method,
        serde_json::to_value(params).expect("params 序列化"),
    ));
    let bytes = encode_frame(&req).expect("编码");
    assert_eq!(req, decode_message(&bytes).expect("解码"));
}

#[test]
fn tasks_list_round_trip_and_sample() {
    let params = TasksListParams {
        cursor: Some("page2".into()),
    };
    round_trip(&params);
    request_round_trip(agent_acp::tasks::method::TASKS_LIST, &params);
    // 首页缺省：无 cursor 键。
    round_trip(&TasksListParams { cursor: None });

    // 样例：27 册 §2 `tasks/list`，cursor 翻页风格（入参 cursor）。
    let sample = r#"{"cursor":"page2"}"#;
    let parsed: TasksListParams = golden_sample(sample);
    assert_eq!(parsed.cursor.as_deref(), Some("page2"));

    let result = TasksListResult {
        tasks: vec![
            Task {
                task_id: "task-7".into(),
                status: TaskStatus::InProgress,
            },
            Task {
                task_id: "task-8".into(),
                status: TaskStatus::Completed,
            },
        ],
        next_cursor: Some("page3".into()),
    };
    round_trip(&result);
    // camelCase 断言点：wire 键为 `nextCursor`（非 next_cursor）。
    let wire = serde_json::to_value(&result).expect("结果序列化");
    assert!(wire.get("nextCursor").is_some(), "wire 键应为 nextCursor");
    assert!(wire.get("next_cursor").is_none());
    let parsed: TasksListResult = golden_sample(
        r#"{"tasks":[{"taskId":"task-7","status":"in_progress"},{"taskId":"task-8","status":"completed"}],"nextCursor":"page3"}"#,
    );
    assert_eq!(parsed.tasks.len(), 2);
    assert_eq!(parsed.tasks[0].task_id, "task-7");
    assert_eq!(parsed.tasks[0].status, TaskStatus::InProgress);
}

#[test]
fn tasks_get_round_trip_and_sample() {
    let params = TasksGetParams {
        task_id: "task-7".into(),
    };
    round_trip(&params);
    request_round_trip(agent_acp::tasks::method::TASKS_GET, &params);

    // 样例：27 册 §2 `tasks/get`（taskId 拼写照 19 册 toolCallId 的 camelCase 族）。
    let sample = r#"{"jsonrpc":"2.0","id":2,"method":"tasks/get","params":{"taskId":"task-7"}}"#;
    let Message::Request(req) = decode_message(sample.as_bytes()).expect("样例解码") else {
        panic!("应为 Request");
    };
    assert_eq!(req.method, "tasks/get");
    let parsed: TasksGetParams = req.params_as().expect("params");
    assert_eq!(parsed.task_id, "task-7");

    let result = TasksGetResult {
        task: Task {
            task_id: "task-7".into(),
            status: TaskStatus::InProgress,
        },
    };
    round_trip(&result);
    // 完整应答帧 round-trip + 样例解析（result_as 强类型）。
    let resp = Message::Response(agent_acp::JsonRpcResponse::ok(
        RequestId::Number(2),
        serde_json::to_value(&result).expect("result 序列化"),
    ));
    let bytes = encode_frame(&resp).expect("编码");
    assert_eq!(resp, decode_message(&bytes).expect("解码"));
    let sample =
        r#"{"jsonrpc":"2.0","id":2,"result":{"task":{"taskId":"task-7","status":"in_progress"}}}"#;
    let Message::Response(r) = decode_message(sample.as_bytes()).expect("样例解码") else {
        panic!("应为 Response");
    };
    let parsed: TasksGetResult = r.result_as().expect("result").expect("JSON-RPC 应为 ok");
    // camelCase 断言点：wire 键为 `taskId`。
    assert_eq!(parsed.task.task_id, "task-7");
    assert_eq!(parsed.task.status, TaskStatus::InProgress);
}

#[test]
fn tasks_cancel_round_trip_and_sample() {
    let params = TasksCancelParams {
        task_id: "task-7".into(),
    };
    round_trip(&params);
    request_round_trip(agent_acp::tasks::method::TASKS_CANCEL, &params);

    // 样例：27 册 §2 `tasks/cancel`。
    let sample = r#"{"jsonrpc":"2.0","id":3,"method":"tasks/cancel","params":{"taskId":"task-7"}}"#;
    let Message::Request(req) = decode_message(sample.as_bytes()).expect("样例解码") else {
        panic!("应为 Request");
    };
    assert_eq!(req.method, "tasks/cancel");
    let parsed: TasksCancelParams = req.params_as().expect("params");
    assert_eq!(parsed.task_id, "task-7");

    // 取消受理即成功：结果体为空对象（`{}`）。
    let result = TasksCancelResult {};
    round_trip(&result);
    assert_eq!(serde_json::to_string(&result).expect("序列化"), "{}");
}

#[test]
fn tasks_result_round_trip_and_sample() {
    let params = TasksResultParams {
        task_id: "task-7".into(),
    };
    round_trip(&params);
    request_round_trip(agent_acp::tasks::method::TASKS_RESULT, &params);

    // 样例：27 册 §2 `tasks/result`——任务产物/输出载荷，open 结构 + `_meta`
    // 预留（27 册 §1 结果族 `codebuddy.ai/outcome` 键作命名空间断言点）。
    let sample = r#"{"jsonrpc":"2.0","id":4,"method":"tasks/result","params":{"taskId":"task-7"}}"#;
    let Message::Request(req) = decode_message(sample.as_bytes()).expect("样例解码") else {
        panic!("应为 Request");
    };
    let parsed: TasksResultParams = req.params_as().expect("params");
    assert_eq!(parsed.task_id, "task-7");

    let result = TasksResultResult {
        content: Some(json!({"report": "任务报告正文", "artifacts": ["/tmp/report.md"]})),
        meta: Some(json!({"codebuddy.ai/outcome": "success"})),
    };
    round_trip(&result);
    let wire = serde_json::to_value(&result).expect("结果序列化");
    // `_meta` 命名空间断言点：wire 键 `_meta` 携带 `codebuddy.ai/*` 私有键。
    assert_eq!(
        wire.get("_meta")
            .and_then(|m| m.get("codebuddy.ai/outcome"))
            .and_then(Value::as_str),
        Some("success")
    );
    let parsed: TasksResultResult = golden_sample(
        r#"{"content":{"report":"任务报告正文","artifacts":["/tmp/report.md"]},"_meta":{"codebuddy.ai/outcome":"success"}}"#,
    );
    assert_eq!(
        parsed.content.as_ref().and_then(|c| c.get("report")),
        Some(&json!("任务报告正文"))
    );
}

#[test]
fn task_status_all_variants_round_trip() {
    // 状态机五态全覆盖（含终态 completed/failed/cancelled，19 册 §4/§7 终态实证）。
    let statuses = [
        TaskStatus::Pending,
        TaskStatus::InProgress,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ];
    for s in &statuses {
        round_trip(s);
    }
    let wire = serde_json::to_value(statuses).expect("序列化");
    assert_eq!(
        wire,
        json!(["pending", "in_progress", "completed", "failed", "cancelled"])
    );

    // TaskId 是字符串形态别名（与会话 id 同风格）。
    let id: TaskId = "task-7".into();
    assert_eq!(id, "task-7");
}
