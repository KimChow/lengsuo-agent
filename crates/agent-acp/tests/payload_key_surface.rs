//! 载荷键面测试：wire 上的**键拼写**是跨进程契约，round-trip 自洽盖不住它。
//!
//! 三类断言：
//! 1. 载荷键快照——样例序列化后的顶层键集合与期望集合逐项相等（camelCase）；
//! 2. 反向解码——按 ACP 客户端实际发送的 camelCase 键能否解回强类型；
//! 3. 类级源码守卫——`methods.rs` 里每个含多词字段的 `pub struct` 都必须声明
//!    `rename_all = "camelCase"`（白名单为零，新增结构体漏 rename 立即红）。

use agent_acp::{
    AuthenticateParams, CancelNotification, InitializeParams, InitializeResult, LoadSessionParams,
    NewSessionParams, NewSessionResult, PermissionOutcome, PermissionOutcomeKind, PromptParams,
    PromptResult, SessionNotification, SessionUpdate, SetSessionModeParams, StopReason,
};
use serde::Serialize;
use serde_json::json;

/// 取序列化结果的顶层键集合（排序后）。
fn top_level_keys<T: Serialize>(value: &T) -> Vec<String> {
    let json = serde_json::to_value(value).expect("序列化");
    let serde_json::Value::Object(map) = json else {
        panic!("载荷应为 JSON 对象，得到 {json}");
    };
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

/// 键集合逐项相等断言（两侧排序，不锁字段出现顺序）。
fn assert_keys<T: Serialize>(value: &T, expected: &[&str]) {
    let mut expected: Vec<String> = expected.iter().map(|k| (*k).to_owned()).collect();
    expected.sort();
    assert_eq!(top_level_keys(value), expected);
}

#[test]
fn payload_keys_are_camel_case() {
    assert_keys(
        &InitializeParams {
            protocol_version: 1,
            client_info: None,
            client_capabilities: None,
        },
        &["protocolVersion"],
    );
    assert_keys(
        &InitializeResult {
            protocol_version: 1,
            agent_info: None,
            agent_capabilities: None,
            auth_methods: None,
        },
        &["protocolVersion"],
    );
    assert_keys(
        &AuthenticateParams {
            method_id: "terminal_auth".into(),
        },
        &["methodId"],
    );
    assert_keys(
        &NewSessionParams {
            cwd: "/home/user/project".into(),
            mcp_servers: None,
        },
        &["cwd"],
    );
    assert_keys(
        &NewSessionResult {
            session_id: "sess-42".into(),
            modes: None,
        },
        &["sessionId"],
    );
    assert_keys(
        &LoadSessionParams {
            session_id: "sess-42".into(),
            cwd: "/home/user/project".into(),
        },
        &["cwd", "sessionId"],
    );
    assert_keys(
        &PromptParams {
            session_id: "sess-42".into(),
            prompt: Vec::new(),
        },
        &["prompt", "sessionId"],
    );
    // 本卡核心：修复前这里产出 ["stop_reason"]。
    assert_keys(
        &PromptResult {
            stop_reason: StopReason::EndTurn,
        },
        &["stopReason"],
    );
    assert_keys(
        &CancelNotification {
            session_id: "sess-42".into(),
        },
        &["sessionId"],
    );
    assert_keys(
        &SetSessionModeParams {
            session_id: "sess-42".into(),
            mode_id: "plan".into(),
        },
        &["modeId", "sessionId"],
    );
    assert_keys(
        &SessionNotification {
            session_id: "sess-42".into(),
            update: SessionUpdate::CurrentMode {
                mode_id: "agent".into(),
            },
        },
        &["sessionId", "update"],
    );
    // `outcome` 为必填字段，故期望集合含它（卡面清单只列了 camelCase 侧键）。
    assert_keys(
        &PermissionOutcome {
            outcome: PermissionOutcomeKind::Selected,
            option_id: Some("allow".into()),
        },
        &["optionId", "outcome"],
    );
}

#[test]
fn prompt_result_decodes_acp_client_payload() {
    // 外部 ACP 客户端发的是 `{"stopReason":"end_turn"}`。
    let parsed: PromptResult = serde_json::from_value(json!({"stopReason": "end_turn"}))
        .expect("camelCase 键必须能解回 PromptResult");
    assert_eq!(
        parsed,
        PromptResult {
            stop_reason: StopReason::EndTurn,
        }
    );
}

/// 类级源码守卫：含下划线字段的结构体必须声明 `rename_all = "camelCase"`。
#[test]
fn multiword_structs_declare_camel_case() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/methods.rs");
    let src = std::fs::read_to_string(&path).expect("读取 methods.rs");
    let lines: Vec<&str> = src.lines().collect();

    let mut multiword = 0usize;
    let mut offenders = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(rest) = lines[i].trim_start().strip_prefix("pub struct ") else {
            i += 1;
            continue;
        };
        let name = rest.split([' ', '{']).next().unwrap_or(rest).to_owned();

        // 属性区：`pub struct` 之前 6 行内。
        let attrs = lines[i.saturating_sub(6)..i].join("\n");
        let declared = attrs.contains("rename_all = \"camelCase\"");

        // 结构体体：本行已闭合（如 `pub struct LogoutParams {}`）则无字段。
        let mut has_underscore_field = false;
        if !lines[i].contains('}') {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim() != "}" {
                if let Some(field) = lines[j].trim().strip_prefix("pub ") {
                    if let Some((field_name, _)) = field.split_once(':') {
                        has_underscore_field |= field_name.trim().contains('_');
                    }
                }
                j += 1;
            }
            i = j;
        }

        if has_underscore_field {
            multiword += 1;
            if !declared {
                offenders.push(name);
            }
        }
        i += 1;
    }

    assert!(multiword > 0, "未扫描到任何多词字段结构体，守卫失效");
    assert!(
        offenders.is_empty(),
        "以下结构体含多词字段却未声明 rename_all = \"camelCase\": {offenders:?}"
    );
}
