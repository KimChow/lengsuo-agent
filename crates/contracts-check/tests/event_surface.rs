//! 事件面穷举：`EVENT_KINDS` 里每个 kind 都必须能被 `lengsuo-shell` 的**真实解码路径**
//! 解析成 typed `GatewayEvent`；未登记的 kind 必须被显式拒绝
//! （`EventError::UnknownKind`），不得静默丢弃。
//!
//! 走的是真客户端解码路径：`GatewayClient` → `subscribe_events()`（真实 WS 握手 +
//! `X-Gateway-Token`）→ 逐帧 `EventSubscription::next()`。mock 只负责按线格式推帧，
//! 被测的是壳侧解码器本身。
use contracts_check::EVENT_KINDS;
use lengsuo_shell::gateway::{EventError, EventKind, GatewayClient, GatewayEvent};
use serde_json::json;
use std::time::Duration;

mod support;
use support::{event_frame, start_gateway};

/// 构造一帧 `{seq, session_id, kind, payload, ts_ms}`（`AGENT-KICKOFF.md` §3）。
fn frame(seq: u64, kind: &str) -> String {
    event_frame(
        seq,
        "11111111-1111-1111-1111-111111111111",
        kind,
        &json!({"text": "hi"}).to_string(),
        1_700_000_000_000,
    )
}

/// 常量本身必须与 `AGENT-KICKOFF.md` 第 23 行逐个对上（含 `user`），
/// 挡住「`EVENT_KINDS` 被删项后循环少跑一圈」的静默漏测。
#[test]
fn frozen_event_kind_list_matches_kickoff_line_23() {
    let expected = [
        "user",
        "thinking",
        "text",
        "tool_use",
        "tool_result",
        "approval_request",
        "files",
        "done",
        "session_update",
    ];
    assert_eq!(EVENT_KINDS.len(), 9, "v0 事件面应为 9 个 kind");
    for kind in expected {
        assert!(
            EVENT_KINDS.contains(&kind),
            "冻结事件 kind {kind} 不在 EVENT_KINDS 里"
        );
    }
    for kind in EVENT_KINDS {
        assert!(expected.contains(kind), "常量里的 {kind} 未在协议里登记");
    }
}

#[tokio::test]
async fn every_frozen_kind_decodes_through_shell() {
    // mock WS 端点为每种 kind 推一帧，客户端走真实解码路径逐帧解。
    let frames: Vec<String> = EVENT_KINDS
        .iter()
        .enumerate()
        .map(|(i, kind)| frame(i as u64 + 1, kind))
        .collect();
    let mock = support::ws_mock(frames).await;
    let client = GatewayClient::from_handshake(lengsuo_shell::gateway::GatewayHandshake {
        port: mock.addr().port(),
        pid: std::process::id(),
        token: "loopback-mock".to_owned(),
    })
    .with_timeout(Duration::from_secs(5));
    let mut sub = client.subscribe_events().await.expect("WS 握手成功");

    for (i, kind) in EVENT_KINDS.iter().enumerate() {
        let event = sub
            .next()
            .await
            .unwrap_or_else(|e| panic!("已冻结事件 kind `{kind}` 解码失败: {e}"));
        let expected: EventKind = serde_json::from_value(json!(kind)).unwrap();
        assert_eq!(event.kind, expected, "kind 往返不一致: {kind}");
        assert_eq!(event.seq, i as u64 + 1);
        if *kind == "user" {
            // `user` 是 v0 的第 9 类（见 contracts_check::EVENT_KINDS 文档注释）。
            assert_eq!(event.kind, EventKind::User);
        }
    }
    mock.shutdown().await;
}

#[tokio::test]
async fn unregistered_kind_is_rejected_explicitly() {
    let cases = [
        "warning",
        "task.created",
        "approval_resolved",
        "error",
        "unknown_kind",
    ];
    for kind in cases {
        let mock = support::ws_mock(vec![frame(1, kind)]).await;
        let client = GatewayClient::from_handshake(lengsuo_shell::gateway::GatewayHandshake {
            port: mock.addr().port(),
            pid: std::process::id(),
            token: "loopback-mock".to_owned(),
        })
        .with_timeout(Duration::from_secs(5));
        let mut sub = client.subscribe_events().await.expect("WS 握手成功");
        match sub.next().await {
            Err(EventError::UnknownKind { kind: got }) => assert_eq!(got, kind),
            other => panic!("未登记 kind `{kind}` 必须被 UnknownKind 拒绝，实得 {other:?}"),
        }
        mock.shutdown().await;
    }
}

/// 帧形态缺字段必须报 `Malformed`（不猜语义），且错误文本不携带原始帧内容。
#[tokio::test]
async fn malformed_frames_are_reported_without_echoing_payload() {
    let secret = "super-secret-token-value";
    let missing_kind = json!({
        "seq": 1,
        "session_id": "s",
        "payload": {"token": secret},
        "ts_ms": 1,
    })
    .to_string();
    let not_json = format!("{{\"seq\":1,\"secret\":\"{secret}\"");
    for bad in [missing_kind, not_json] {
        let mock = support::ws_mock(vec![bad]).await;
        let client = GatewayClient::from_handshake(lengsuo_shell::gateway::GatewayHandshake {
            port: mock.addr().port(),
            pid: std::process::id(),
            token: "loopback-mock".to_owned(),
        })
        .with_timeout(Duration::from_secs(5));
        let mut sub = client.subscribe_events().await.expect("WS 握手成功");
        match sub.next().await {
            Err(EventError::Malformed { reason }) => {
                assert!(!reason.contains(secret), "错误摘要泄漏了原始载荷: {reason}");
            }
            other => panic!("非法帧应为 Malformed，实得 {other:?}"),
        }
        mock.shutdown().await;
    }
}

/// 真网关 + 真客户端：整条事件流可解码，`seq` 会话内单调递增。
#[tokio::test]
async fn real_gateway_event_stream_decodes_with_monotonic_seq() {
    let (g, dir) = start_gateway().await;
    let client = GatewayClient::connect(&dir.path().join("gateway.json"))
        .await
        .expect("读握手文件")
        .with_timeout(Duration::from_secs(5));

    let created = client
        .session_create(json!({"title": "contract"}))
        .await
        .expect("session:create");
    let id = created["id"].as_str().unwrap().to_owned();

    let mut sub = client.subscribe_events().await.expect("订阅 /events");
    let sent = client
        .session_send(&id, "hello contract")
        .await
        .expect("session:send");
    assert!(
        sent["lastSeq"].as_u64().unwrap() > 0,
        "发送应落事件: {sent}"
    );

    let mut last = 0u64;
    let mut kinds = Vec::new();
    while !kinds.contains(&EventKind::Done) {
        let event: GatewayEvent = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .expect("有限等待")
            .expect("事件帧可解码");
        assert_eq!(event.session_id, id);
        assert!(event.seq > last, "seq 必须单调递增");
        last = event.seq;
        kinds.push(event.kind);
    }
    assert!(
        kinds.contains(&EventKind::User),
        "回合应以 user 起头: {kinds:?}"
    );

    g.shutdown().await.unwrap();
}
