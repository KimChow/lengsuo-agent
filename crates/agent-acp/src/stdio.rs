//! 真实 stdio 传输。两个方向相反、但共用同一套线程/拆除设施的形态：
//!
//! - [`spawn_stdio`]：**客户端侧**（本端 spawn agent 子进程），持其 stdin/stdout；
//!   帧汇 = 子进程 stdin，帧源 = 子进程 stdout。
//! - [`host_stdio_transport`]：**host 侧**（本端被外部桌面壳 spawn），托管**当前
//!   进程**的 stdin/stdout；帧汇 = 本进程 stdout，帧源 = 本进程 stdin。方向与
//!   客户端侧相反，IO 实现（[`read_loop`]/[`spawn_writer_to`]/有界拆除）完全复用。
//!
//! 无论哪一侧，字节流 → reader 线程（[`LineDecoder`] + [`crate::codec::decode_frame`]
//! 归类），writer 线程 ← 帧汇通道。同步线程模型。
//!
//! 安全基线：spawn 前对子进程环境做 **scrub**——`env_clear` 后只装回过滤过的
//! 变量：保留最小系统变量白名单，剔除凭据族（名字含 `KEY`/`TOKEN`/`SECRET`/
//! `PASSWORD` 等，或云厂商前缀），支持显式 denylist 追加与「白名单整体替换」
//! 两档收口。非 UTF-8 变量名/值一律剥离（不确定即敏感，fail-closed）。
//!
//! stderr 策略：**透传不解析**（`Stdio::inherit()`），agent 诊断直落父进程
//! stderr，不与 stdout 帧流混淆。host 侧不接管 stderr（那是对端/终端的事）。
//!
//! 拆除（[`Teardown`] 钩子，Client/Host 关闭或 Transport 未被接管即丢弃时执行一次）：
//! 释放写侧通道（writer flush + 关 fd）→ 有界等待 writer/reader 线程 → 客户端
//! 形态下子进程仍活则 `kill` + reap。任何一步都有界，最坏路径是 kill 倒逼管道
//! EOF，卡不住调用方。host 侧与 stdin 同源：关 fd 只影响**本进程后续读**，
//! 对端 EOF 即收尾（见 [`host_stdio_transport`] 注释）。

use std::io::{BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::codec::{decode_frame, LineDecoder, DEFAULT_MAX_LINE_BYTES};
use crate::error::Error;
use crate::transport::{event_channel, FrameSink, SourceEvent, Teardown, Transport};

/// 默认拆除有界等待：2 秒。
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// 读块大小：8 KiB（行帧在 [`LineDecoder`] 里重组，块大小不影响语义）。
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// 凭据族变量名模式（大写包含匹配）。
const CREDENTIAL_PATTERNS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "COOKIE",
    "AUTH",
];

/// 凭据族前缀（云厂商/凭证链）。
const CREDENTIAL_PREFIXES: &[&str] = &["AWS_", "AZURE_", "GCP_", "GOOGLE_", "ALIBABA_CLOUD_"];

/// 最小系统变量白名单（allowlist 模式下唯一保留集）。
pub const SYSTEM_ENV_KEEP: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// 变量名是否属于凭据族（不确定即视为敏感）。
#[must_use]
pub fn is_credential_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    CREDENTIAL_PREFIXES.iter().any(|p| upper.starts_with(p))
        || CREDENTIAL_PATTERNS.iter().any(|p| upper.contains(p))
}

/// scrub 规则：该变量是否可传给子进程。
#[must_use]
pub fn env_pair_allowed(key: &str, allowlist_only: bool, extra_denied: &[String]) -> bool {
    if extra_denied.iter().any(|d| d == key) || is_credential_env(key) {
        return false;
    }
    !allowlist_only || SYSTEM_ENV_KEEP.contains(&key)
}

/// spawn 配置。
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// 可执行文件路径。
    pub program: String,
    /// 参数表。
    pub args: Vec<String>,
    /// 追加剥离的环境变量名（精确匹配）。
    pub denied_env: Vec<String>,
    /// true：子进程环境整体替换为白名单（最严格）；false：按凭据族模式剥离。
    pub allowlist_only_env: bool,
    /// 单行帧上限。
    pub max_line_bytes: usize,
    /// 拆除时有界等待上限。
    pub shutdown_timeout: Duration,
}

impl SpawnConfig {
    /// 以程序名新建配置（其余取安全默认）。
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            denied_env: Vec::new(),
            allowlist_only_env: false,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
        }
    }

    /// 追加一个参数。
    #[must_use]
    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
}

/// 写侧：帧字节经通道交给 writer 线程落子进程 stdin。
///
/// 写入集中在一根线程：管道写满只挂起 writer，不阻塞路由线程与调用方。
struct StdinSink {
    tx: Option<Sender<Vec<u8>>>,
}

impl FrameSink for StdinSink {
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), Error> {
        let Some(tx) = &self.tx else {
            return Err(Error::Transport {
                reason: "子进程 stdin 已关闭".into(),
            });
        };
        tx.send(frame.to_vec()).map_err(|_| Error::Transport {
            reason: "子进程 stdin writer 线程已退出".into(),
        })
    }

    /// 释放发送端 → writer 收到断连 → flush + 关 stdin。幂等。
    fn close(&mut self) {
        self.tx = None;
    }
}

/// 回收子进程：kill + wait，绝不留孤儿/僵尸。
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// 起 writer 线程；线程资源耗尽时返回 Err（由 [`spawn_stdio`] 负责收尸）。
fn spawn_writer(stdin: ChildStdin, rx: Receiver<Vec<u8>>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("acp-stdio-writer".into())
        .spawn(move || {
            let mut stdin = stdin;
            while let Ok(frame) = rx.recv() {
                if stdin
                    .write_all(&frame)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break; // 对端退出：后续 send_frame 会看到线程结束而报错。
                }
            }
            let _ = stdin.flush();
            drop(stdin); // 关 stdin：agent 读到 EOF，自然收尾。
        })
}

/// 起 reader 线程；线程资源耗尽时返回 Err（由 [`spawn_stdio`] 负责收尸）。
fn spawn_reader(
    stdout: BufReader<Box<dyn Read + Send>>,
    tx: Sender<SourceEvent>,
    max_line_bytes: usize,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("acp-stdio-reader".into())
        .spawn(move || read_loop(stdout, &tx, max_line_bytes, "读取子进程 stdout 失败"))
}

/// 读者主循环（客户端侧子进程 stdout 与 host 侧 stdin 共用）。
///
/// 切行（[`LineDecoder`]）、归类（[`decode_frame`]）、fail-closed（超长行/截断
/// 下毒）三条语义对两侧逐字一致，只有 `what` 用于 IO 错误的消息措辞。
fn read_loop(
    mut reader: BufReader<Box<dyn Read + Send>>,
    tx: &Sender<SourceEvent>,
    max_line_bytes: usize,
    what: &str,
) {
    let mut dec = LineDecoder::new(max_line_bytes);
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                // EOF：残留未完结字节是截断帧，fail-closed 下毒。
                let reason = dec.finish().err().map(|e| e.to_string());
                let _ = tx.send(SourceEvent::Closed { reason });
                return;
            }
            Ok(n) => match dec.feed(&chunk[..n]) {
                Ok(frames) => {
                    for body in frames {
                        let ev = match decode_frame(&body) {
                            Ok(top) => SourceEvent::Frame(top),
                            Err(e) => SourceEvent::BadFrame {
                                reason: e.to_string(),
                            },
                        };
                        if tx.send(ev).is_err() {
                            return; // 路由已停。
                        }
                    }
                }
                Err(e) => {
                    // 超长行/已损坏：字节流无法重新对齐，终止连接。
                    let _ = tx.send(SourceEvent::Closed {
                        reason: Some(e.to_string()),
                    });
                    return;
                }
            },
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let _ = tx.send(SourceEvent::Closed {
                    reason: Some(format!("{what}：{e}")),
                });
                return;
            }
        }
    }
}

/// 有界等待线程退出；超时返回 false（放弃等待句柄，kill 已倒逼其解阻塞）。
fn join_bounded(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = handle.join();
    true
}

/// 拆除子进程：关 stdin → 有界等线程 → kill + reap。钩子内按 move 语义执行一次。
fn teardown(
    stdin_tx_clone: Sender<Vec<u8>>,
    mut child: Child,
    writer: JoinHandle<()>,
    reader: JoinHandle<()>,
    timeout: Duration,
) {
    drop(stdin_tx_clone); // sink 的 tx 此前已释放：writer 收到断连 → flush+关 stdin。
    if !join_bounded(reader, timeout) || !join_bounded(writer, timeout) {
        // 线程卡住：kill 倒逼 stdout EOF / stdin 写失败，不等线程。
        reap(&mut child);
        return;
    }
    // 线程已退：agent 不理 stdin EOF 也 kill，绝不留孤儿/僵尸。
    if !matches!(child.try_wait(), Ok(Some(_))) {
        reap(&mut child);
    }
}

/// scrub 当前进程环境并 spawn 子进程，返回接好管道的 [`Transport`]
/// （带一次性拆除钩子；[`crate::client::Client`] 接管，或自身丢弃时触发）。
///
/// # Errors
/// [`Error::Io`]：spawn 失败、子进程未提供 stdin/stdout 管道，或 reader/writer
/// 线程起不来（失败前一律先 kill+reap 子进程，不留孤儿）。
pub fn spawn_stdio(cfg: &SpawnConfig) -> crate::Result<Transport> {
    let mut command = Command::new(&cfg.program);
    command
        .args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()); // 透传不解析。
    command.env_clear();
    for (k, v) in std::env::vars_os() {
        let (Some(k), Some(v)) = (k.into_string().ok(), v.into_string().ok()) else {
            continue; // 非 UTF-8：不确定即剥离。
        };
        if env_pair_allowed(&k, cfg.allowlist_only_env, &cfg.denied_env) {
            command.env(&k, &v);
        }
    }
    let shutdown_timeout = cfg.shutdown_timeout;
    let max_line_bytes = cfg.max_line_bytes;
    let mut child = command.spawn().map_err(|e| Error::Io {
        reason: format!(
            "启动 ACP agent 子进程失败（program={:?}）：{e}",
            cfg.program
        ),
    })?;
    let stdin = match (child.stdin.take(), child.stdout.take()) {
        (Some(si), Some(so)) => (si, so),
        _ => {
            // 管道缺失：先收尸再报错，不留孤儿。
            reap(&mut child);
            return Err(Error::Io {
                reason: "子进程未提供 stdin/stdout 管道".into(),
            });
        }
    };

    // 线程起不来也先收尸：模块契约是「任何路径都不留孤儿进程」。
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = match spawn_writer(stdin.0, frame_rx) {
        Ok(h) => h,
        Err(e) => {
            reap(&mut child);
            return Err(Error::Io {
                reason: format!("启动 stdio writer 线程失败：{e}"),
            });
        }
    };
    let (source, ev_tx) = event_channel();
    let reader = match spawn_reader(
        BufReader::new(Box::new(stdin.1) as Box<dyn Read + Send>),
        ev_tx,
        max_line_bytes,
    ) {
        Ok(h) => h,
        Err(e) => {
            // 先收尸倒逼 writer 从管道写/通道收里解阻塞，再回收该线程。
            reap(&mut child);
            drop(frame_tx);
            let _ = writer.join();
            return Err(Error::Io {
                reason: format!("启动 stdio reader 线程失败：{e}"),
            });
        }
    };

    let sink = Box::new(StdinSink {
        tx: Some(frame_tx.clone()),
    });
    let teardown = Teardown::new(move || {
        teardown(frame_tx, child, writer, reader, shutdown_timeout);
    });
    Ok(Transport::with_teardown(sink, source, teardown))
}

// ── host 侧（被 spawn 的一端）：托管当前进程的 stdin/stdout ──────────────────
//
// 语义：外部（桌面壳 / 网关的对端）**spawn 本进程并持有管道两端**，本端只托管
// 自己的 stdin/stdout——帧汇（写）→ 本进程 stdout，帧源（读）← 本进程 stdin。
// 与 `spawn_stdio` 是同一套 IO、方向相反：客户端侧本端持有子进程的两根管道，
// host 侧由对端持有本进程的两根管道。本卡只提供装配函数，不改既定 socket/stdin
// 拓扑（「谁 spawn 谁」的最终形态见 `docs/contracts/acp-host.md` §7 决策项 3，仍未定）。
//
// 与客户端侧唯一的结构差异：**没有子进程句柄可 kill**。客户端侧拆除靠
// 「kill 子进程 → 其 stdout 关 → reader 的 read 返回」；host 侧没有这一手，且
// `std::io::stdin()` 是进程全局句柄，进程内无法只关「读者这一份」（`from_raw_fd(0)`
// 会在析构时关掉全局 fd 0，之后进程再也读不了 stdin），也**没有**任何用户态手段能
// 打断另一线程里阻塞中的 `read(0)`（无 self-pipe、无 poll、无 unsafe 可借）。
//
// 处置：**读者线程一旦起就 detach**。它独占本进程 stdin，读到 EOF（对端关管道 /
// 本端写侧释放）自行收尾；本端拆除**不 join 它**，而是让分派线程从「通道型帧源」
// 的 `recv_timeout` 上醒来查 stop 标志退出——`Host::stop()` 里分派线程的 join 因此
// 有界，与被阻塞的读者线程无关（见 `host_router::dispatch_loop`：每 `POLL_INTERVAL`
// 醒一次）。代价：拆除后该线程可能仍挂着（它是 detached，进程退出时由内核回收），
// 换来的是「拆除永不挂起」这一硬约定——与客户端侧 kill 兜底同旨。
//
// 因此 host 侧的帧源不是「直连子进程 stdout」，而是一根「由 detach 读者喂入」的
// 事件通道：`event_channel()` 给分派线程用（永不阻塞读 stdin），读者线程把帧事件
// 发进来（见 [`spawn_host_reader`]）。IO 实现仍是 `stdio.rs` 既有那一套
// （`read_loop`：`LineDecoder` 切行 + `decode_frame` 归类 + fail-closed）。

/// 起 host 侧读者：**detach** 的线程，独占 stdin，把帧事件喂进 `tx`。
///
/// 与 [`spawn_reader`] 的差异只有两点：读的是标准输入（`Box<dyn Read>` 而非
/// `ChildStdout`），以及**返回即放手**（不返回 `JoinHandle`，线程自行 detach）。
/// 切行 / 归类 / fail-closed 复用 [`read_loop`]，与客户端侧逐条同语义。
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
/// 与 [`spawn_writer`] 同款：写入集中一根线程、对端退出即停、通道断开即 flush +
/// 关 fd。线程资源耗尽时返回 Err。
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
/// 方向与 [`spawn_stdio`] **相反**：
/// - 帧汇（写侧）→ 本进程 **stdout**：host 的应答与 `notify` 落在这里；
/// - 帧源（读侧）← 本进程 **stdin**：对端的请求 / 通知从这里进来。
///
/// 复用与 [`spawn_stdio`] **同一套** IO 设施：[`read_loop`]（`LineDecoder` 切行 +
/// `decode_frame` 归类 + fail-closed）、[`spawn_writer_to`]、有界拆除。本函数只做
/// 「标准流 → 帧源/帧汇」的装配与方向对调，**不新写协议 IO**。
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
