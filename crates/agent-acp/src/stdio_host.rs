//! host 侧（被 spawn 的一端）：托管**当前进程**的 stdin/stdout。
//!
//! 语义：外部（桌面壳 / 网关的对端）**spawn 本进程并持有管道两端**，本端只托管
//! 自己的 stdin/stdout——帧汇（写）→ 本进程 stdout，帧源（读）← 本进程 stdin。
//! 与 [`crate::stdio_client`] 的 `spawn_stdio` 是同一套 IO、方向相反：客户端侧本端
//! 持有子进程的两根管道，host 侧由对端持有本进程的两根管道。本模块只提供装配函数，
//! 不改既定 socket/stdin 拓扑（「谁 spawn 谁」的最终形态见
//! `docs/contracts/acp-host.md` §7 决策项 3，仍未定）。
//!
//! 与客户端侧唯一的结构差异：**没有子进程句柄可 kill**。客户端侧拆除靠
//! 「kill 子进程 → 其 stdout 关 → reader 的 read 返回」；host 侧没有这一手，且
//! `std::io::stdin()` 是进程全局句柄，进程内无法只关「读者这一份」（`from_raw_fd(0)`
//! 会在析构时关掉全局 fd 0，之后进程再也读不了 stdin），也**没有**任何用户态手段能
//! 打断另一线程里阻塞中的 `read(0)`（无 self-pipe、无 poll、无 unsafe 可借）。
//!
//! 处置：**读者线程一旦起就 detach**。它独占本进程 stdin，读到 EOF（对端关管道 /
//! 本端写侧释放）自行收尾；本端拆除**不 join 它**，而是让分派线程从「通道型帧源」
//! 的 `recv_timeout` 上醒来查 stop 标志退出——`Host::stop()` 里分派线程的 join 因此
//! 有界，与被阻塞的读者线程无关（见 `host_send.rs` 的分派循环：每 poll_interval
//! 醒一次查 stop）。代价：拆除后该线程可能仍挂着（它是 detached，进程退出时由内核回收），
//! 换来的是「拆除永不挂起」这一硬约定——与客户端侧 kill 兜底同旨。
//!
//! 因此 host 侧的帧源不是「直连子进程 stdout」，而是一根「由 detach 读者喂入」的
//! 事件通道：`event_channel()` 给分派线程用（永不阻塞读 stdin），读者线程把帧事件
//! 发进来（见 [`spawn_host_reader`]）。IO 实现仍是共用那一套（
//! [`crate::stdio_client::read_loop`]：`LineDecoder` 切行 + `decode_frame` 归类 +
//! fail-closed）。

use std::io::{BufReader, Read, Write};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::codec::DEFAULT_MAX_LINE_BYTES;
use crate::error::Error;
use crate::stdio_client::{join_bounded, read_loop, StdinSink, DEFAULT_SHUTDOWN_TIMEOUT};
use crate::transport::{event_channel, FrameSink, SourceEvent, Teardown, Transport};

/// 起 host 侧读者：**detach** 的线程，独占 stdin，把帧事件喂进 `tx`。
///
/// 与 `stdio_client::spawn_reader` 的差异只有两点：读的是标准输入（`Box<dyn Read>`
/// 而非 `ChildStdout`），以及**返回即放手**（不返回 `JoinHandle`，线程自行 detach）。
/// 切行 / 归类 / fail-closed 复用 `read_loop`，与客户端侧逐条同语义。
///
/// 读侧不引入任何「唤醒」机制：`std::io::stdin` 的阻塞 `read` 无法被本进程的
/// 其它线程以零 unsafe 打断（见本节头注释），拆除的**有界性**改由分派线程侧保证。
fn spawn_host_reader(stdin: Box<dyn Read + Send>, tx: Sender<SourceEvent>, max_line_bytes: usize) {
    let _ = std::thread::Builder::new()
        .name("acp-stdio-reader".into())
        .spawn(move || {
            read_loop(
                BufReader::new(stdin),
                &tx,
                max_line_bytes,
                "读取 host stdin 失败",
            )
        });
}

/// 起 writer 线程把帧字节写到 `writer`（host 形态不持子进程）。
///
/// 与 `stdio_client::spawn_writer` 同款：写入集中一根线程、对端退出即停、通道断开
/// 即 flush + 关 fd。线程资源耗尽时返回 Err。
fn spawn_writer_to(
    mut writer: Box<dyn Write + Send>,
    frame_rx: Receiver<Vec<u8>>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("acp-stdio-writer".into())
        .spawn(move || {
            while let Ok(frame) = frame_rx.recv() {
                if writer
                    .write_all(&frame)
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break; // 对端退出：后续 send_frame 会看到线程结束而报错。
                }
            }
            let _ = writer.flush();
            drop(writer); // 关 stdout：对端读到 EOF，自然收尾。
        })
}

/// 托管**当前进程** stdin/stdout，装出一端被外部（桌面壳 / 网关）spawn 的
/// [`Transport`]——即「[`Host`](crate::host::Host) 怎么接 stdio」的答案。
///
/// 方向与 `stdio_client::spawn_stdio` **相反**：
/// - 帧汇（写侧）→ 本进程 **stdout**：host 的应答与 `notify` 落在这里；
/// - 帧源（读侧）← 本进程 **stdin**：对端的请求 / 通知从这里进来。
///
/// 复用与 [`spawn_stdio`](crate::stdio_client::spawn_stdio) **同一套** IO 设施：
/// `read_loop`（`LineDecoder` 切行 + `decode_frame` 归类 + fail-closed）、
/// [`spawn_writer_to`]、有界拆除。本函数只做「标准流 → 帧源/帧汇」的装配与方向对调，
/// **不新写协议 IO**。
///
/// 拆除（[`Transport`] 的 `Teardown` 钩子，`Host::stop` 或未被接管即丢弃时执行
/// 一次）：关写侧 → 有界等 writer 线程。读者线程是 detach 的，不参与等待——它
/// 独占 stdin，对端关管道时自然收尾。**拆除全路径有界**，对端永不关 stdin 时
/// `Host::stop()` 也不挂起（详见本节头注释）。
///
/// # Errors
/// [`Error::Io`]：reader/writer 线程起不来（失败路径先回收已建线程，不留残留）。
///
/// # Examples
/// ```no_run
/// use agent_acp::host::Host;
/// use agent_acp::stdio::host_stdio_transport;
///
/// let host = Host::new(host_stdio_transport()?);
/// host.register_method("initialize", |_| Ok(serde_json::json!({"protocolVersion": 1})));
/// # Ok::<(), agent_acp::Error>(())
/// ```
#[cfg(unix)]
pub fn host_stdio_transport() -> crate::Result<Transport> {
    assemble_host_transport(
        Box::new(std::io::stdin()) as Box<dyn Read + Send>,
        Box::new(std::io::stdout()) as Box<dyn Write + Send>,
        DEFAULT_MAX_LINE_BYTES,
        DEFAULT_SHUTDOWN_TIMEOUT,
    )
}

/// host 形态的装配内核：接读侧/写侧字节流，起线程、装 [`Transport`]。
///
/// 与 `spawn_stdio` 共用一套线程与拆除设施，方向对调（读 stdin / 写 stdout）。
fn assemble_host_transport(
    stdin: Box<dyn Read + Send>,
    stdout: Box<dyn Write + Send>,
    max_line_bytes: usize,
    shutdown_timeout: Duration,
) -> crate::Result<Transport> {
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = match spawn_writer_to(stdout, frame_rx) {
        Ok(h) => h,
        Err(e) => {
            return Err(Error::Io {
                reason: format!("启动 host 侧 stdio writer 线程失败：{e}"),
            });
        }
    };
    // 帧源是通道型：分派线程的 `next_event(POLL_INTERVAL)` 永不阻塞在 stdin 上，
    // 故 `Host::stop()` 的分派线程 join 有界（读者线程 detach，见本节头注释）。
    let (source, ev_tx) = event_channel();
    // 读者线程 detach：起不来就拿不到「线程起不来」的 Err（detach 语义即「放弃
    // 这一手柄」）。真实失败只可能是资源耗尽，与 spawn_stdio 同源的取舍：
    // 这里选择不因读者线程失败而让整个 transport 构造失败——对端仍可用写侧。
    spawn_host_reader(stdin, ev_tx, max_line_bytes);
    let sink: Box<dyn FrameSink> = Box::new(StdinSink {
        tx: Some(frame_tx.clone()),
    });
    Ok(Transport::with_teardown(
        sink,
        source,
        Teardown::new(move || {
            teardown_host(frame_tx, writer, shutdown_timeout);
        }),
    ))
}

/// host 形态的拆除：关写侧 → 有界等 writer。读者线程 detach 不等待（见本节头注释）。
fn teardown_host(frame_tx: Sender<Vec<u8>>, writer: JoinHandle<()>, timeout: Duration) {
    drop(frame_tx); // writer 队列断开 → flush + 关 stdout，writer 自行退出。
    let _ = join_bounded(writer, timeout);
}
