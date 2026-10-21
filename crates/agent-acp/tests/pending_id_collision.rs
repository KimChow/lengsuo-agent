//! 在途请求 id 撞车面（内存 mock 传输，不 spawn 进程）。
//!
//! 覆盖：同 id 重复登记在途槽位 ⇒ **覆盖计数可观测**；不同 id ⇒ **计数不误涨**；
//! 被顶掉的请求方拿到的错误**不得谎报成「超时」**；control 面同样可观测；
//! 未知 id 计数与覆盖计数**互不串扰**；不同 id 的正常回执**乱序仍正确归属**。
//!
//! 语义取舍：同 id 覆盖**是允许的**（与 host 侧同名 handler 覆盖语义一致，
//! 见 `host_role.rs::duplicate_registration_overwrites`），本层**不拒绝**重复 id；
//! 但覆盖必须**可观测**（计数）且**不谎报**（错误文案区分「超时」与「被顶掉」）。

use std::time::{Duration, Instant};

use agent_acp::client::{Client, ClientConfig};
use agent_acp::message::RequestId;
use agent_acp::method;
use agent_acp::transport::{mock_transport, MockPeer};
use agent_acp::ControlSubtype;
use agent_acp::Error;
use serde_json::{json, Value};

/// 轮询等待上限。取 30s 而非紧贴实际耗时：`cargo test --workspace` 并行时会与
/// 数百用例抢 CPU，2s 会被调度抖动顶穿。这里只是「等到条件成立」的上限，
/// 放宽不改变被测语义。
const WAIT: Duration = Duration::from_secs(30);

fn client_with(max_events: usize) -> (Client, MockPeer) {
    let (transport, peer) = mock_transport();
    let client = Client::with_config(
        transport,
        ClientConfig {
            max_events,
            poll_interval: Duration::from_millis(5),
        },
    );
    (client, peer)
}

/// 轮询等待条件成立（路由线程异步推进，断言前必须等到）。
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "等待超时：{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 取出 `Error::Transport` 的 reason（其它变体即失败）。
fn transport_reason(err: Error) -> String {
    match err {
        Error::Transport { reason } => reason,
        other => panic!("应为 Transport 错误，得到 {other:?}"),
    }
}

/// A. 同 id 重复登记 ⇒ 覆盖计数涨到 1（覆盖允许，但必须可观测）。
#[test]
fn same_id_second_request_counts_pending_overwrite() {
    let (client, _peer) = client_with(8);
    assert_eq!(client.pending_overwrite_count(), 0, "初始无覆盖");

    let id = RequestId::String("dup-1".into());
    let _first = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 1");
    assert_eq!(client.pending_overwrite_count(), 0, "首次登记不得算覆盖");

    let _second = client
        .send_request_id(id.clone(), method::SESSION_NEW, json!({}))
        .expect("发送 2");
    assert_eq!(
        client.pending_overwrite_count(),
        1,
        "同 id 二次登记必须计入覆盖计数"
    );
}

/// B. 不同 id 登记 ⇒ 覆盖计数**不涨**（防空扫描：不是「发两次就恒红」）。
#[test]
fn distinct_ids_do_not_count_as_overwrite() {
    let (client, _peer) = client_with(8);
    let _a = client
        .send_request_id(RequestId::Number(11), method::INITIALIZE, json!({}))
        .expect("发送 1");
    let _b = client
        .send_request_id(RequestId::Number(12), method::SESSION_NEW, json!({}))
        .expect("发送 2");
    let _c = client
        .send_request_id(RequestId::String("x-1".into()), method::LOGOUT, Value::Null)
        .expect("发送 3");
    assert_eq!(
        client.pending_overwrite_count(),
        0,
        "不同 id 不得计入覆盖计数"
    );
}

/// C. 被顶掉的请求方拿到的错误**不得谎报成「超时」**（须说真话：顶掉/断开）。
#[test]
fn overwritten_slot_reports_disconnect_not_timeout() {
    let (client, _peer) = client_with(8);
    let id = RequestId::Number(21);
    let first = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 1");
    let _second = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 2");

    let err = first
        .wait_timeout(Duration::from_millis(200))
        .expect_err("槽位被顶掉 ⇒ 旧请求方必须拿到错误");
    let reason = transport_reason(err);
    assert!(
        !reason.contains("等待响应超时"),
        "被顶掉不得谎报成超时：{reason}"
    );
    assert!(
        reason.contains("断开") || reason.contains("顶掉"),
        "错误文案须说真话（断开/顶掉）：{reason}"
    );
}

/// D. control 面同 id 重复登记 ⇒ 覆盖计数同样涨到 1（两张表都修到了）。
#[test]
fn control_same_id_counts_pending_overwrite() {
    let (client, _peer) = client_with(8);
    let id = RequestId::Number(31);
    let _first = client
        .send_control_request(id.clone(), ControlSubtype::TerminalCreate, json!({}))
        .expect("发送 control 1");
    let _second = client
        .send_control_request(id.clone(), ControlSubtype::TerminalCreate, json!({}))
        .expect("发送 control 2");
    assert_eq!(
        client.pending_overwrite_count(),
        1,
        "control 面同 id 登记必须计入覆盖计数"
    );
}

/// E. 未知 id 回执计数与覆盖计数**互不串扰**（一个动，另一个不动）。
#[test]
fn unknown_id_count_and_overwrite_count_are_independent() {
    let (client, peer) = client_with(8);
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": 999, "result": {}}));
    wait_until("未知 id 计数 = 1", || client.unknown_id_count() == 1);
    assert_eq!(
        client.pending_overwrite_count(),
        0,
        "未知 id 回执不得计入覆盖计数"
    );

    // 反向：制造覆盖后，未知 id 计数**不得**跟着涨。
    let id = RequestId::Number(41);
    let _a = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 1");
    let _b = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 2");
    assert_eq!(client.pending_overwrite_count(), 1, "覆盖计数 = 1");
    assert_eq!(client.unknown_id_count(), 1, "覆盖不得计入未知 id 计数");
}

/// F. 不同 id 的回执**乱序**仍各自归属（本卡没破坏正常 id↔回执关联）。
#[test]
fn distinct_ids_keep_their_own_replies_in_any_order() {
    let (client, peer) = client_with(8);
    let h1 = client
        .send_request_id(RequestId::Number(51), method::INITIALIZE, json!({}))
        .expect("发送 1");
    let h2 = client
        .send_request_id(RequestId::Number(52), method::SESSION_NEW, json!({}))
        .expect("发送 2");

    // 逆序回：先 52 后 51。
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": 52, "result": {"sessionId": "s-52"}}));
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": 51, "result": {"protocolVersion": 1}}));

    let r1 = h1.wait_timeout(WAIT).expect("1 未超时").expect("1 成功");
    let r2 = h2.wait_timeout(WAIT).expect("2 未超时").expect("2 成功");
    assert_eq!(r1["protocolVersion"], 1);
    assert_eq!(r2["sessionId"], "s-52");
    assert_eq!(client.pending_overwrite_count(), 0, "正常回执不得算覆盖");
    assert_eq!(client.unknown_id_count(), 0, "乱序不得误判为未知 id");
}

/// G. 覆盖计数进 `Debug` 输出（可观测 ⇒ 得看得见）。
#[test]
fn overwrite_count_is_visible_in_debug_output() {
    let (client, _peer) = client_with(8);
    let id = RequestId::Number(61);
    let _a = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 1");
    let _b = client
        .send_request_id(id.clone(), method::INITIALIZE, json!({}))
        .expect("发送 2");
    let dump = format!("{client:?}");
    assert!(
        dump.contains("pending_overwrites"),
        "Debug 输出须含覆盖计数字段：{dump}"
    );
    assert!(
        dump.contains("unknown_ids"),
        "Debug 输出须含未知 id 计数字段：{dump}"
    );
}
