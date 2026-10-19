//! host 侧角色测试（内存 mock 传输，不 spawn 进程）。
//!
//! 覆盖六类：方法表应答与 id 逐字回显、未注册方法 → `-32601` 不悬挂、
//! 主动通知可被对端解回、畸形帧计数且连接仍可用、control 面应答、拆除幂等。
//!
//! 客户端侧（反向角色）见 `transport_client.rs` / `control_routing.rs`；
//! 真实子进程见 `stdio_transport.rs`。

use std::time::{Duration, Instant};

use agent_acp::codec::decode_frame;
use agent_acp::control::{ControlSubtype, TopLevel};
use agent_acp::host::{Host, HostConfig, HostEvent, NextEvent};
use agent_acp::message::{JsonRpcErrorObject, Message, RequestId, JSONRPC_ERROR_METHOD_NOT_FOUND};
use agent_acp::transport::{mock_transport, MockPeer};
use serde_json::json;

/// 轮询等待上限。取 30s 而非紧贴实际耗时：`cargo test --workspace` 并行时会与
/// 数百用例抢 CPU，紧上限会被调度抖动顶穿。这里只是「等到条件成立」的上限，
/// 放宽不改变被测语义。
const WAIT: Duration = Duration::from_secs(30);

fn host_with(max_events: usize) -> (Host, MockPeer) {
    let (transport, peer) = mock_transport();
    let host = Host::with_config(
        transport,
        HostConfig {
            max_events,
            ..HostConfig::default()
        },
    );
    (host, peer)
}

/// 轮询等待条件成立（分派线程异步推进，断言前必须等到）。
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "等待超时：{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 收一帧出站字节并解成顶层归类。
fn recv_frame(peer: &MockPeer) -> TopLevel {
    let bytes = peer.recv_outgoing(WAIT).expect("应有出站帧");
    decode_frame(&bytes).expect("出站帧可解码")
}

/// 收一帧出站字节并解成 JSON-RPC 三分类。
fn recv_message(peer: &MockPeer) -> Message {
    let TopLevel::Rpc(m) = recv_frame(peer) else {
        panic!("出站应为 JSON-RPC 帧");
    };
    m
}

// ── 1. 方法表：注册方法 → 回 Response，id 逐字一致 ──────────────────────────

#[test]
fn registered_method_answers_with_verbatim_id() {
    let (host, peer) = host_with(8);
    host.register_method("session/new", |req| {
        let cwd = req.params_as::<serde_json::Value>().expect("params")["cwd"].clone();
        Ok(json!({"sessionId": "sess-1", "cwd": cwd}))
    });

    // 数字 id 与字符串 id 各一：两类 id 都必须逐字回显。
    peer.feed_line(br#"{"jsonrpc":"2.0","id":7,"method":"session/new","params":{"cwd":"/w/a"}}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("已注册方法必须回响应");
    };
    assert_eq!(resp.id, RequestId::Number(7));
    assert_eq!(resp.result.expect("result")["sessionId"], "sess-1");
    assert!(resp.error.is_none(), "成功响应不得带 error");

    peer.feed_line(
        br#"{"jsonrpc":"2.0","id":"req-abc","method":"session/new","params":{"cwd":"/w/b"}}"#,
    );
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("已注册方法必须回响应");
    };
    assert_eq!(resp.id, RequestId::String("req-abc".into()));
    assert_eq!(resp.result.expect("result")["cwd"], "/w/b");
}

/// handler 返回 Err → 响应带该 error 对象（与 client 侧同语义）。
#[test]
fn handler_error_becomes_error_response() {
    let (host, peer) = host_with(8);
    host.register_method("session/prompt", |_| {
        Err(JsonRpcErrorObject {
            code: agent_acp::JSONRPC_ERROR_INVALID_PARAMS,
            message: "sessionId 缺失".into(),
            data: None,
        })
    });
    peer.feed_line(br#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{}}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("应回响应");
    };
    assert_eq!(resp.id, RequestId::Number(1));
    let err = resp.error.expect("error");
    assert_eq!(err.code, agent_acp::JSONRPC_ERROR_INVALID_PARAMS);
    assert!(resp.result.is_none(), "错误响应不得带 result");
}

// ── 2. 未注册方法 → -32601，不 panic ──────────────────────────────────────

#[test]
fn unregistered_method_answers_method_not_found() {
    let (host, peer) = host_with(8);
    peer.feed_line(br#"{"jsonrpc":"2.0","id":"x-1","method":"no/such","params":null}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("未注册方法也必须应答（不得静默丢弃，对端会悬挂）");
    };
    assert_eq!(resp.id, RequestId::String("x-1".into()));
    let err = resp.error.expect("error");
    assert_eq!(err.code, JSONRPC_ERROR_METHOD_NOT_FOUND);
    assert_eq!(err.data, Some(json!("no/such")));
    // 不掉线：后续已注册方法照常应答。
    host.register_method("initialize", |_| Ok(json!({"protocolVersion": 1})));
    peer.feed_line(br#"{"jsonrpc":"2.0","id":2,"method":"initialize"}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("连接应仍可用");
    };
    assert_eq!(resp.id, RequestId::Number(2));
    assert!(!host.is_closed());
}

// ── 3. 主动通知：对端能按 decode_message 解回，method/params 一致 ─────────

#[test]
fn notify_pushes_notification_to_peer() {
    let (host, peer) = host_with(8);
    let params = json!({
        "sessionId": "sess-9",
        "update": {"sessionUpdate": "agent_message_chunk", "type": "text", "text": "hi"}
    });
    host.notify("session/update", params.clone())
        .expect("推送通知");

    let bytes = peer.recv_outgoing(WAIT).expect("应有出站帧");
    // 按最低层解码（对端视角）：帧体即发送时的方向，method/params 原样。
    let decoded = agent_acp::decode_message(&bytes).expect("通知可解回");
    let Message::Notification(n) = decoded else {
        panic!("notify 必须产出 notification（无 id）");
    };
    assert_eq!(n.method, "session/update");
    assert_eq!(n.params, Some(params));
    assert_eq!(
        n.params_as::<serde_json::Value>().expect("params")["sessionId"],
        "sess-9"
    );

    // 通知是单向的：对端不得回帧，host 也不为此挂 pending。
    assert!(peer.recv_outgoing(Duration::from_millis(80)).is_none());
    assert_eq!(host.unexpected_response_count(), 0);
}

// ── 4. 畸形帧：计数增加且连接仍可用 ───────────────────────────────────────

#[test]
fn malformed_frames_count_but_keep_connection_usable() {
    let (host, peer) = host_with(8);
    host.register_method("initialize", |_| Ok(json!("pong")));

    peer.feed_line(br#"{"jsonrpc":"2.0","id":1,"method":"initialize""#); // 截断 JSON
    peer.feed_line(b"not json at all"); // 非 JSON 行
    wait_until("畸形帧计数 = 2", || host.bad_frame_count() == 2);
    assert!(!host.is_closed(), "畸形帧不得断连（字节流仍对齐）");

    // 后续正常帧仍能被处理。
    peer.feed_line(br#"{"jsonrpc":"2.0","id":9,"method":"initialize"}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("畸形帧之后正常帧仍应处理");
    };
    assert_eq!(resp.id, RequestId::Number(9));
    assert_eq!(resp.result.expect("result"), json!("pong"));

    // 字节形态入口（LineDecoder + decode_frame）：同样计数、同样不断连。
    let (host2, _peer2) = host_with(8);
    host2.register_method("initialize", |_| Ok(json!("pong")));
    // 第一行是**完整的行**但 JSON 截断：切行层无残留，归类层计畸形帧。
    host2
        .from_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"init\n")
        .expect("切行不超长");
    host2
        .from_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\"}\n")
        .expect("切行不超长");
    assert_eq!(host2.bad_frame_count(), 1);
    assert!(!host2.is_closed());
}

// ── 5. control 面：ControlRequest → control_response，request_id 一致 ────

#[test]
fn control_request_is_answered_with_matching_request_id() {
    let (host, peer) = host_with(8);
    // 已注册 subtype：分派线程内联应答。
    host.on_control(ControlSubtype::ElicitationCreate, |req| {
        assert_eq!(req.request.subtype, ControlSubtype::ElicitationCreate);
        Ok(json!({"env": "prod"}))
    });
    peer.feed_line(
        br#"{"type":"control_request","request_id":"cr-1","request":{"subtype":"elicitation_create","sessionId":"s-1"}}"#,
    );
    let TopLevel::ControlResponse(resp) = recv_frame(&peer) else {
        panic!("已注册 subtype 应回 control_response");
    };
    assert_eq!(resp.request_id, RequestId::String("cr-1".into()));
    assert_eq!(resp.response["env"], "prod");

    // 未注册 subtype：入事件队列，由上层延后应答（id 同样回显）。
    peer.feed_line(
        br#"{"type":"control_request","request_id":42,"request":{"subtype":"terminal_create"}}"#,
    );
    let NextEvent::Event(HostEvent::ControlRequest(req)) = host.next_event(WAIT) else {
        panic!("未认领 control 请求应入事件队列");
    };
    assert_eq!(req.request_id, RequestId::Number(42));
    host.respond_control(&req.request_id, Ok(json!({"terminalId": "t-1"})))
        .expect("延后应答");
    let TopLevel::ControlResponse(resp) = recv_frame(&peer) else {
        panic!("延后应答应为 control_response 信封");
    };
    assert_eq!(resp.request_id, RequestId::Number(42));
    assert_eq!(resp.response["terminalId"], "t-1");

    // control 通知是单向事件，不入应答路径。
    peer.feed_line(br#"{"type":"control_notification","subtype":"terminal_output","data":"x"}"#);
    let NextEvent::Event(HostEvent::ControlNotification(n)) = host.next_event(WAIT) else {
        panic!("control 通知应入事件队列");
    };
    assert_eq!(n.payload["subtype"], "terminal_output");
}

/// 入站 `session/cancel` 通知（host 侧常见入站通知）入队可观测。
#[test]
fn inbound_notification_is_observable_as_event() {
    let (host, peer) = host_with(8);
    peer.feed_json(&json!({
        "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "s-3"}
    }));
    let NextEvent::Event(HostEvent::Notification(n)) = host.next_event(WAIT) else {
        panic!("入站通知应入事件队列");
    };
    assert_eq!(n.method, "session/cancel");
    assert_eq!(n.params.expect("params")["sessionId"], "s-3");

    // host 不发起请求：对端来的响应只计数 + 可观测，不 panic。
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    let NextEvent::Event(HostEvent::UnexpectedResponse(r)) = host.next_event(WAIT) else {
        panic!("入站响应应可观测");
    };
    assert_eq!(r.id, RequestId::Number(1));
    assert_eq!(host.unexpected_response_count(), 1);
}

// ── 6. 拆除幂等 ──────────────────────────────────────────────────────────

#[test]
fn stop_is_idempotent_and_drop_after_stop_is_safe() {
    let (mut host, peer) = host_with(8);
    host.register_method("initialize", |_| Ok(json!("ok")));
    peer.feed_line(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
    let Message::Response(_) = recv_message(&peer) else {
        panic!("拆除前应能正常应答");
    };

    host.stop();
    host.stop(); // 二次 stop 不得 panic

    // 拆除后写侧关闭：notify 报错而不 panic。
    assert!(host.notify("session/update", json!({})).is_err());

    // Drop 后再显式 stop（作用域结束后由借用检查保证不会真正调用；此处验证
    // 重复 stop 在已拆除状态下的语义与 drop 本身不 panic）。
    let (mut host2, _peer2) = host_with(8);
    host2.stop();
    drop(host2);
    // Drop 路径本身（未显式 stop）也必须干净收尾。
    let (host3, _peer3) = host_with(8);
    drop(host3);
}

/// 对端下毒（Closed）→ 终止可观测，且 stop 仍幂等。
#[test]
fn peer_closed_terminates_and_stop_still_idempotent() {
    let (mut host, peer) = host_with(8);
    peer.feed_event(agent_acp::transport::SourceEvent::Closed {
        reason: Some("对端 EOF".into()),
    });
    wait_until("终止可见", || host.is_closed());
    assert_eq!(host.closed_reason().as_deref(), Some("对端 EOF"));
    assert_eq!(host.next_event(WAIT), NextEvent::Closed);
    host.stop();
    // 共享状态计数不因重复拆除而失真。
    assert_eq!(host.unexpected_response_count(), 0);
    assert_eq!(host.bad_frame_count(), 0);
}

/// 事件队列有界：满丢最旧并计数（与 client 侧同语义）。
#[test]
fn bounded_event_queue_drops_oldest_and_counts() {
    let (host, peer) = host_with(2);
    for i in 1..=5 {
        peer.feed_json(&json!({
            "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": format!("s-{i}")}
        }));
    }
    wait_until("丢最旧计数 = 3", || host.dropped_event_count() == 3);
    let mut kept = Vec::new();
    while let Some(HostEvent::Notification(n)) = host.poll_event() {
        kept.push(
            n.params.expect("params")["sessionId"]
                .as_str()
                .expect("sessionId")
                .to_string(),
        );
    }
    assert_eq!(kept, vec!["s-4".to_string(), "s-5".to_string()]);
}

/// 出站帧字节序：多帧连续写出时逐帧可解（写锁覆盖编码+写，不交错半帧）。
#[test]
fn sequential_frames_stay_decodable() {
    let (host, peer) = host_with(8);
    for i in 1..=4 {
        host.notify("session/update", json!({"seq": i}))
            .expect("推送");
    }
    host.register_method("initialize", |req| Ok(json!({"echo": req.id.display()})));
    peer.feed_line(br#"{"jsonrpc":"2.0","id":"last","method":"initialize"}"#);
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(recv_frame(&peer));
    }
    let methods: Vec<String> = seen
        .iter()
        .map(|top| match top {
            TopLevel::Rpc(Message::Notification(n)) => n.method.clone(),
            TopLevel::Rpc(Message::Response(r)) => r.result.clone().expect("result")["echo"]
                .as_str()
                .expect("echo")
                .to_string(),
            other => panic!("意外帧：{other:?}"),
        })
        .collect();
    assert_eq!(
        methods,
        vec![
            "session/update",
            "session/update",
            "session/update",
            "session/update",
            "last"
        ]
    );
}

/// 重复注册同名方法 = 覆盖（对齐 client 侧 `on_request` 语义）。
#[test]
fn duplicate_registration_overwrites() {
    let (host, peer) = host_with(8);
    host.register_method("initialize", |_| Ok(json!("first")));
    host.register_method("initialize", |_| Ok(json!("second")));
    peer.feed_line(br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
    let Message::Response(resp) = recv_message(&peer) else {
        panic!("应回响应");
    };
    assert_eq!(resp.result.expect("result"), json!("second"));
}
