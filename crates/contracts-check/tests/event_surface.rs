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

/// 常量必须与 `agent-core::schema::EventKind` 逐项对齐（含持久化事件 `user`），
/// 挡住「`EVENT_KINDS` 被删项后循环少跑一圈」的静默漏测。
///
/// 第 23 行枚举句缺席的三类（`approval_resolved` / `error` / `user`）按实现登记，
/// 见 `contracts_check::EVENT_KINDS` 文档注释。
#[test]
fn frozen_event_kind_list_matches_agent_core_event_kind() {
    let expected = [
        "user",
        "thinking",
        "text",
        "tool_use",
        "tool_result",
        "approval_request",
        "approval_resolved",
        "files",
        "done",
        "error",
        "session_update",
    ];
    assert_eq!(
        EVENT_KINDS.len(),
        11,
        "v0 事件面应为 11 项（agent-core 10 变体 + 持久化 user）"
    );
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

/// **契约缺口探针**：`EVENT_KINDS` 里每一个 kind 都必须能被壳侧解码。
///
/// 与 [`every_frozen_kind_decodes_through_shell`] 的区别：本用例**逐 kind 收集失败**
/// 再统一断言，缺口存在时报错文本会一次列出**全部**不可解码的 kind（而不是在第一个
/// 就 panic）。这是本卡暴露「壳侧 `EventKind` 缺 `approval_resolved` / `error`」的红点：
/// 修壳侧前该断言应失败且列出这两类，修完后转绿。**不要把它改成恒真或 ignore**。
#[tokio::test]
async fn all_frozen_kinds_are_decodable_by_shell() {
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

    let mut undecodable = Vec::new();
    for kind in EVENT_KINDS {
        match sub.next().await {
            Ok(event) => assert_eq!(
                event.kind,
                serde_json::from_value::<EventKind>(json!(kind)).unwrap(),
                "kind 往返不一致: {kind}"
            ),
            Err(e) => undecodable.push(format!("{kind} ({e})")),
        }
    }
    assert!(
        undecodable.is_empty(),
        "以下冻结事件 kind 不能走壳侧解码路径（壳侧 EventKind 与 \
         agent-core::schema::EventKind 不一致）：{undecodable:?}"
    );
    mock.shutdown().await;
}

/// 同上，但按「每个 kind 一帧」逐帧独立解码（不依赖前序帧的消费位置），
/// 便于定位到具体哪一类被壳侧拒绝。
#[tokio::test]
async fn each_frozen_kind_decodes_in_isolation() {
    for kind in EVENT_KINDS {
        let mock = support::ws_mock(vec![frame(1, kind)]).await;
        let client = GatewayClient::from_handshake(lengsuo_shell::gateway::GatewayHandshake {
            port: mock.addr().port(),
            pid: std::process::id(),
            token: "loopback-mock".to_owned(),
        })
        .with_timeout(Duration::from_secs(5));
        let mut sub = client.subscribe_events().await.expect("WS 握手成功");
        let got = sub.next().await;
        mock.shutdown().await;
        let event = got
            .unwrap_or_else(|e| panic!("冻结事件 kind `{kind}` 在壳侧解码失败（协议缺口）: {e}"));
        let expected: EventKind = serde_json::from_value(json!(kind)).unwrap();
        assert_eq!(event.kind, expected, "kind 往返不一致: {kind}");
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

/// 未知 kind 必须被显式拒绝（`UnknownKind`），不得静默丢弃。
///
/// **注意**：`approval_resolved` / `error` **已冻结**（见 `EVENT_KINDS`），不属于此列——
/// 它们被拒是壳侧缺口，归 [`all_frozen_kinds_are_decodable_by_shell`] 断言，
/// 不要塞回本用例（否则会把「缺口」误写成「预期行为」）。
#[tokio::test]
async fn unregistered_kind_is_rejected_explicitly() {
    let cases = [
        "definitely_not_a_kind",
        "warning",
        "task.created",
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

/// **端到端**：真网关把 `approval_resolved` / `error` **推上线**，壳侧必须能解出。
///
/// `agent-gateway` `server.rs` 把 `event.kind` 当任意字符串写入帧（不做白名单），
/// 所以这两类冻结 kind 会原样到达线格式；本用例在真网关上向总线推一帧
/// `approval_resolved`、一帧 `error`，用壳侧真实订阅路径（`subscribe_events()`，
/// 含真实 WS 握手 + `X-Gateway-Token`）逐帧解，断言解出的 `kind` 就是推的那类。
///
/// 「网关原样透传」的判据：壳侧从**线帧**解出的 typed `kind` 与推入总线的
/// `agent-core::schema::Event.kind` 字符串一致——中间只经过网关的
/// `json!({"seq":..,"kind":event.kind,..})` 序列化，没有重命名/白名单/改写。
///
/// 期待（本卡交付时）：壳侧 `EventKind` 尚缺这两类 → 帧被 `UnknownKind` 拒，
/// 即本用例当前失败，**缺口真实存在**。修壳侧后转绿。
#[tokio::test]
async fn real_gateway_forwards_approval_resolved_and_error_to_shell() {
    use agent_core::schema::{Event, Isolation};

    for kind in ["approval_resolved", "error"] {
        let (g, rt, dir) = support::start_gateway_with_runtime().await;
        let client = GatewayClient::connect(&dir.path().join("gateway.json"))
            .await
            .expect("读握手文件")
            .with_timeout(Duration::from_secs(5));
        let created = client
            .session_create(json!({"title": "passthrough"}))
            .await
            .expect("session:create");
        let id = created["id"].as_str().unwrap().to_owned();

        let mut sub = client.subscribe_events().await.expect("订阅 /events");
        // 等服务端订阅登记完成，避免推的事件早于订阅建立而丢失。
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 按网关订阅的隔离键推总线（`GatewayConfig::new(.., Isolation::local_default())`，
        // 见 tests/support/mod.rs 的 start_gateway）。
        rt.bus(&Isolation::local_default().key()).publish(Event {
            seq: 1,
            session_id: id.parse().unwrap(),
            kind: kind.to_owned(),
            payload: json!({"probe": true}),
            ts_ms: 1_700_000_000_000,
        });

        let event = tokio::time::timeout(Duration::from_secs(3), sub.next())
            .await
            .expect("有限等待事件帧")
            .unwrap_or_else(|e| {
                panic!("真网关把已冻结 kind `{kind}` 推上线，但壳侧拒绝解码（协议缺口）: {e}")
            });
        assert_eq!(
            event.kind,
            serde_json::from_value::<EventKind>(json!(kind)).unwrap(),
            "网关透传的 kind 与壳侧解出的不一致: {kind}"
        );
        g.shutdown().await.unwrap();
    }
}
