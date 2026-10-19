//! `session:replay` 协议面：正常回放、增量锚点、limit 钳制、隔离与入参校验。
//!
//! 仅在 Unix 运行：网关起服需写私有握手文件（ACL 保护），Windows 上该 ACL 未实现，
//! `write_handshake` 按 fail-closed 返回 `Unsupported`。
#![cfg(unix)]
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

async fn request(port: u16, token: &str, action: &str, payload: Value) -> (u16, Value) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let body = json!({"action": action, "payload": payload}).to_string();
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let wire = format!("POST /rpc HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Gateway-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        s.write_all(wire.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    })
    .await
    .expect("bounded RPC")
}

fn runtime(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

async fn start(dir: &std::path::Path, rt: Runtime) -> GatewayHandle {
    GatewayHandle::start(
        rt,
        GatewayConfig::new(dir.join("gw.json"), Isolation::local_default()),
    )
    .await
    .unwrap()
}

async fn create_session(g: &GatewayHandle, title: &str) -> Value {
    let (_, v) = request(
        g.port(),
        g.token(),
        "session:create",
        json!({"title": title}),
    )
    .await;
    assert_eq!(v["code"], 0);
    v["data"]["id"].clone()
}

async fn replay(g: &GatewayHandle, payload: Value) -> Value {
    let (_, v) = request(g.port(), g.token(), "session:replay", payload).await;
    v
}

#[tokio::test]
async fn replay_returns_full_history_in_seq_order() {
    let dir = tempfile::tempdir().unwrap();
    let g = start(dir.path(), runtime(dir.path())).await;
    let id = create_session(&g, "replay").await;
    let (_, sent) = request(
        g.port(),
        g.token(),
        "session:send",
        json!({"id": id, "text": "hello"}),
    )
    .await;
    assert_eq!(sent["code"], 0);

    let v = replay(&g, json!({"id": id, "afterSeq": 0})).await;
    assert_eq!(v["code"], 0);
    let events = v["data"]["events"].as_array().unwrap();
    // 建会话自身落 1 条 session_update，发送回合再落若干条；seq 连续，全量回放条数即最大 seq。
    let mut last = 0u64;
    for e in events {
        let seq = e["seq"].as_u64().unwrap();
        assert!(seq > 0, "afterSeq=0 应返回全部事件，seq 从 1 起");
        assert!(seq > last, "事件必须按 seq 升序返回");
        last = seq;
        assert_eq!(e["sessionId"], id);
    }
    assert_eq!(events.len() as u64, last);
    assert_eq!(v["data"]["lastSeq"], last);
    assert_eq!(v["data"]["lastSeq"], sent["data"]["lastSeq"]);

    // 回归锚：未注册动作仍 404（只增不改契约）。
    let (_, unknown) = request(g.port(), g.token(), "not:registered", json!({})).await;
    assert_eq!(unknown["code"], 404);
    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_continues_from_after_seq_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let g = start(dir.path(), runtime(dir.path())).await;
    let id = create_session(&g, "incremental").await;
    let (_, sent) = request(
        g.port(),
        g.token(),
        "session:send",
        json!({"id": id, "text": "hello"}),
    )
    .await;
    assert_eq!(sent["code"], 0);

    let full = replay(&g, json!({"id": id, "afterSeq": 0})).await;
    let events = full["data"]["events"].as_array().unwrap();
    let anchor = events[1]["seq"].as_u64().unwrap();

    let tail = replay(&g, json!({"id": id, "afterSeq": anchor})).await;
    assert_eq!(tail["code"], 0);
    let tail_events = tail["data"]["events"].as_array().unwrap();
    assert_eq!(tail_events.len(), events.len() - 2);
    assert!(tail_events[0]["seq"].as_u64().unwrap() > anchor);
    assert_eq!(tail["data"]["lastSeq"], full["data"]["lastSeq"]);

    // 锚点追平最大 seq 后是空页：lastSeq 回落为 afterSeq，客户端据此终止增量拉取。
    let last = full["data"]["lastSeq"].as_u64().unwrap();
    let empty = replay(&g, json!({"id": id, "afterSeq": last})).await;
    assert_eq!(empty["code"], 0);
    assert_eq!(empty["data"]["events"].as_array().unwrap().len(), 0);
    assert_eq!(empty["data"]["lastSeq"], last);
    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_limit_is_clamped_by_core_page_cap() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(dir.path());
    let session = rt
        .create_session(&agent_core::runtime::CreateSession::plain(
            "clamp",
            Isolation::local_default(),
        ))
        .unwrap();
    // 建会话落 1 条 session_update，echo provider 每次发送落 3 条（thinking/text/done）：
    // 1 + 200×3 = 601 条，超过 core 页上限。
    for i in 0..200 {
        rt.send_text(session.id, &format!("m{i}")).unwrap();
    }
    let g = start(dir.path(), rt).await;

    let v = replay(&g, json!({"id": session.id, "afterSeq": 0, "limit": 10000})).await;
    assert_eq!(v["code"], 0);
    let page = v["data"]["events"].as_array().unwrap();
    assert!(page.len() <= 500, "超过 core 页上限时必须被钳制");
    assert_eq!(page.len(), 500, "601 条事件存在，钳制必须实际生效");
    assert_eq!(v["data"]["lastSeq"], 500);

    // 从上一页尾锚点继续拉，能拿到剩余全部事件。
    let rest = replay(
        &g,
        json!({"id": session.id, "afterSeq": 500, "limit": 10000}),
    )
    .await;
    assert_eq!(rest["code"], 0);
    assert_eq!(rest["data"]["events"].as_array().unwrap().len(), 101);
    assert_eq!(rest["data"]["lastSeq"], 601);
    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_rejects_foreign_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(dir.path());
    let foreign = rt
        .create_session(&agent_core::runtime::CreateSession::plain(
            "foreign",
            Isolation {
                user_id: "other".into(),
                tenant_id: "other".into(),
                env_id: None,
            },
        ))
        .unwrap();
    let g = start(dir.path(), rt).await;
    let v = replay(&g, json!({"id": foreign.id, "afterSeq": 0})).await;
    assert_eq!(v["code"], 403);
    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_rejects_missing_or_invalid_after_seq() {
    let dir = tempfile::tempdir().unwrap();
    let g = start(dir.path(), runtime(dir.path())).await;
    let id = create_session(&g, "validation").await;
    for payload in [
        json!({"id": id}),
        json!({"id": id, "afterSeq": "7"}),
        json!({"id": id, "afterSeq": 18446744073709551615u64}),
    ] {
        let v = replay(&g, payload).await;
        assert_eq!(v["code"], 400, "afterSeq 缺失/非数字/u64::MAX 必须拒收");
    }
    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn replay_of_unknown_session_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let g = start(dir.path(), runtime(dir.path())).await;
    let v = replay(&g, json!({"id": Uuid::new_v4(), "afterSeq": 0})).await;
    assert_eq!(v["code"], 404);
    g.shutdown().await.unwrap();
}
