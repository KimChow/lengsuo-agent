//! `Session` / `Message` / `ToolInfo` / `Isolation` 的线上键名面。

use super::registered;
use agent_core::schema::{Isolation, Message, Session, ToolInfo};
use contracts_check::{
    ISOLATION_JSON_KEYS, KICKOFF_SCHEMA_FIELD_NAMES, MESSAGE_JSON_KEYS, SESSION_JSON_KEYS,
    TOOL_INFO_JSON_KEYS,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// 取一个 JSON 对象的键集合。
fn object_keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("被测值必须是 JSON 对象")
        .keys()
        .cloned()
        .collect()
}

/// 构造一个**把所有 `Option` 字段都填上值**的 `Session`。
///
/// 必须填满：`env_binding` / `memory_digest` / `run_status` 都带
/// `skip_serializing_if = "Option::is_none"` ⇒ 键集合随「字段是否为空」变化。
/// 只填一部分会让断言随取值而变（= 又一次「锁取值」违规）。
///
/// 构造方式：用 `serde_json::from_value` 从**线上形态**（camelCase）反序列化
/// ——这一步本身就验证了「线上 camelCase 能被 core 读回」，且不需要 `uuid`
/// 直接依赖（`contracts-check` 的 dev-deps 里没有 `uuid`，本卡不改 `Cargo.toml`）。
fn full_session() -> Session {
    serde_json::from_value(json!({
        "id": "11111111-1111-1111-1111-111111111111",
        "title": "contract",
        "workspaceRoot": "/tmp/contract",
        "mode": "craft",
        "permissionMode": "full",
        "envBinding": "env-1",
        "isolation": {"userId": "u", "tenantId": "t", "envId": "e"},
        "messages": [{
            "role": "tool_call",
            "content": "summary",
            "tool": {
                "name": "fs.read",
                "argsRef": "blob-args-1",
                "resultRef": "blob-result-1",
                "ms": 7
            },
            "tsMs": 1_700_000_000_000u64
        }],
        "createdAtMs": 1_700_000_000_000u64,
        "updatedAtMs": 1_700_000_000_500u64,
        "status": "running",
        "lastSeq": 3,
        "memoryDigest": "digest",
        "runStatus": {
            "runState": "idle",
            "turnSeq": 0,
            "lastTransitionAt": 1_700_000_000_000u64
        }
    }))
    .expect("线上 camelCase 形态的会话应能被 agent-core 读回")
}

/// `Session` 线上 JSON 的**实际键集合**必须与登记的 [`SESSION_JSON_KEYS`] 相等。
///
/// 登记的是**线上**那一份（camelCase）——§3 文档写的是 Rust 侧 snake_case
/// 源字段名，线上不是那一份。
#[test]
fn session_json_keys_match_the_registered_contract() {
    let session = full_session();
    let actual = object_keys(&serde_json::to_value(&session).expect("会话可序列化"));
    let contracted = registered(SESSION_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Session 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `Message` 线上 JSON 的**实际键集合**必须与登记的 [`MESSAGE_JSON_KEYS`] 相等。
#[test]
fn message_json_keys_match_the_registered_contract() {
    let message: Message = serde_json::from_value(json!({
        "role": "tool_call",
        "content": "summary",
        "tool": {
            "name": "fs.read",
            "argsRef": "blob-args-1",
            "resultRef": "blob-result-1",
            "ms": 7
        },
        "tsMs": 1_700_000_000_000u64
    }))
    .expect("线上 camelCase 形态的消息应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&message).expect("消息可序列化"));
    let contracted = registered(MESSAGE_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Message 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `ToolInfo` 线上 JSON 的**实际键集合**必须与登记的 [`TOOL_INFO_JSON_KEYS`] 相等。
#[test]
fn tool_info_json_keys_match_the_registered_contract() {
    let tool: ToolInfo = serde_json::from_value(json!({
        "name": "fs.read",
        "argsRef": "blob-args-1",
        "resultRef": "blob-result-1",
        "ms": 7
    }))
    .expect("线上 camelCase 形态的工具信息应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&tool).expect("工具信息可序列化"));
    let contracted = registered(TOOL_INFO_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "ToolInfo 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `Isolation` 线上 JSON 的**实际键集合**必须与登记的 [`ISOLATION_JSON_KEYS`] 相等。
#[test]
fn isolation_json_keys_match_the_registered_contract() {
    let isolation: Isolation = serde_json::from_value(json!({
        "userId": "u",
        "tenantId": "t",
        "envId": "e"
    }))
    .expect("线上 camelCase 形态的隔离键应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&isolation).expect("隔离键可序列化"));
    let contracted = registered(ISOLATION_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Isolation 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// **反向守卫**：登记的线上键名面**不得**含 §3 文档的 snake_case 源字段名。
///
/// 这条防的是本卡最容易犯的错：把 §3 原文（`workspace_root` / `ts_ms` /
/// `args_ref` …）误当成**线上键名**登记进去。线上是 camelCase 那一份。
///
/// 只比「与线上写法不同」的那些名字（[`KICKOFF_SCHEMA_FIELD_NAMES`]）——
/// `role` / `content` / `name` / `ms` 两侧同名，比了没有意义。
#[test]
fn registered_wire_keys_are_not_the_kickoff_snake_case_field_names() {
    let mut wire = registered(SESSION_JSON_KEYS);
    wire.extend(registered(MESSAGE_JSON_KEYS));
    wire.extend(registered(TOOL_INFO_JSON_KEYS));
    wire.extend(registered(ISOLATION_JSON_KEYS));
    let kickoff = registered(KICKOFF_SCHEMA_FIELD_NAMES);
    let leaked: Vec<&String> = wire.intersection(&kickoff).collect();
    assert!(
        leaked.is_empty(),
        "登记的线上键名面混进了 §3 的 snake_case 源字段名 {leaked:?}——\
         线上是 camelCase 那一份，§3 原文不是线上键名"
    );
}
