//! 事件重放协议 serde 金向量测试（风格对齐 `golden_vectors.rs`）。
//!
//! 重放语义 = 19 册 §4（先 history 后 live 水合、锚点游标续拉）；
//! 回放安全标记 = 27 册 §1 规则 A27-1 回放族 `_meta` 键
//! （`codebuddy.ai/historyReplay` / `promptReplaySafe` / `isCompactInternal`）。
//!
//! 27/19/18 册未实证重放的 wire 方法名，故不组装 JSON-RPC 方法帧，
//! 直接对 params/result 载荷做金向量（JSON 字符串 → 反序列化 → 字段断言 →
//! 再序列化 → 语义相等）。

use agent_acp::{
    ReplayEvent, ReplayEventsParams, ReplayEventsResult, ReplayPhase, ReplaySafetyMeta,
    SessionUpdate, TextContent,
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

#[test]
fn replay_params_round_trip_and_sample() {
    let params = ReplayEventsParams {
        session_id: "sess-42".into(),
        cursor: Some("evt-100".into()),
        limit: Some(50),
    };
    round_trip(&params);
    // 从头重放：锚点与上限缺省。
    round_trip(&ReplayEventsParams {
        session_id: "sess-42".into(),
        cursor: None,
        limit: None,
    });

    // 样例：锚点 cursor + 上限 limit（camelCase 断言点：wire 键 `sessionId`）。
    let parsed: ReplayEventsParams =
        golden_sample(r#"{"sessionId":"sess-42","cursor":"evt-100","limit":50}"#);
    assert_eq!(parsed.session_id, "sess-42");
    assert_eq!(parsed.cursor.as_deref(), Some("evt-100"));
    assert_eq!(parsed.limit, Some(50));
    let wire = serde_json::to_value(&params).expect("params 序列化");
    assert!(wire.get("sessionId").is_some(), "wire 键应为 sessionId");
    assert!(wire.get("session_id").is_none());
}

#[test]
fn replay_result_history_before_live_sample() {
    // 样例：19 册 §4 重放顺序——history 恒先于 live；nextCursor 为续拉锚点。
    let parsed: ReplayEventsResult = golden_sample(
        r#"{"events":[{"sessionId":"sess-42","update":{"sessionUpdate":"agent_message_chunk","type":"text","text":"历史消息"},"phase":"history","_meta":{"codebuddy.ai/historyReplay":true,"codebuddy.ai/promptReplaySafe":true}},{"sessionId":"sess-42","update":{"sessionUpdate":"tool_call_update","toolCallId":"tc-1","status":"completed"},"phase":"live","_meta":{"codebuddy.ai/promptReplaySafe":true,"codebuddy.ai/isCompactInternal":false}}],"nextCursor":"evt-200"}"#,
    );
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.next_cursor.as_deref(), Some("evt-200"));
    // 顺序断言：第一段 history、第二段 live（19 册 §4 调用顺序）。
    assert_eq!(parsed.events[0].phase, ReplayPhase::History);
    assert_eq!(parsed.events[1].phase, ReplayPhase::Live);
    assert!(matches!(
        parsed.events[0].update,
        SessionUpdate::AgentMessageChunk(_)
    ));
    assert!(matches!(
        parsed.events[1].update,
        SessionUpdate::ToolCallUpdate(_)
    ));
    // 标记断言：历史段带 historyReplay，两段均可安全重放。
    assert_eq!(
        parsed.events[0].meta,
        Some(ReplaySafetyMeta {
            history_replay: Some(true),
            prompt_replay_safe: Some(true),
            is_compact_internal: None,
        })
    );
    assert_eq!(
        parsed.events[1].meta,
        Some(ReplaySafetyMeta {
            history_replay: None,
            prompt_replay_safe: Some(true),
            is_compact_internal: Some(false),
        })
    );

    // camelCase 断言点：wire 键 `nextCursor`（非 next_cursor）。
    let wire = serde_json::to_value(&parsed).expect("结果序列化");
    assert!(wire.get("nextCursor").is_some(), "wire 键应为 nextCursor");
    assert!(wire.get("next_cursor").is_none());
}

#[test]
fn replay_safety_meta_namespaced_keys() {
    // 27 册 §1 回放族全三键：wire 键带 `codebuddy.ai/` 前缀原样命名。
    let sample = r#"{"codebuddy.ai/historyReplay":true,"codebuddy.ai/promptReplaySafe":false,"codebuddy.ai/isCompactInternal":true}"#;
    let parsed: ReplaySafetyMeta = golden_sample(sample);
    assert_eq!(parsed.history_replay, Some(true));
    assert_eq!(parsed.prompt_replay_safe, Some(false));
    assert_eq!(parsed.is_compact_internal, Some(true));

    // 再序列化后三个命名空间键逐一在位（键名逐字断言）。
    let wire = serde_json::to_value(parsed).expect("标记序列化");
    assert_eq!(
        wire.get("codebuddy.ai/historyReplay"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        wire.get("codebuddy.ai/promptReplaySafe"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        wire.get("codebuddy.ai/isCompactInternal"),
        Some(&Value::Bool(true))
    );
    assert_eq!(wire.as_object().expect("对象").len(), 3);

    // 三键全部缺省：空对象 `{}` round-trip（skip 掉 None 键）。
    let empty = ReplaySafetyMeta::default();
    round_trip(&empty);
    assert_eq!(serde_json::to_string(&empty).expect("序列化"), "{}");
}

#[test]
fn replay_event_carries_update_and_marker() {
    // 单事件：update 为 session/update 同形载荷，`_meta` 挂回放安全标记。
    let event = ReplayEvent {
        session_id: "sess-42".into(),
        update: SessionUpdate::AgentMessageChunk(agent_acp::ContentBlock::Text(TextContent::new(
            "压缩后内部恢复的消息",
        ))),
        phase: ReplayPhase::History,
        meta: Some(ReplaySafetyMeta {
            history_replay: Some(true),
            prompt_replay_safe: None,
            is_compact_internal: Some(true),
        }),
    };
    round_trip(&event);
    let wire = serde_json::to_value(&event).expect("事件序列化");
    // 判别式与 camelCase 断言点：sessionUpdate / sessionId。
    assert_eq!(
        wire.get("update").and_then(|u| u.get("sessionUpdate")),
        Some(&json!("agent_message_chunk"))
    );
    assert!(wire.get("sessionId").is_some(), "wire 键应为 sessionId");
    assert_eq!(wire.get("phase"), Some(&json!("history")));

    let result = ReplayEventsResult {
        events: vec![event],
        next_cursor: None,
    };
    round_trip(&result);
    // 无后续锚点：`nextCursor` 键整个缺省（非 null）。
    let wire = serde_json::to_value(&result).expect("结果序列化");
    assert!(wire.get("nextCursor").is_none());
}
