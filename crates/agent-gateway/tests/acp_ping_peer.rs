//! 生产链路：`from_gateway_config` 起的 host 必须能应答白名单动作 `sys:ping`。
//!
//! 背景：`ACP_FORWARD_ACTIONS` 的唯一成员是 `sys:ping`，转发把动作名**原样**
//! 投给对端方法表（`crates/agent-gateway/src/acp.rs::forward_action`）。既有用例
//! 要么手工 `register_method("sys:ping", …)` 塞一个假方法，要么用裸对端验证
//! `-32601`，都没覆盖「**生产入口**起出来的方法表」。本文件走真实链路：
//! 两侧都经 [`AcpHostHandle::from_gateway_config`] 装配（真实 `Host` + 真实方法表，
//! 不手工注册任何方法），网关 `POST /rpc {"action":"sys:ping"}` 必须拿到
//! `200` 且 `data.pid` 为本进程 pid，**不是** `-32601`。
//!
//! 对照用例：对端改用 [`AcpHostHandle::start`]（库层入口，按契约不隐式注册）
//! → 仍回 `-32601`，证明「答得上」来自生产入口的基线注册，而不是转发机制
//! 凭空兜底；同时守住 `start` 的三层默认值不被本卡改动。
//!
//! 传输用内存回环（与 `acp_bridge.rs` 同款）：不 spawn 进程、不监听 0.0.0.0。

use std::sync::Arc;
use std::time::Duration;

use agent_acp::host::HostConfig;
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{AcpHostHandle, GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 上界须大于网关自身 RPC 超时（默认 30s）与转发传播超时（5s）。
const RPC_BOUND: Duration = Duration::from_secs(40);

/// `GatewayConfig` 在本入口只是形态参数（路径/isolation 不被读取）。
fn gw_config(path: &str) -> GatewayConfig {
    GatewayConfig::new(path, Isolation::local_default())
}

fn runtime_in(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

async fn gw_request(port: u16, token: &str, action: &str, payload: Value) -> (u16, Value) {
    tokio::time::timeout(RPC_BOUND, async {
        let body = json!({"action":action,"payload":payload}).to_string();
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

/// 起网关并挂上给定的 ACP handle（复刻 `acp_bridge.rs::start_gateway` 口径）。
async fn start_gateway(acp: AcpHostHandle) -> (GatewayHandle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = gw_config(&dir.path().join("gw.json").to_string_lossy()).with_acp(acp);
    let g = GatewayHandle::start(runtime_in(dir.path()), config)
        .await
        .unwrap();
    (g, dir)
}

/// 内存回环：两端各一条 `Transport`，帧字节按顶层归类互投。
///
/// 由调用方决定各自用哪个入口起 host——这正是本文件要对照的变量
/// （`from_gateway_config` vs `start`）。
fn loopback_transports() -> (
    agent_acp::transport::Transport,
    agent_acp::transport::Transport,
) {
    let (gw_source, gw_source_tx) = agent_acp::transport::event_channel();
    let (peer_source, peer_source_tx) = agent_acp::transport::event_channel();
    let gw_transport = agent_acp::transport::Transport::new(
        Box::new(FrameBridge { tx: peer_source_tx }),
        gw_source,
    );
    let peer_transport = agent_acp::transport::Transport::new(
        Box::new(FrameBridge { tx: gw_source_tx }),
        peer_source,
    );
    (gw_transport, peer_transport)
}

/// 帧汇 → 帧源的直连：把本端写出的帧字节解成顶层归类后转投对端帧源。
struct FrameBridge {
    tx: std::sync::mpsc::Sender<agent_acp::transport::SourceEvent>,
}

impl agent_acp::transport::FrameSink for FrameBridge {
    fn send_frame(&mut self, frame: &[u8]) -> agent_acp::Result<()> {
        let ev = match agent_acp::codec::decode_frame(frame) {
            Ok(top) => agent_acp::transport::SourceEvent::Frame(top),
            Err(e) => agent_acp::transport::SourceEvent::BadFrame {
                reason: e.to_string(),
            },
        };
        self.tx.send(ev).map_err(|_| agent_acp::Error::Transport {
            reason: "回环对端已释放".into(),
        })
    }
}

// ── 1. 真实链路：生产入口起的对端答得上 sys:ping ────────────────────────────

/// 两侧都走**生产入口**（`from_gateway_config`）：对端方法表来自 host 基线，
/// 网关转发 `sys:ping` 必须得到 `200` + `data.pid`，而不是 `-32601`。
///
/// 修复前：生产入口只注册 `initialize` / `logout`，转发必回 `-32601`（本用例红）。
#[tokio::test]
async fn production_entry_answers_allowlisted_ping_via_forwarding() {
    let (gw_transport, peer_transport) = loopback_transports();
    let config = gw_config("/tmp/acp-ping-peer.json");
    let peer = AcpHostHandle::from_gateway_config(&config, peer_transport);
    let gw_acp = AcpHostHandle::from_gateway_config(&config, gw_transport);

    let (g, _dir) = start_gateway(gw_acp).await;

    let (status, resp) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(resp["code"], 0, "转发拿到对端应答即算传送成功");
    assert!(
        resp["data"]["error"].is_null(),
        "生产链路不得再回 ACP -32601：{resp}"
    );
    assert_eq!(
        resp["data"]["pid"],
        json!(std::process::id()),
        "sys:ping 应由生产入口起的 host 基线应答"
    );

    drop(peer);
    g.shutdown().await.unwrap();
}

// ── 2. 对照：start 起的对端不隐式注册 → 仍 -32601 ───────────────────────────

/// 对端用 [`AcpHostHandle::start`]（显式 config 入口）起，按契约**不做隐式注册**：
/// 转发 `sys:ping` 仍拿回 `-32601`。这条守住三层默认值的区分，并反证第 1 条
/// 的应答来自生产入口的基线注册。
#[tokio::test]
async fn start_entry_peer_still_reports_method_not_found_for_ping() {
    let (gw_transport, peer_transport) = loopback_transports();
    let config = gw_config("/tmp/acp-ping-peer-start.json");
    let peer = AcpHostHandle::start(peer_transport, HostConfig::default());
    let gw_acp = AcpHostHandle::from_gateway_config(&config, gw_transport);

    let (g, _dir) = start_gateway(gw_acp).await;

    let (status, resp) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(resp["code"], 0);
    assert_eq!(
        resp["data"]["error"]["code"], -32601,
        "start 不得隐式注册 sys:ping：{resp}"
    );
    assert!(resp["data"]["pid"].is_null());

    drop(peer);
    g.shutdown().await.unwrap();
}
