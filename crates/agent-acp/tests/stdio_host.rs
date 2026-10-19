//! host 侧 stdio 装配的端到端验收：`host_stdio_transport()` 真机往返。
//!
//! 被测函数托管**当前进程**的 stdin/stdout，而测试进程自身的 stdin/stdout 归
//! libtest 所有（直接占用会吃掉测试框架输出），故用 `current_exe()` **自我重入**：
//! 父进程（libtest 主进程）以「对端客户端」身份 spawn 同一个测试二进制、只跑
//! 子进程模式的用例；子进程则用 `host_stdio_transport()` 接住自己的 stdin/stdout
//! 建 [`Host`]，按方法表应答。二者构成一次真实的双进程 ACP 往返。
//!
//! 覆盖：`initialize` 往返、id 逐字回显、`notify` 主动推送可达对端、host 收到
//! 入站通知、EOF/子进程退出后 host 有界终止（不 hang）、子进程被回收（不留僵尸）。
//!
//! 全线 Unix：用例依赖真实管道与 POSIX 进程语义。所有等待都带超时，无无限阻塞。

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use agent_acp::client::{Client, ClientConfig};
use agent_acp::host::Host;
use agent_acp::message::RequestId;
use agent_acp::method;
use agent_acp::transport::{event_channel, FrameSink, SourceEvent, Teardown, Transport};
use agent_acp::HostConfig;
use serde_json::{json, Value};

/// 环境变量：置位即表示「本进程是被父测试 spawn 出来的 host 端」。
const CHILD_ENV: &str = "ACP_STDIO_HOST_CHILD";
/// 子进程模式用例名（父进程按 `--exact` 精确重入它）。
const CHILD_TEST: &str = "host_stdio_child_entry";

/// 上下限：30s 是「等到条件成立」的上限而非性能预算（全量测试并行时子进程调度
/// 会抖动）；`BOUNDED` 用于断言拆除「确实很快返回」，避免把有界性写成虚断言。
const WAIT: Duration = Duration::from_secs(30);
const BOUNDED: Duration = Duration::from_secs(10);

/// 1. host 侧 stdio 装配的完整往返（父进程 = 对端客户端）。
#[test]
fn host_stdio_round_trip_with_real_child() {
    let (mut child, client) = connect_to_child();

    // initialize：子进程在方法表里注册了它，须逐字回 id 并回 result。
    let handle = client
        .send_request(method::INITIALIZE, json!({"protocolVersion": 1}))
        .expect("发送 initialize");
    assert_eq!(handle.id, RequestId::Number(1));
    let outcome = handle.wait_timeout(WAIT).expect("未超时").expect("成功");
    assert_eq!(outcome["protocolVersion"], 1);
    assert_eq!(outcome["agentInfo"]["name"], "host-child");

    // 字符串 id 也必须逐字回显（两类 id 各验一次）。
    let handle = client
        .send_request_id(
            RequestId::String("req-abc".into()),
            method::LOGOUT,
            Value::Null,
        )
        .expect("发送 logout");
    let outcome = handle.wait_timeout(WAIT).expect("未超时").expect("成功");
    assert_eq!(outcome["ok"], true);

    // 未注册方法：host 自动回 -32601 而非静默丢弃（对端不会悬挂）。
    let handle = client
        .send_request("no/such", Value::Null)
        .expect("发送未知方法");
    let outcome = handle.wait_timeout(WAIT).expect("未超时");
    assert!(outcome.is_err(), "未注册方法必须回 error，得到 {outcome:?}");

    // host 主动推送的 `session/update`（本端存在理由）应作为事件到达客户端。
    let deadline = Instant::now() + WAIT;
    let mut pushed = false;
    while !pushed {
        match client.next_event(Duration::from_millis(100)) {
            agent_acp::client::NextEvent::Event(agent_acp::router::ClientEvent::Notification(
                n,
            )) => {
                assert_eq!(n.method, "session/update");
                assert_eq!(n.params.expect("params")["sessionId"], "sess-1");
                pushed = true;
            }
            agent_acp::client::NextEvent::Idle => {}
            agent_acp::client::NextEvent::Closed => panic!("推送前连接已终止"),
            other => panic!("意外事件 {other:?}"),
        }
        assert!(Instant::now() < deadline, "应收到 host 主动推送");
    }

    drop(client); // 关写侧 → 子进程 stdin 读到 EOF → host 有界收尾退出。
    let status = wait_child_bounded(&mut child, BOUNDED);
    assert!(status.success(), "子进程应干净退出，状态 {status:?}");
}

/// 2. 对端（父进程）先退：子进程 host 须在 EOF 后有界终止，且进程被回收。
#[test]
fn host_terminates_bounded_on_peer_eof() {
    let (mut child, client, _drain) = connect_to_child_holding_stdout();
    let started = Instant::now();
    drop(client); // 关写侧 → 子进程读到 stdin EOF（stdout 仍被父进程读到 EOF 为止）。
    let status = wait_child_bounded(&mut child, BOUNDED);
    assert!(status.success(), "EOF 后 host 应有界退出，状态 {status:?}");
    assert!(
        started.elapsed() < BOUNDED * 2,
        "EOF 后收尾不得拖到超过界，实际 {:?}",
        started.elapsed()
    );
}

/// 3. 子进程被强杀（模拟对端崩溃）：父进程不得留僵尸，测试可回收。
#[test]
fn host_child_is_reaped_on_kill() {
    let (mut child, _client, _drain) = connect_to_child_holding_stdout();
    child.kill().expect("kill 子进程");
    let status = wait_child_bounded(&mut child, BOUNDED);
    assert!(!status.success(), "被 kill 的子进程不应报告成功退出");
}

// ── 子进程模式 ─────────────────────────────────────────────────────────────

/// 子进程入口：接住自己的 stdin/stdout 建 [`Host`]，按方法表应答并推一条通知。
///
/// 由父进程 `--exact` 重入；直接跑 libtest 时立即返回（不占住测试框架的 stdio）。
#[test]
fn host_stdio_child_entry() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return; // 常规 `cargo test` 下只是个空用例。
    }
    let transport = agent_acp::stdio::host_stdio_transport().expect("host 侧 stdio 装配");
    let host = Host::with_config(
        transport,
        HostConfig {
            max_events: 16,
            ..HostConfig::default()
        },
    );
    host.register_method(method::INITIALIZE, |_| {
        Ok(json!({"protocolVersion": 1, "agentInfo": {"name": "host-child"}}))
    });
    host.register_method(method::LOGOUT, |_| Ok(json!({"ok": true})));

    // 主动推一条通知：用它证明「帧汇 → 本进程 stdout → 对端」真实可用。
    host.notify(
        "session/update",
        json!({"sessionId": "sess-1", "update": {"sessionUpdate": "agent_message_chunk"}}),
    )
    .expect("推送通知");

    // 等对端关管道：EOF 会让 host 终止（帧源关闭）。超时兜底，绝不无限等待。
    let deadline = Instant::now() + WAIT;
    while !host.is_closed() {
        assert!(Instant::now() < deadline, "子进程 host 等待关闭超时");
        let _ = host.next_event(Duration::from_millis(5));
    }
    // 正常返回 → 进程退出码 0；Host 的 Drop 走有界拆除（读者线程 detach）。
}

// ── 测试夹具 ───────────────────────────────────────────────────────────────

/// 自我重入：spawn 同一个测试二进制跑子进程用例，stdin/stdout 接成管道；再把它
/// 的管道接成一个对端 `Client`。返回 `(Child, Client)`——`Child` 保留用于有界
/// 回收，管道已被 `Client` 接管。
fn connect_to_child() -> (Child, Client) {
    let (child, client, drain) = connect_to_child_holding_stdout();
    // 用到的用例都会消费子进程 stdout 直到 EOF；drain 线程只是兜底读，不阻塞退出。
    let _ = drain;
    (child, client)
}

/// 同 [`connect_to_child`]，但额外返回读到子进程 stdout EOF 的加入句柄。
///
/// 「对端先退」类用例（EOF / kill）会**主动丢弃 `Client`**，此时若让 `Child` 的
/// stdout 读端随之关闭，子进程里 libtest 打印测试总结会命中 EPIPE 而 panic——
/// 那是 libtest 与「占用进程 stdout」的固有冲突，与被测的 host 侧装配无关。
/// 保留读者线程的 stdout 句柄直到子进程退出，可让子进程干净收尾，从而**只**度量
/// 被测语义。
fn connect_to_child_holding_stdout() -> (Child, Client, std::thread::JoinHandle<()>) {
    let exe = std::env::current_exe().expect("定位测试二进制");
    let mut child = Command::new(exe)
        .args([CHILD_TEST, "--exact", "--nocapture"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env(CHILD_ENV, "1")
        .spawn()
        .expect("拉起 host 子进程");

    let stdin = child.stdin.take().expect("子进程 stdin 管道");
    let stdout = child.stdout.take().expect("子进程 stdout 管道");
    let (transport, drain) = child_transport(stdin, stdout);
    let client = Client::with_config(
        transport,
        ClientConfig {
            max_events: 64,
            poll_interval: Duration::from_millis(5),
        },
    );
    (child, client, drain)
}

/// 把子进程的 stdin/stdout 接成对端 `Transport`：写侧 = 子进程 stdin，读侧由
/// 一条 detach 线程逐行喂事件通道。与客户端侧 `spawn_stdio` 同形，但这里由测试
/// 显式接线（被测对象是 host 侧，不能借道被测函数）。
///
/// 读线程 detach：测试结束时若有残留，进程退出即回收，不阻塞。
fn child_transport(
    stdin: ChildStdin,
    stdout: std::process::ChildStdout,
) -> (Transport, std::thread::JoinHandle<()>) {
    let (source, tx) = event_channel();
    let drain = std::thread::Builder::new()
        .name("test-child-reader".into())
        .spawn(move || child_read_loop(stdout, tx))
        .expect("起测试读线程");
    let transport = Transport::with_teardown(
        Box::new(TestSink { stdin: Some(stdin) }),
        source,
        Teardown::new(|| {}),
    );
    (transport, drain)
}

/// 对端写侧：一帧一帧写进子进程 stdin 并 flush。
struct TestSink {
    stdin: Option<ChildStdin>,
}

impl FrameSink for TestSink {
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), agent_acp::Error> {
        let Some(stdin) = &mut self.stdin else {
            return Err(agent_acp::Error::Transport {
                reason: "子进程 stdin 已关闭".into(),
            });
        };
        stdin
            .write_all(frame)
            .and_then(|()| stdin.flush())
            .map_err(|e| agent_acp::Error::Transport {
                reason: format!("写子进程 stdin 失败：{e}"),
            })
    }

    fn close(&mut self) {
        self.stdin = None; // 关管道 → 子进程读到 EOF。
    }
}

/// 对端读侧：逐行读子进程 stdout，发进事件通道；EOF 即报终止。
///
/// **通道关闭后继续读到真正的 EOF**：`tx.send` 失败只说明本端路由已停，此时若
/// 立刻返回并丢掉 `ChildStdout`，会关掉子进程的 stdout 读端——子进程里 libtest
/// 打印测试总结就会命中 EPIPE。所以发送失败后转为「纯排空」，把子进程 stdout 读
/// 到 EOF 再退出，让子进程干净收尾。这只影响测试夹具，不改变被测语义。
fn child_read_loop(stdout: std::process::ChildStdout, tx: Sender<SourceEvent>) {
    let mut lines = BufReader::new(stdout).lines();
    let mut routed = true;
    loop {
        match lines.next() {
            None => {
                if routed {
                    let _ = tx.send(SourceEvent::Closed { reason: None });
                }
                return;
            }
            Some(Ok(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let ev = match agent_acp::decode_frame(line.as_bytes()) {
                    Ok(top) => SourceEvent::Frame(top),
                    Err(e) => SourceEvent::BadFrame {
                        reason: e.to_string(),
                    },
                };
                if routed && tx.send(ev).is_err() {
                    routed = false; // 路由已停：转为纯排空，直到子进程关 stdout。
                }
            }
            Some(Err(_)) => return,
        }
    }
}

/// 有界等待子进程退出；超时即 kill + 回收并失败（测试不许无限挂住、不留僵尸）。
fn wait_child_bounded(child: &mut Child, limit: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait().expect("轮询子进程状态") {
            Some(status) => return status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("子进程未在有界时间内退出（不残留僵尸：已 kill + wait）");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
