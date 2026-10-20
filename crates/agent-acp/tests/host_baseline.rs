//! host 基线方法测试：`initialize` / `logout` 的应答面、版本协商、错误面，
//! 以及「默认不启用」「基线不得变成全能兜底」两条不变量。
//!
//! 全部走内存 mock 传输（不 spawn 进程），与 `host_role.rs` 同一套装配。

use std::time::Duration;

use agent_acp::codec::decode_frame;
use agent_acp::control::TopLevel;
use agent_acp::host::Host;
use agent_acp::host_baseline::{register_baseline, BaselineConfig, BASELINE_METHODS};
use agent_acp::message::{
    JsonRpcResponse, Message, RequestId, JSONRPC_ERROR_INVALID_PARAMS,
    JSONRPC_ERROR_METHOD_NOT_FOUND,
};
use agent_acp::methods::{method, InitializeResult};
use agent_acp::transport::{mock_transport, MockPeer};
use serde_json::{json, Value};

/// 等待上限：与 `host_role.rs` 同口径（并行跑 workspace 时抗调度抖动）。
const WAIT: Duration = Duration::from_secs(30);

fn host_with_baseline(config: &BaselineConfig) -> (Host, MockPeer) {
    let (transport, peer) = mock_transport();
    let host = Host::new(transport);
    register_baseline(&host, config);
    (host, peer)
}

/// 发一条带 id 的请求，返回出站帧原始字节。
fn request_frame(peer: &MockPeer, id: i64, method_name: &str, params: Option<Value>) -> Vec<u8> {
    let mut frame = json!({"jsonrpc": "2.0", "id": id, "method": method_name});
    if let Some(p) = params {
        frame["params"] = p;
    }
    peer.feed_json(&frame);
    peer.recv_outgoing(WAIT).expect("应有出站帧")
}

/// 发一条带 id 的请求并等响应（`params == None` 时帧内不带 `params` 键）。
fn send_request(
    peer: &MockPeer,
    id: i64,
    method_name: &str,
    params: Option<Value>,
) -> JsonRpcResponse {
    let bytes = request_frame(peer, id, method_name, params);
    let TopLevel::Rpc(Message::Response(resp)) = decode_frame(&bytes).expect("出站帧可解码")
    else {
        panic!("请求必须得到 JSON-RPC 响应");
    };
    assert_eq!(resp.id, RequestId::Number(id), "id 必须逐字回显");
    resp
}

/// 发请求并返回**未解码**的出站 JSON。
///
/// `JsonRpcResponse.result` 是 `Option<Value>`，serde 会把 wire 上的
/// `"result": null` 解成 `None`——与「既无 result 也无 error」不可区分。
/// 断言 `logout` 确实回 `null` 必须看原始帧。
fn send_request_raw(peer: &MockPeer, id: i64, method_name: &str, params: Option<Value>) -> Value {
    serde_json::from_slice(&request_frame(peer, id, method_name, params)).expect("出站帧是 JSON")
}

/// 解出成功结果（失败响应直接 panic 并带上 error）。
fn ok_result(resp: JsonRpcResponse) -> Value {
    assert!(resp.error.is_none(), "应为成功响应：{:?}", resp.error);
    resp.result.expect("成功响应必须有 result")
}

fn error_code(resp: JsonRpcResponse) -> i64 {
    assert!(resp.result.is_none(), "错误响应不得带 result");
    resp.error.expect("错误响应必须有 error").code
}

// ── 1. initialize：协商值 + agentInfo ─────────────────────────────────────

#[test]
fn initialize_answers_negotiated_version_and_agent_info() {
    let config = BaselineConfig {
        protocol_version: 2,
        agent_name: "test-host".into(),
    };
    let (host, peer) = host_with_baseline(&config);

    let resp = send_request(
        &peer,
        1,
        method::INITIALIZE,
        Some(json!({"protocolVersion": 2, "clientInfo": {"name": "peer"}})),
    );
    let result: InitializeResult = resp.result_as().expect("结果可解").expect("应为成功响应");
    assert_eq!(result.protocol_version, 2);
    let info = result.agent_info.expect("必须回 agentInfo");
    assert_eq!(info.name, "test-host");
    assert!(info.version.is_some(), "agentInfo.version 应带上本端版本");
    assert!(result.agent_capabilities.is_none(), "不得发明能力字段");
    assert!(result.auth_methods.is_none(), "不得发明认证方式字段");
    assert!(!host.is_closed());
}

// ── 2. initialize 参数畸形 → -32602，不 panic ────────────────────────────

#[test]
fn initialize_with_malformed_params_answers_invalid_params() {
    let (host, peer) = host_with_baseline(&BaselineConfig::default());

    // 缺 protocolVersion。
    let resp = send_request(
        &peer,
        1,
        method::INITIALIZE,
        Some(json!({"clientInfo": {"name": "peer"}})),
    );
    assert_eq!(error_code(resp), JSONRPC_ERROR_INVALID_PARAMS);

    // 类型错（字符串而非整数）。
    let resp = send_request(
        &peer,
        2,
        method::INITIALIZE,
        Some(json!({"protocolVersion": "one"})),
    );
    assert_eq!(error_code(resp), JSONRPC_ERROR_INVALID_PARAMS);

    // 完全没有 params。
    let resp = send_request(&peer, 3, method::INITIALIZE, None);
    assert_eq!(error_code(resp), JSONRPC_ERROR_INVALID_PARAMS);

    // 不 panic、不断连：后续合规请求照常应答。
    let resp = send_request(
        &peer,
        4,
        method::INITIALIZE,
        Some(json!({"protocolVersion": 1})),
    );
    assert!(ok_result(resp).is_object());
    assert!(!host.is_closed());
}

// ── 3. 版本协商：高回落本端，低原样回请求值 ──────────────────────────────

#[test]
fn initialize_negotiates_to_the_lower_version() {
    let config = BaselineConfig {
        protocol_version: 2,
        agent_name: "test-host".into(),
    };
    let (host, peer) = host_with_baseline(&config);

    // 请求高于本端 → 回落本端上限。
    let resp = send_request(
        &peer,
        1,
        method::INITIALIZE,
        Some(json!({"protocolVersion": 7})),
    );
    let result: InitializeResult = resp.result_as().expect("可解").expect("成功");
    assert_eq!(result.protocol_version, 2);

    // 请求低于本端 → 原样回请求值。
    let resp = send_request(
        &peer,
        2,
        method::INITIALIZE,
        Some(json!({"protocolVersion": 1})),
    );
    let result: InitializeResult = resp.result_as().expect("可解").expect("成功");
    assert_eq!(result.protocol_version, 1);

    assert!(!host.is_closed());
}

// ── 4. 基线不得变成全能兜底：其它方法仍 -32601 ───────────────────────────

#[test]
fn other_methods_stay_method_not_found_after_baseline() {
    let (host, peer) = host_with_baseline(&BaselineConfig::default());

    for (id, name) in [(1, method::SESSION_NEW), (2, "no/such")] {
        let resp = send_request(&peer, id, name, Some(json!({"cwd": "/w"})));
        assert_eq!(error_code(resp), JSONRPC_ERROR_METHOD_NOT_FOUND);
    }
    assert!(!host.is_closed());
}

// ── 5. logout：无参数、回 null ───────────────────────────────────────────

#[test]
fn logout_answers_null_without_params() {
    let (host, peer) = host_with_baseline(&BaselineConfig::default());

    for id in [1, 2] {
        let frame = send_request_raw(&peer, id, method::LOGOUT, None);
        assert_eq!(frame["id"], id, "id 必须逐字回显");
        assert!(frame.get("error").is_none(), "logout 不得回 error");
        assert_eq!(frame["result"], Value::Null, "logout 必须回 result: null");
    }
    assert!(!host.is_closed());
}

// ── 6. 默认不启用：未注册基线的 Host 对 initialize/logout 仍 -32601 ──────

#[test]
fn host_without_baseline_still_answers_method_not_found() {
    let (transport, peer) = mock_transport();
    let host = Host::new(transport); // 不调用 register_baseline

    let resp = send_request(
        &peer,
        1,
        method::INITIALIZE,
        Some(json!({"protocolVersion": 1})),
    );
    assert_eq!(error_code(resp), JSONRPC_ERROR_METHOD_NOT_FOUND);

    let resp = send_request(&peer, 2, method::LOGOUT, None);
    assert_eq!(error_code(resp), JSONRPC_ERROR_METHOD_NOT_FOUND);

    assert!(!host.is_closed());
}

// ── 7. 不变量：基线方法名全部取自 method 常量集合 ────────────────────────

#[test]
fn baseline_methods_are_drawn_from_frozen_method_constants() {
    // 不写死数量：只守「基线 ⊆ 方法常量集合」这条关系。
    let frozen = [
        method::INITIALIZE,
        method::AUTHENTICATE,
        method::SESSION_NEW,
        method::SESSION_LOAD,
        method::SESSION_RESUME,
        method::SESSION_PROMPT,
        method::SESSION_CANCEL,
        method::SESSION_UPDATE,
        method::SESSION_SET_MODE,
        method::SESSION_REQUEST_PERMISSION,
        method::LOGOUT,
        method::ELICITATION_CREATE,
        method::ELICITATION_COMPLETE,
    ];
    for name in BASELINE_METHODS {
        assert!(
            frozen.contains(name),
            "基线方法 {name:?} 必须取自 crate::methods::method 常量"
        );
    }
    assert!(BASELINE_METHODS.contains(&method::INITIALIZE));
    assert!(BASELINE_METHODS.contains(&method::LOGOUT));
}
