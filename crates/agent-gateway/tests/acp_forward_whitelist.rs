//! `ACP_FORWARD_ACTIONS` 白名单的**决策断言**（W2 · lane proto）。
//!
//! 本文件只把「**哪些 v0 动作可以走 ACP 转发**这个决策写成数据 + 断言」，
//! **不发明** `域:动作` ↔ ACP `session/*` 的方法名映射——转发时动作名**原样**
//! 作为 ACP method（见 `crates/agent-gateway/src/acp.rs::forward_action`）。
//!
//! 覆盖点（对应 `docs/contracts/acp-host.md` §6）：
//! 1. **白名单 ⊆ 冻结动作面**：表里每个名字都必须是 v0 冻结动作的**原名**，
//!    防自创名字（本卡白名单为 `["sys:ping"]`）；
//! 2. **白名单内动作确实走旁路**：挂 `AcpHostHandle` 并注册**同名**方法 → 经
//!    `POST /rpc` 的 `sys:ping` 由对端 ACP 应答，而不是落到既有实现；
//! 3. **白名单外动作不受影响**：`tool:list`（不在白名单）挂 handle 后仍走既有
//!    实现，结果与**不挂 handle** 时逐字节一致；
//! 4. **不挂 handle 时全体动作不变**：遍历 v0 冻结动作面，断言与
//!    `acp_bridge.rs::without_handle_existing_action_path_is_unchanged` 同口径。
//!
//! 传输用内存构造（与 `tests/acp_bridge.rs` 同款）：`mock_transport()` 直连或
//! `loopback_pair()` 内存回环——不 spawn 进程、不监听 0.0.0.0。

use std::sync::Arc;
use std::time::Duration;

use agent_acp::codec::decode_message;
use agent_acp::host::HostConfig;
use agent_acp::message::Message;
use agent_acp::transport::mock_transport;
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::rpc::ACP_FORWARD_ACTIONS;
use agent_gateway::{AcpHostHandle, GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// v0 冻结动作面（11 个）的**手写副本**，真相来源是 `AGENT-KICKOFF.md` §2 与
/// `crates/contracts-check/src/lib.rs::ACTIONS`。
///
/// 为什么手写而不 `use contracts_check::ACTIONS`：`contracts-check` 依赖
/// `agent-gateway`（方向是 `contracts-check → agent-gateway`），反向把它当
/// dev-dependency 会形成**依赖环**；本卡边界**不许动共享区 `Cargo.toml`**，
/// 故这里先手写、由 `frozen_action_surface_is_exactly_11` 守住「删项即红」，
/// 待主理人开基建卡后再改为引用单一来源（已写进卡报告）。
const FROZEN_ACTIONS: &[&str] = &[
    "session:list",
    "session:create",
    "session:get",
    "session:send",
    "session:stop",
    "session:replay",
    "approval:respond",
    "config:get",
    "config:set",
    "tool:list",
    "sys:ping",
];

// ── 1. 白名单 ⊆ 冻结动作面（防自创名字）──────────────────────────────────────

/// 白名单里每个动作都必须是 v0 冻结动作的**原名**：既挡住自创名字，也挡住
/// 借白名单偷偷塞进一个「新动作」——转发只搬运既有动作，不扩张动作面。
#[test]
fn every_allowlisted_action_is_a_frozen_v0_action() {
    for action in ACP_FORWARD_ACTIONS {
        assert!(
            FROZEN_ACTIONS.contains(action),
            "白名单动作 {action} 不在 v0 冻结动作面内——转发不得自创动作名"
        );
    }
}

/// 本卡白名单的**显式形态**：只放 `sys:ping`（无副作用的健康检查，最小可验证
/// 样本）。这条把「本卡只放一个探针动作」的决策钉住：日后想扩白名单，
/// 必须同时改这里的断言，逼出一次显式评审（而不是悄悄多转发一个动作）。
#[test]
fn allowlist_is_the_single_ping_probe() {
    assert_eq!(
        ACP_FORWARD_ACTIONS,
        &["sys:ping"],
        "本卡白名单应只为 sys:ping；扩表需显式改本断言"
    );
}

/// 动作面**不缩水**：这条与 `contracts-check` 的 `ACTIONS.len() == 11` 同口径，
/// 手写副本若被误删一项，本文件其余「遍历全体动作」的用例会静默少跑一圈。
#[test]
fn frozen_action_surface_is_exactly_11() {
    assert_eq!(FROZEN_ACTIONS.len(), 11, "v0 冻结动作面应为 11 个");
    for action in ACP_FORWARD_ACTIONS {
        // 白名单非空时也顺带证明「本副本确实覆盖了白名单用到的名字」。
        assert!(FROZEN_ACTIONS.contains(action));
    }
}

// ── RPC 面脚手架（与 `tests/acp_bridge.rs` 同款，走真实 HTTP 回环）──────────

/// 上界须**大于**网关自身的 RPC 超时（`rpc_timeout_ms`，默认 30s）：旁路在
/// `spawn_blocking` 里等对端应答，对端一直不回时由 5s 传播超时兜底（→ 502），
/// 再交网关 30s 的 RPC 超时上界。若这里卡得更紧会先于网关超时（误导性地报
/// 「RPC 超时」而非真实应答）。
const RPC_BOUND: Duration = Duration::from_secs(40);

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

fn runtime_in(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

/// 起网关，`acp` 侧可选挂 handle（复刻 `acp_bridge.rs::start_gateway` 的装配口径）。
async fn start_gateway(acp: Option<AcpHostHandle>) -> (GatewayHandle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = GatewayConfig::new(dir.path().join("gw.json"), Isolation::local_default());
    if let Some(handle) = acp {
        config = config.with_acp(handle);
    }
    let g = GatewayHandle::start(runtime_in(dir.path()), config)
        .await
        .unwrap();
    (g, dir)
}

// ── 2. 白名单内动作确实走旁路（ACP 应答，而非既有实现）──────────────────────

/// 挂 handle + 对端注册**同名方法** `sys:ping` → `POST /rpc` 的 `sys:ping`
/// 必须由 ACP 对端应答。既有实现回 `{"pid": <本进程 pid>}`；这里对端故意回一个
/// 哨兵 pid，据此断言「走的是 ACP 而非既有路径」。
///
/// 旁路在 `spawn_blocking` 里**同步等应答**（5s 上界），故对端必须**并发**喂回执：
/// 另起一条线程收转发请求帧、按**逐字 id** 立刻回哨兵值——否则请求线程只能等满
/// 超时，返回 502 而非本用例期望的 ACP 应答。
#[tokio::test]
async fn allowlisted_action_is_served_by_acp_forwarding() {
    let (transport, peer) = mock_transport();
    let acp = AcpHostHandle::start(transport, HostConfig::default());
    // 对端（真实 Host 的一侧）用**冻结动作名**注册方法：转发**原样**当 ACP method。
    acp.register_method("sys:ping", |_| Ok(json!({"pid": 424242})));

    // 应答线程：收转发请求 → 校验 method 逐字为动作名 → 按逐字 id 回哨兵 pid。
    let answer = std::thread::spawn(move || {
        let Message::Request(req) = decode_message(
            &peer
                .recv_outgoing(Duration::from_secs(30))
                .expect("应有转发请求帧"),
        )
        .expect("请求可解码") else {
            panic!("转发必须写出 JSON-RPC 请求");
        };
        assert_eq!(req.method, "sys:ping", "冻结动作名必须原样作 ACP method");
        peer.feed_json(&json!({"jsonrpc": "2.0", "id": req.id, "result": {"pid": 424242}}));
    });

    let (g, _dir) = start_gateway(Some(acp)).await;
    let (status, resp) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(status, 200);
    answer.join().expect("应答线程不应 panic");

    assert_eq!(resp["code"], 0, "旁路应答应归入 code=0");
    assert_eq!(
        resp["data"]["pid"], 424242,
        "sys:ping 应由 ACP 对端应答，而非既有实现"
    );
    // 既有实现会回本进程 pid；哨兵值出现即证明没落到既有路径。
    assert_ne!(
        resp["data"]["pid"],
        json!(std::process::id()),
        "不得落到既有 sys:ping 实现"
    );

    g.shutdown().await.unwrap();
}

/// 挂了 handle 但**对端未注册** `sys:ping` → 对端按 ACP 语义回 `-32601`
/// （真实 `Host` 方法表未命中，不静默丢弃）。白名单显式启用后，该动作的语义
/// 即由 ACP 决定：`-32601` 以转发错误信封带回，**不静默落回既有实现**
/// （避免同一次调用在两种实现间切换）。
///
/// 用内存回环 + **真实对端 `Host`**（照抄 `acp_bridge.rs::loopback_pair` 的装配）：
/// 只有真实 host 才会替未注册方法自动回 `-32601`；`mock_transport()` 的裸对端
/// 会静默吞掉请求（因而不适合本用例）。
#[tokio::test]
async fn allowlisted_action_without_peer_method_reports_method_not_found() {
    let (handle, peer_host) = loopback_pair();
    let (g, _dir) = start_gateway(Some(handle)).await;

    let (status, resp) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(status, 200);
    // 已验证语义：白名单命中即走旁路，对端的 `-32601` 以**转发错误信封**
    // `{"error":{…}}` 原样带回（`code=0` 是「转发本身成功送达并拿到应答」），
    // 且**不**落回既有实现——故 data 里是 error 而非 `{"pid": …}`。
    assert_eq!(resp["code"], 0, "转发拿到对端应答即算传送成功");
    assert_eq!(
        resp["data"]["error"]["code"], -32601,
        "对端未注册该方法，应原样带回 ACP -32601"
    );
    assert_eq!(resp["data"]["error"]["data"], "sys:ping");
    assert!(
        resp["data"]["pid"].is_null(),
        "白名单命中后不得静默落回既有 sys:ping 实现"
    );

    drop(peer_host);
    g.shutdown().await.unwrap();
}

/// 内存回环：返回 `(网关侧句柄, 对端真实 Host)`——两侧都是**真实 `Host`**，
/// 帧经 [`FrameBridge`] 归类后互投（与 `acp_bridge.rs` 同款装配，独立副本以
/// 使本文件自足）。
fn loopback_pair() -> (AcpHostHandle, agent_acp::host::Host) {
    let (gw_source, gw_source_tx) = agent_acp::transport::event_channel();
    let (peer_source, peer_source_tx) = agent_acp::transport::event_channel();
    let gw_sink = Box::new(FrameBridge {
        tx: peer_source_tx.clone(),
    });
    let peer_sink = Box::new(FrameBridge { tx: gw_source_tx });
    let gw_transport = agent_acp::transport::Transport::new(gw_sink, gw_source);
    let peer_transport = agent_acp::transport::Transport::new(peer_sink, peer_source);
    (
        AcpHostHandle::start(gw_transport, HostConfig::default()),
        agent_acp::host::Host::new(peer_transport),
    )
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

// ── 3. 白名单外动作不受影响（结果与不挂 handle 一致）────────────────────────

/// `tool:list` 不在白名单：挂 handle 后仍走**既有实现**，结果与不挂 handle 时
/// **逐字节一致**。这条挡住「挂 handle 就静默改道」的回归——启用与否由白名单
/// 显式拍板，白名单外的动作一个都不许变。
///
/// 对端**注册了** `tool:list` 却不应答转发请求：白名单先于转发判定，故请求根本
/// 不该投出去——若真投了，网关会等到传播超时（5s）才返回，使本次调用明显慢于
/// 「不挂 handle」的对照；用耗时对照把「未转发」钉住，比抢 `recv_outgoing` 的
/// 200ms 窗口更稳（那 5s 窗口宽于任何合理的竞争期）。
#[tokio::test]
async fn non_allowlisted_action_is_identical_with_and_without_handle() {
    let (g_off, _dir_off) = start_gateway(None).await;
    let started = std::time::Instant::now();
    let (status_off, off) = gw_request(g_off.port(), g_off.token(), "tool:list", json!({})).await;
    let off_elapsed = started.elapsed();

    let (transport, _peer) = mock_transport();
    let acp = AcpHostHandle::start(transport, HostConfig::default());
    // 即便对端**注册了** `tool:list`，白名单不含它 → 不得被转发。
    acp.register_method("tool:list", |_| Ok(json!([{"name": "should-not-be-used"}])));
    let (g_on, _dir_on) = start_gateway(Some(acp)).await;
    let started = std::time::Instant::now();
    let (status_on, on) = gw_request(g_on.port(), g_on.token(), "tool:list", json!({})).await;
    let on_elapsed = started.elapsed();

    assert_eq!(status_off, status_on);
    assert_eq!(on, off, "白名单外动作挂/不挂 handle 结果必须逐字节一致");
    assert!(
        on_elapsed < Duration::from_secs(3),
        "白名单外动作不得被转发：一旦转发会先等满 5s 传播超时（实测 on={on_elapsed:?}, off={off_elapsed:?}）"
    );

    g_off.shutdown().await.unwrap();
    g_on.shutdown().await.unwrap();
}

// ── 4. 不挂 handle 时全体动作不变（与 acp_bridge.rs 同口径）────────────────

/// 默认 `acp = None`：遍历 v0 冻结动作面，全部仍走既有实现——`sys:ping` 也走
/// 既有路径（旁路完全不生效），未登记动作 404、非法 payload 400，一条都没变。
#[tokio::test]
async fn without_handle_all_frozen_actions_unchanged() {
    let (g, _dir) = start_gateway(None).await;

    // 建会话 → 发送 → 回放（沿用 replay_rpc / acp_bridge 的断言口径）。
    let (_, created) =
        gw_request(g.port(), g.token(), "session:create", json!({"title": "x"})).await;
    assert_eq!(created["code"], 0);
    let id = created["data"]["id"].clone();
    let (_, sent) = gw_request(
        g.port(),
        g.token(),
        "session:send",
        json!({"id": id, "text": "hi"}),
    )
    .await;
    assert_eq!(sent["code"], 0);

    // 全体冻结动作在「不挂 handle」下都不该 404（未登记/未知动作才 404）。
    for action in FROZEN_ACTIONS {
        let (_, resp) = gw_request(g.port(), g.token(), action, json!({"id": id})).await;
        assert_ne!(
            resp["code"], 404,
            "冻结动作 {action} 在未挂 handle 时不得被当作未登记动作"
        );
    }

    // 白名单里的 sys:ping 在未挂 handle 时也走既有实现（本进程 pid）。
    let (_, ping) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(ping["code"], 0);
    assert_eq!(ping["data"]["pid"], json!(std::process::id()));

    // 未登记动作仍 404（旁路默认关闭，不得悄悄改写语义）。
    let (_, unknown) = gw_request(g.port(), g.token(), "not:registered", json!({})).await;
    assert_eq!(unknown["code"], 404);

    // 非法 afterSeq 仍 400。
    let (_, bad) = gw_request(g.port(), g.token(), "session:replay", json!({"id": id})).await;
    assert_eq!(bad["code"], 400);

    g.shutdown().await.unwrap();
}
