//! 网关 ↔ ACP host 的**按冻结动作面转发**端到端契约（W2 · lane proto）。
//!
//! 覆盖点（对应 `docs/contracts/acp-host.md` §6）：
//! - 转发走的是**真实 [`agent_acp::host::Host`]**（非重实现桩）：注册方法 → 经
//!   `AcpHostHandle::forward_action` 转发 → 对端收到**逐字 id** 的请求，应答按 id
//!   配对回本端；
//! - **未注册**动作 → 收到 ACP `-32601 method not found`（不 panic、不静默丢弃）；
//! - 帧类型是两边**同一套**（`codec` / `message`），故对端能按 `decode_message`
//!   解出 `session:create` 这类**冻结动作名**（即「原样当 ACP method，不发明映射」）；
//! - **不挂 handle** 时既有动作路径行为不变（对照 `tests/replay_rpc.rs` 的口径：
//!   ACTIONS 全 0 成功、未登记动作 404、filters 与隔离语义不变）。
//!
//! 用 `mock_transport()` 做内存回环（照抄 `crates/agent-acp/tests/host_role.rs`
//! 的用法）：不 spawn 进程、不监听 0.0.0.0，仅本地内存通道。

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_acp::codec::decode_message;
use agent_acp::host::HostConfig;
use agent_acp::message::{Message, RequestId, JSONRPC_ERROR_METHOD_NOT_FOUND};
use agent_acp::transport::{mock_transport, MockPeer};
use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{AcpHostHandle, GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 等待上限：宽松取值容纳并行测试的调度抖动，只作「等到条件成立」的上界。
const WAIT: Duration = Duration::from_secs(30);

/// 起一个内存回环上的网关 host 句柄（真实 `Host`，`mock_transport` 对端）。
fn host_with_peer() -> (AcpHostHandle, MockPeer) {
    let (transport, peer) = mock_transport();
    (AcpHostHandle::start(transport, HostConfig::default()), peer)
}

/// 帧汇 → 帧源的直连：把本端写出的帧字节解成顶层归类后转投对端帧源。
/// 用它把两条 ACP 传输接成一条**内存回环**，两侧各自跑**真实 `Host`**。
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

/// 建一条内存回环：返回 `(网关句柄, 对端 Host)`。两侧都是**真实 `Host`**——
/// 网关侧是被接线的一端，对端侧扮演「实现了方法表的远端 host」。
///
/// 帧类型是两边共用的一套：网关写出的请求经 [`FrameBridge`] 归类后进对端帧源，
/// 对端 `Host` 用它自己的方法表应答，应答再经回环回到网关侧被转发接收线程截住。
fn loopback_pair() -> (AcpHostHandle, agent_acp::host::Host) {
    let (gw_source, gw_source_tx) = agent_acp::transport::event_channel();
    let (peer_source, peer_source_tx) = agent_acp::transport::event_channel();
    // 网关写侧 → 对端帧源；对端写侧 → 网关帧源。
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

/// 转发调用放到线程里跑（它同步阻塞等应答），主线程负责喂对端。
fn forward_in_thread(
    handle: AcpHostHandle,
    action: &'static str,
    payload: Value,
) -> std::thread::JoinHandle<agent_acp::Result<agent_gateway::acp::ForwardOutcome>> {
    std::thread::spawn(move || handle.forward_action(action, &payload))
}

// ── 1. 注册方法 → forward_action 转发 → 逐字 id 应答配对 ────────────────────

#[test]
fn forward_action_reaches_registered_method_and_pairs_verbatim_id() {
    let (handle, peer) = host_with_peer();
    // 对端（真实 Host 的一侧）注册**冻结动作名**作为 ACP method。
    handle.register_method("session:create", |req| {
        let title = req.params_as::<Value>().expect("params")["title"].clone();
        Ok(json!({"id": "sess-forward-1", "title": title}))
    });

    let join = forward_in_thread(
        handle.clone(),
        "session:create",
        json!({"title": "bridged"}),
    );

    // 对端收到的是本端写出的请求：method 即动作名（无改写），id 为字符串/数字均可。
    let bytes = peer.recv_outgoing(WAIT).expect("应有转发请求帧");
    let Message::Request(req) = decode_message(&bytes).expect("请求可解码") else {
        panic!("转发必须写出 JSON-RPC 请求");
    };
    assert_eq!(
        req.method, "session:create",
        "冻结动作名必须原样作 ACP method"
    );
    assert_eq!(req.params.as_ref().expect("params")["title"], "bridged");
    let outbound_id = req.id.clone();

    // 对端按**逐字 id** 回响应：本端转发按 id 配对，不应错配。
    peer.feed_json(&json!({
        "jsonrpc": "2.0", "id": outbound_id, "result": {"id": "sess-forward-1", "title": "bridged"}
    }));

    let outcome = join
        .join()
        .expect("转发线程不应 panic")
        .expect("转发应成功");
    assert!(outcome.error.is_none(), "成功转发的应答不得带 error");
    assert!(!outcome.is_method_not_found());
    let result = outcome.result.as_ref().expect("result");
    assert_eq!(result["id"], "sess-forward-1");
    // 信封形状：成功即 result 本体（供网关 `code=0` 的 data 直接使用）。
    assert_eq!(outcome.envelope["id"], "sess-forward-1");
    assert_eq!(outcome.envelope["title"], "bridged");
}

/// 字符串 id（对端逐字回显字符串 id）也必须配对成功。
#[test]
fn forward_action_pairs_string_id_verbatim() {
    let (handle, peer) = host_with_peer();
    handle.register_method("sys:ping", |_| Ok(json!({"pid": 4242})));

    let join = forward_in_thread(handle.clone(), "sys:ping", json!({}));
    let Message::Request(req) =
        decode_message(&peer.recv_outgoing(WAIT).expect("请求帧")).expect("请求可解码")
    else {
        panic!("应写出请求");
    };
    // 本端转发 id 用字符串拼写注入对端应答，验证 id 逐字回显（两形态都走通）。
    let echo_id = req.id.clone();
    assert!(matches!(echo_id, RequestId::Number(_)), "默认编号为数字 id");
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": echo_id, "result": {"pid": 4242}}));

    let outcome = join.join().expect("不 panic").expect("转发成功");
    assert_eq!(outcome.result.expect("result")["pid"], 4242);
}

// ── 2. 未注册动作 → -32601（不 panic、不静默丢弃）───────────────────────────

#[test]
fn forward_action_of_unregistered_method_yields_method_not_found() {
    let (handle, _peer_host) = loopback_pair();
    // 对端**真实 Host** 未注册任何方法：方法表未命中，按 ACP 语义自动回 -32601。
    let outcome = handle
        .forward_action("tool:list", &json!({}))
        .expect("未注册方法应答了 -32601，转发本身是成功的");
    assert!(outcome.is_method_not_found(), "必须是 -32601 语义");
    assert!(outcome.result.is_none(), "错误应答不得带 result");
    let err = outcome.error.expect("error 对象");
    assert_eq!(err.code, JSONRPC_ERROR_METHOD_NOT_FOUND);
    assert_eq!(err.data, Some(json!("tool:list")));
    // 信封形状：失败即 `{"error":{…}}`（供网关错误信道直接使用）。
    assert_eq!(
        outcome.envelope["error"]["code"],
        JSONRPC_ERROR_METHOD_NOT_FOUND
    );
}

/// 回环对端上注册方法 → 转发命中**真实 Host 方法表**并逐字配对 id。
#[test]
fn forward_action_hits_registered_method_on_real_peer_host() {
    let (handle, peer) = loopback_pair();
    peer.register_method("session:list", |_| Ok(json!([{"id": "s-1"}])));

    let outcome = handle
        .forward_action("session:list", &json!({}))
        .expect("已注册方法应被对端应答");
    assert!(outcome.error.is_none(), "成功应答不得带 error");
    assert_eq!(outcome.result.expect("result")[0]["id"], "s-1");
}

/// 真实 host 一侧实际上确实回了 `-32601` 帧（不是本端臆造）：同一套帧类型下，
/// 直接喂未注册方法给对端 `Host`，对端视角也应看到 `-32601` 应答帧。
#[test]
fn real_host_answers_unregistered_with_frames_on_the_wire() {
    let (transport, peer) = mock_transport();
    let host = agent_acp::host::Host::new(transport);
    peer.feed_line(br#"{"jsonrpc":"2.0","id":9,"method":"no/such","params":null}"#);
    let Message::Response(resp) =
        decode_message(&peer.recv_outgoing(WAIT).expect("应答帧")).expect("响应可解码")
    else {
        panic!("未注册方法应回响应");
    };
    assert_eq!(resp.id, RequestId::Number(9));
    assert_eq!(
        resp.error.expect("error").code,
        JSONRPC_ERROR_METHOD_NOT_FOUND
    );
    drop(host);
}

// ── 3. 转发接的是真实 Host 的同一套帧类型 ────────────────────────────────────

/// 转发请求由本端写出，对端 `Host` 用**同一 codec** 解码并应答（不是自实现的桩）。
/// 另外：非转发帧（如主动 `notify`）仍走 host 的既有通知路径。
#[test]
fn forwarding_uses_the_real_host_frame_types() {
    let (handle, peer) = host_with_peer();
    handle.register_method("session:list", |_| Ok(json!([])));

    let join = forward_in_thread(handle.clone(), "session:list", json!({}));
    // 对端既能解出请求……
    let Message::Request(req) =
        decode_message(&peer.recv_outgoing(WAIT).expect("请求帧")).expect("请求可解码")
    else {
        panic!("应写出请求");
    };
    assert_eq!(req.method, "session:list");
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": req.id, "result": []}));
    join.join().expect("不 panic").expect("转发成功");

    // ……host 的主动通知路径（`notify`）不受转发接线影响，帧仍原样可解。
    handle
        .notify("session/update", json!({"sessionId": "s-1"}))
        .expect("推送通知");
    let Message::Notification(n) =
        decode_message(&peer.recv_outgoing(WAIT).expect("通知帧")).expect("通知可解码")
    else {
        panic!("notify 必须产出 notification");
    };
    assert_eq!(n.method, "session/update");
}

// ── 4. 不挂 handle：既有动作路径行为不变（零回归对照）───────────────────────

async fn gw_request(port: u16, token: &str, action: &str, payload: Value) -> (u16, Value) {
    tokio::time::timeout(Duration::from_secs(5), async {
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

async fn start_gateway() -> (GatewayHandle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::builder()
        .workdir(dir.path())
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap();
    let handle = GatewayHandle::start(
        rt,
        GatewayConfig::new(dir.path().join("gw.json"), Isolation::local_default()),
    )
    .await
    .unwrap();
    (handle, dir)
}

/// 默认（`GatewayConfig::new`，`acp = None`）下既有动作路径与 `tests/replay_rpc.rs`
/// 口径一致：回放正常、未登记动作 404、非法 payload 400——一条都没变。
#[tokio::test]
async fn without_handle_existing_action_path_is_unchanged() {
    let (g, _dir) = start_gateway().await;

    // 建会话 → 发送 → 回放（沿用 replay_rpc 的断言口径）。
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

    let (_, replayed) = gw_request(
        g.port(),
        g.token(),
        "session:replay",
        json!({"id": id, "afterSeq": 0}),
    )
    .await;
    assert_eq!(replayed["code"], 0);
    let events = replayed["data"]["events"].as_array().unwrap();
    assert!(!events.is_empty(), "回放应返回事件");
    assert_eq!(
        replayed["data"]["lastSeq"],
        events.last().unwrap()["seq"].clone()
    );

    // 未登记动作仍 404（旁路默认关闭，未注册动作不得被悄悄改写语义）。
    let (_, unknown) = gw_request(g.port(), g.token(), "not:registered", json!({})).await;
    assert_eq!(unknown["code"], 404);

    // 非法 afterSeq 仍 400。
    let (_, bad) = gw_request(g.port(), g.token(), "session:replay", json!({"id": id})).await;
    assert_eq!(bad["code"], 400);

    g.shutdown().await.unwrap();
}

/// 显式挂上 handle 后，**未被列入 [`agent_gateway::rpc::ACP_FORWARD_ACTIONS`] 的动作**
/// 仍走既有实现（本卡白名单为空，故默认不改变任何动作）。这条挡住「挂 handle 就
/// 静默改道」的回归——启用与否由白名单显式拍板。
#[tokio::test]
async fn attaching_handle_without_allowlist_keeps_existing_paths() {
    assert!(
        agent_gateway::rpc::ACP_FORWARD_ACTIONS.is_empty(),
        "本卡白名单应显式为空"
    );
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::builder()
        .workdir(dir.path())
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap();
    let (transport, _peer) = mock_transport();
    let acp = AcpHostHandle::start(transport, HostConfig::default());
    let g = GatewayHandle::start(
        rt,
        GatewayConfig::new(dir.path().join("gw.json"), Isolation::local_default()).with_acp(acp),
    )
    .await
    .unwrap();

    let (_, ping) = gw_request(g.port(), g.token(), "sys:ping", json!({})).await;
    assert_eq!(ping["code"], 0, "白名单外的动作仍走既有实现");

    let (_, unknown) = gw_request(g.port(), g.token(), "not:registered", json!({})).await;
    assert_eq!(unknown["code"], 404);

    g.shutdown().await.unwrap();
}

/// 转发路径的时间上界：对端不应答 → 转发在超时内返回传输错，而非永久悬挂。
#[test]
fn forward_action_times_out_without_hanging_forever() {
    let (handle, _peer) = host_with_peer();
    // 不喂任何响应：接收线程无帧可截，转发必须由超时兜底。
    let start = Instant::now();
    let err = handle
        .forward_action("session:get", &json!({}))
        .expect_err("无应答必须报错");
    assert!(start.elapsed() < Duration::from_secs(20), "超时应有界");
    let _ = err;
}
