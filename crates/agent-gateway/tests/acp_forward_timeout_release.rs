//! 转发**超时后在途槽位必须释放**的回归契约（W2 · lane proto）。
//!
//! 覆盖点（对应 `docs/contracts/acp-host.md` §6.1）：
//! - **超时后可再次转发**：对端不应答 ⇒ 第一次转发超时回传输错；紧接着的第
//!   二次转发必须**真的把帧写出去**（对端 `recv_outgoing` 能收到第二帧、且是
//!   新 id）并按新 id 配对成功，而不是被「已有转发在途」当头拒掉；
//! - **迟到应答不误配**：超时之后对端才把**第一次**的应答喂回来 ⇒ 第二次转发
//!   不得把它当成自己的结果（迟到应答配对不上 ⇒ 不认领）；
//! - **并发保护未被一并拆掉**：一条转发在途时，另一条仍被本地拒（「一次只等
//!   一条」是刻意设计，超时释放不得把它放宽）。
//!
//! 失败分类**可机器判定**：传输错按 reason 归成「超时 / 在途未清 / 其他」三类
//! （见 [`ForwardFailure`]）——断言不满足于「是个错」。
//!
//! 用 `mock_transport()` 做内存回环：不 spawn 进程、不监听端口。

use std::time::{Duration, Instant};

use agent_acp::codec::decode_message;
use agent_acp::host::{HostConfig, HostEvent, NextEvent};
use agent_acp::message::{Message, RequestId};
use agent_acp::transport::{mock_transport, MockPeer};
use agent_gateway::acp::ForwardOutcome;
use agent_gateway::AcpHostHandle;
use serde_json::json;

/// 「等到条件成立」的上界（宽松值容纳并行测试的调度抖动）。
const WAIT: Duration = Duration::from_secs(30);
/// 「转发帧必须已经写进内存通道」的上界。帧写出是**同进程内存投递**（毫秒级），
/// 该上界只用于判定「第二帧**没**写出」这一失败，与超时语义的上界无关。
const FRAME_WAIT: Duration = Duration::from_secs(10);
/// 轮询事件队列的间隔（等迟到应答入队时用）。
const POLL: Duration = Duration::from_secs(1);

/// 转发失败的分类：把传输错按 reason 归类，使三类失败**可机器判定**。
#[derive(Debug, PartialEq, Eq)]
enum ForwardFailure {
    /// 对端未在超时内应答（真超时）。
    Timeout,
    /// 上一条转发仍在途（本地并发保护把本次转发拒掉）。
    Busy,
    /// 其他传输错（写侧失败 / 连接终止 / 句柄已拆除）——保留原始 reason。
    Other(String),
}

impl ForwardFailure {
    /// 把转发错误归类；归不了类的落到 [`ForwardFailure::Other`]（断言会打印它）。
    fn of(err: &agent_acp::Error) -> Self {
        let agent_acp::Error::Transport { reason } = err else {
            return Self::Other(format!("{err}"));
        };
        if reason.contains("未在超时内应答") {
            Self::Timeout
        } else if reason.contains("已有转发在途") {
            Self::Busy
        } else {
            Self::Other(reason.clone())
        }
    }
}

/// 起一个内存回环上的网关 host 句柄（真实 `Host`，`mock_transport` 对端）。
fn host_with_peer() -> (AcpHostHandle, MockPeer) {
    let (transport, peer) = mock_transport();
    (AcpHostHandle::start(transport, HostConfig::default()), peer)
}

/// 取对端收到的一帧转发请求，返回其 id（方法名即冻结动作名）。
fn recv_request(peer: &MockPeer) -> RequestId {
    let bytes = peer.recv_outgoing(WAIT).expect("对端应收到转发请求帧");
    let Message::Request(req) = decode_message(&bytes).expect("请求可解码") else {
        panic!("转发必须写出 JSON-RPC 请求");
    };
    req.id
}

/// 发起一次转发：返回其结果（失败已归类）。
fn forward(handle: &AcpHostHandle, action: &str) -> Result<ForwardOutcome, ForwardFailure> {
    handle
        .forward_action(action, &json!({}))
        .map_err(|e| ForwardFailure::of(&e))
}

/// 对端回一条应答（id 逐字回显）。
fn answer(peer: &MockPeer, id: &RequestId, tag: &str) {
    peer.feed_json(&json!({"jsonrpc": "2.0", "id": id, "result": {"tag": tag}}));
}

/// 在另一条线程上发起转发（转发是同步阻塞等应答的，故放线程里跑）。
fn forward_in_thread(
    handle: AcpHostHandle,
    action: &'static str,
) -> std::thread::JoinHandle<Result<ForwardOutcome, agent_acp::Error>> {
    std::thread::spawn(move || handle.forward_action(action, &json!({})))
}

/// 用例 A（核心）：对端不应答 ⇒ 第一次转发超时；**紧接着的第二次转发必须真的
/// 把帧发出去**（对端能收到第二帧、是新 id）并按新 id 配对成功。
///
/// 修复前：第二次转发在本地就被「已有转发在途」拒掉（压根不写帧）。
#[test]
fn timeout_releases_inflight_slot_so_next_forward_can_start() {
    let (handle, peer) = host_with_peer();

    // 第一次转发：对端不应答 ⇒ 有界超时（真超时，不是「在途未清」）。
    let err = forward(&handle, "session:get").expect_err("对端不应答必须回传输错");
    assert_eq!(
        err,
        ForwardFailure::Timeout,
        "第一次转发必须是「真超时」这一类"
    );
    // 第一帧确实写出去了（对端收下了，只是没回）。
    let first_id = recv_request(&peer);

    // 第二次转发：放线程里跑（它同步等应答），主线程留着对端做应答。
    let join = forward_in_thread(handle.clone(), "session:get");

    // 第二次转发必须**真的写出第二帧**。
    let second_id = match peer.recv_outgoing(FRAME_WAIT) {
        Some(bytes) => {
            let Message::Request(req) = decode_message(&bytes).expect("请求可解码") else {
                panic!("第二帧必须是请求");
            };
            req.id
        }
        None => {
            // 第二帧没写出 ⇒ 第二次转发已被本地拒（打印它的实际失败分类）。
            let outcome = join.join().expect("转发线程不应 panic");
            let failure = match outcome {
                Ok(_) => ForwardFailure::Other("转发竟然成功了但没写出帧".into()),
                Err(e) => ForwardFailure::of(&e),
            };
            panic!("第二次转发必须真的把帧写出去（对端未收到第二帧）；实际失败分类：{failure:?}");
        }
    };
    assert_ne!(
        second_id, first_id,
        "第二次转发必须是新 id（不得复用上一条的在途 id）"
    );

    // 对端这回应答 ⇒ 第二次转发必须成功（帧已写出 + 应答按 id 配对）。
    answer(&peer, &second_id, "second");
    let outcome = join.join().expect("转发线程不应 panic");
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => panic!(
            "第二次转发必须成功，实际失败分类：{:?}",
            ForwardFailure::of(&e)
        ),
    };
    assert_eq!(
        outcome.result.expect("result")["tag"],
        "second",
        "第二次转发必须拿到对端对第二帧的应答"
    );
}

/// 用例 B（防回归）：超时后**迟到的应答**不得被误配给下一次转发。
///
/// 对端先不回 ⇒ 本端超时 ⇒ 本端再发一次 ⇒ 这时对端把**第一次**的应答喂回来
/// ⇒ 第二次转发不得把这条迟到应答当成自己的结果。
#[test]
fn late_response_after_timeout_is_not_mispaired_to_next_forward() {
    let (handle, peer) = host_with_peer();

    // 第一次转发：不应答 ⇒ 超时。
    let err = forward(&handle, "session:get").expect_err("对端不应答必须回传输错");
    assert_eq!(err, ForwardFailure::Timeout, "第一次必须是真超时");
    let first_id = recv_request(&peer);

    // 第二次转发（放线程里跑；主线程留着对端喂应答）。
    let join = forward_in_thread(handle.clone(), "session:get");
    let second_id = match peer.recv_outgoing(FRAME_WAIT) {
        Some(bytes) => {
            let Message::Request(req) = decode_message(&bytes).expect("请求可解码") else {
                panic!("第二帧必须是请求");
            };
            req.id
        }
        None => {
            let outcome = join.join().expect("转发线程不应 panic");
            let failure = match outcome {
                Ok(_) => ForwardFailure::Other("转发竟然成功了但没写出帧".into()),
                Err(e) => ForwardFailure::of(&e),
            };
            panic!("第二次转发必须真的把帧写出去（对端未收到第二帧）；实际失败分类：{failure:?}");
        }
    };
    assert_ne!(second_id, first_id, "第二次转发必须是新 id");

    // 迟到应答（id = 第一次那条）：配不上第二次的在途 id ⇒ 不得被认领。
    answer(&peer, &first_id, "late");
    // 第二次的正经应答。
    answer(&peer, &second_id, "second");

    let outcome = join.join().expect("转发线程不应 panic");
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => panic!(
            "第二次转发必须成功，实际失败分类：{:?}",
            ForwardFailure::of(&e)
        ),
    };
    assert_eq!(
        outcome.result.expect("result")["tag"],
        "second",
        "迟到应答不得被第二次转发当成自己的结果"
    );

    // 迟到应答配对不上 ⇒ 交回 host：计入 `unexpected_responses` 并入事件队列
    // （可观测，不静默吞掉）。此处等它入队，验「它没被第二次转发消费」。
    let deadline = Instant::now() + WAIT;
    loop {
        match handle.next_event(POLL) {
            NextEvent::Event(HostEvent::UnexpectedResponse(resp)) => {
                assert_eq!(
                    resp.id, first_id,
                    "入队的迟到应答必须是第一次那条（id 逐字）"
                );
                break;
            }
            NextEvent::Event(_other) => {}
            NextEvent::Idle | NextEvent::Closed => {}
        }
        if Instant::now() >= deadline {
            panic!("迟迟等不到迟到应答的 unexpected response 事件");
        }
    }
}

/// 用例 C（守住并发保护）：一条转发在途时，另一条必须仍被本地拒——本卡的修复
/// 只放宽**超时之后**的再次发起，不得把「一次只等一条」这条并发保护拆掉。
#[test]
fn concurrent_forward_is_still_rejected_while_one_is_in_flight() {
    let (handle, peer) = host_with_peer();

    let join = forward_in_thread(handle.clone(), "session:get");
    let first_id = recv_request(&peer); // 第一条在途（已写出、未被应答）

    // 第二条：在途未清 ⇒ 立刻被拒。
    let err = forward(&handle, "session:get").expect_err("并发转发必须被本地拒");
    assert_eq!(
        err,
        ForwardFailure::Busy,
        "在途期间必须报「已有转发在途」这一类"
    );

    // 第一条照常被应答（并发保护不该把在途的那条也误伤）。
    answer(&peer, &first_id, "first");
    let outcome = join.join().expect("转发线程不应 panic");
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => panic!(
            "在途的那条转发必须成功，实际失败分类：{:?}",
            ForwardFailure::of(&e)
        ),
    };
    assert_eq!(outcome.result.expect("result")["tag"], "first");
}
