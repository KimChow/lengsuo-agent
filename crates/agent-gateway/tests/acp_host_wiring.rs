//! 网关生产入口 ↔ host 基线方法的接线契约（W2 · lane proto）。
//!
//! 覆盖 `docs/contracts/acp-host.md` §8.4 的**两层默认值**：
//! - [`AcpHostHandle::from_gateway_config`] 是网关生产入口，默认注册基线：
//!   `initialize` 有正确应答、`logout` 回 `null`；**未注册方法仍回 `-32601`**
//!   （基线不是全能兜底）；
//! - [`AcpHostHandle::start`] 按调用方传入的 [`HostConfig`] 原样起，**不隐式注册**
//!   （`initialize` 仍回 `-32601`）——这条最容易被后人改坏，必须守住；
//! - `initialize` 参数畸形 → `-32602`，不 panic（沿基线语义）。
//!
//! 用 `mock_transport()` 做内存回环（与 `acp_bridge.rs` / `host_role.rs` 同款）：
//! 不 spawn 进程、不监听端口。

use std::time::Duration;

use agent_acp::codec::decode_message;
use agent_acp::host::HostConfig;
use agent_acp::message::{
    Message, RequestId, JSONRPC_ERROR_INVALID_PARAMS, JSONRPC_ERROR_METHOD_NOT_FOUND,
};
use agent_acp::transport::{mock_transport, MockPeer};
use agent_core::schema::Isolation;
use agent_gateway::{AcpHostHandle, GatewayConfig};
use serde_json::Value;

/// 等待上限：只作「等到条件成立」的上界，容纳并行测试的调度抖动。
const WAIT: Duration = Duration::from_secs(30);

/// `GatewayConfig` 在本入口只是形态参数（路径/isolation 不被读取）。
fn config() -> GatewayConfig {
    GatewayConfig::new("/tmp/acp-host-wiring.json", Isolation::local_default())
}

/// 经**生产入口**起 host（`mock_transport` 对端）。
fn production_host() -> (AcpHostHandle, MockPeer) {
    let (transport, peer) = mock_transport();
    (
        AcpHostHandle::from_gateway_config(&config(), transport),
        peer,
    )
}

/// 收一帧出站字节并解成 JSON-RPC 消息。
fn recv_message(peer: &MockPeer) -> Message {
    let bytes = peer.recv_outgoing(WAIT).expect("应有出站帧");
    decode_message(&bytes).expect("出站帧可解码")
}

/// 取一条响应；不是响应即 panic。
fn recv_response(peer: &MockPeer) -> agent_acp::message::JsonRpcResponse {
    let Message::Response(resp) = recv_message(peer) else {
        panic!("host 必须回 JSON-RPC 响应");
    };
    resp
}

// ── 1. 生产入口：initialize 有正确应答 ──────────────────────────────────────

#[test]
fn production_entry_answers_initialize() {
    let (_handle, peer) = production_host();
    peer.feed_line(
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
    );

    let resp = recv_response(&peer);
    assert_eq!(resp.id, RequestId::Number(1), "id 必须逐字回显");
    assert!(
        resp.error.is_none(),
        "initialize 不得回错误: {:?}",
        resp.error
    );
    let result = resp.result.expect("initialize 应有 result");
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .expect("protocolVersion 必须存在且为整数");
    assert_eq!(version, 1, "请求值与配置值取 min，二者均为 1");
    let name = result["agentInfo"]["name"]
        .as_str()
        .expect("agentInfo.name 必须存在");
    assert!(!name.is_empty(), "agentInfo.name 不得为空");
}

// ── 2. 生产入口：同一 host 上 logout 回 null ────────────────────────────────

#[test]
fn production_entry_answers_logout_with_null() {
    let (_handle, peer) = production_host();
    // 先握手再登出：证明两个基线方法注册在**同一个** host 方法表上。
    peer.feed_line(
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
    );
    recv_response(&peer);

    peer.feed_line(br#"{"jsonrpc":"2.0","id":2,"method":"logout"}"#);
    // 看线上原始 JSON：`Option<Value>` 解码会把 `"result":null` 折成 `None`，
    // 故「应答是 null」必须按帧体断言，而不是按解码后的 `result`。
    let bytes = peer.recv_outgoing(WAIT).expect("logout 应有出站帧");
    let body: Value = serde_json::from_slice(&bytes).expect("出站帧可解码");
    assert_eq!(body["id"], 2, "id 必须逐字回显");
    assert!(body.get("error").is_none(), "logout 不得回错误");
    assert_eq!(body["result"], Value::Null, "logout 应答必须是 null");
}

// ── 3. 生产入口：未注册方法仍 -32601（基线不是全能兜底）────────────────────

#[test]
fn production_entry_still_rejects_unregistered_method() {
    let (_handle, peer) = production_host();
    peer.feed_line(br#"{"jsonrpc":"2.0","id":3,"method":"session/new","params":{}}"#);

    let resp = recv_response(&peer);
    assert!(resp.result.is_none(), "未注册方法不得有 result");
    let err = resp.error.expect("未注册方法必须有 error");
    assert_eq!(
        err.code, JSONRPC_ERROR_METHOD_NOT_FOUND,
        "基线不得变成全能兜底"
    );
    assert_eq!(
        err.data,
        Some(Value::from("session/new")),
        "错误应回显方法名"
    );
}

// ── 4. start 不隐式注册：initialize 仍 -32601 ──────────────────────────────

#[test]
fn start_does_not_implicitly_register_baseline() {
    let (transport, peer) = mock_transport();
    let _handle = AcpHostHandle::start(transport, HostConfig::default());

    peer.feed_line(
        br#"{"jsonrpc":"2.0","id":4,"method":"initialize","params":{"protocolVersion":1}}"#,
    );
    let resp = recv_response(&peer);
    assert!(resp.result.is_none(), "start 不得隐式注册 initialize");
    assert_eq!(
        resp.error.expect("initialize 应回 -32601").code,
        JSONRPC_ERROR_METHOD_NOT_FOUND,
        "start 语义是「按调用方配置原样起」"
    );
}

// ── 5. initialize 参数畸形 → -32602，不 panic（接线后错误面不变）───────────

#[test]
fn malformed_initialize_params_yield_invalid_params() {
    let (_handle, peer) = production_host();

    // 缺 protocolVersion。
    peer.feed_line(br#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":{}}"#);
    let resp = recv_response(&peer);
    assert_eq!(resp.id, RequestId::Number(5));
    assert_eq!(
        resp.error.expect("缺 protocolVersion 应回 -32602").code,
        JSONRPC_ERROR_INVALID_PARAMS
    );

    // 类型错：protocolVersion 是字符串。连接仍可用（host 未 panic / 未断连）。
    peer.feed_line(
        br#"{"jsonrpc":"2.0","id":6,"method":"initialize","params":{"protocolVersion":"one"}}"#,
    );
    let resp = recv_response(&peer);
    assert_eq!(resp.id, RequestId::Number(6));
    assert_eq!(
        resp.error.expect("类型错应回 -32602").code,
        JSONRPC_ERROR_INVALID_PARAMS
    );
}
