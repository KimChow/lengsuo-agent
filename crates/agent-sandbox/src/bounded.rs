//! 有界执行层：spawn → 两条独立读线程（按流字节上限）→ 轮询 deadline → kill/wait → 有界收流。
//!
//! 分层对照 参考实现 `core/src/exec.rs` 的进程生命周期段：
//! 常量表 → [`append_capped`] → 读线程（超限后继续读并丢弃，避免管道写满堵死子进程）
//! → `try_wait` 轮询 + 到期 kill/wait → 在 drain 预算内收集已读字节（[`Drained`]）。
//!
//! 与参考实现的运行时差异（Rust std 同步实现，不引入 tokio/unsafe）：
//! 参考实现等 `child.wait()` 与过期定时器赛跑、用 `killpg` 杀整个进程组；
//! 本模块按卡片边界只 kill **直接子进程**，并以固定间隔轮询替代异步等待。
//! 孙进程若仍持有 stdout/stderr 写端，读线程会继续存活，但主流程只在 drain
//! 预算内等待，绝不无限挂起；此时 [`Drained::orphaned_writers`] 为真并留一条
//! 事实日志——不静默，也不据此声称进程树回收。

use std::io::Read;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::types::{ExecBounds, ExecOutcome, ExecStatus, SandboxError, SandboxType};

/// 单次 `read` 的最大字节数（参考实现 `READ_CHUNK_SIZE`）。
const READ_CHUNK_SIZE: usize = 8 * 1024;
/// 存活轮询间隔（参考实现用异步等待，无对应常量；同步实现取 10ms）。
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 进程退出后等待两条管道关闭的**总**预算（参考实现 `IO_DRAIN_TIMEOUT_MS` 为每条各 2s）。
/// 孙进程可能继承写端并长期持有；预算耗尽即返回已收字节，不再等待。
/// 两条流共用同一预算并轮询推进，避免一条卡住的流把预算吃光、
/// 也避免参考实现里「逐条各等满」把收尾时间翻倍。
const IO_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
/// drain 阶段的单次等待步长：小于预算，保证两条流都被轮询到。
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// 读线程发回主流程的事件。字节在发送前已按上限裁剪，通道占用因此有界。
enum StreamEvent {
    Chunk(Vec<u8>),
    /// 本流首次越过字节上限；只发一次。
    Capped,
}

/// 按上限追加：`dst` 最多增长到 `max_bytes`，多余字节丢弃（对照参考实现 `append_capped`）。
fn append_capped(dst: &mut Vec<u8>, src: &[u8], max_bytes: usize) {
    let remaining = max_bytes.saturating_sub(dst.len());
    let take = remaining.min(src.len());
    dst.extend_from_slice(&src[..take]);
}

/// 启动单条流的读取线程：读到 EOF 为止，最多保留 `cap` 字节；
/// **超限后继续读并丢弃**——提前停止读取会让写端收到 `SIGPIPE` 或阻塞在满管道上，
/// 改变子进程行为甚至造成死锁（对照参考实现 `read_output` 的相同注释）。
fn spawn_reader<R: Read + Send + 'static>(mut stream: R, cap: usize) -> Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0u8; READ_CHUNK_SIZE];
        // 保留缓冲只增长到 `cap`，发送的是每块新接受的字节；超限部分读完即丢。
        let mut retained: Vec<u8> = Vec::new();
        let mut capped = false;
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let before = retained.len();
                    append_capped(&mut retained, &chunk[..n], cap);
                    let kept = &retained[before..];
                    if kept.len() < n && !capped {
                        capped = true;
                        if tx.send(StreamEvent::Capped).is_err() {
                            break;
                        }
                    }
                    if !kept.is_empty() && tx.send(StreamEvent::Chunk(kept.to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// 收流结果：两条流的字节与封顶标记，外加「收尾时读端仍未关闭」的如实上报。
struct Drained {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// 任一条流撞到 [`ExecBounds::max_output_bytes`]。
    capped: bool,
    /// drain 预算耗尽时仍有读端未关闭：**直接子进程已被回收**，因此握有写端的
    /// 只可能是被它派生、且我们无权处置的后代（我们只 kill 直接子进程）。
    /// 该布尔只是事实上报，用于日志/指标；不得据此声称完成进程树回收。
    orphaned_writers: bool,
}

/// 在**共享** drain 预算内收齐两条流已发出的字节；预算耗尽或通道断开即返回。
/// 已收字节始终保留——超时路径也返回 partial 输出。
///
/// 两条流在同一循环里轮询（而不是逐条各等满预算）：被孙进程握住写端的流
/// 因此不会把收尾时间拖成「超时 + 2×预算」。
fn drain_both(stdout_rx: Receiver<StreamEvent>, stderr_rx: Receiver<StreamEvent>) -> Drained {
    let mut d = Drained {
        stdout: Vec::new(),
        stderr: Vec::new(),
        capped: false,
        orphaned_writers: false,
    };
    let mut stdout_done = false;
    let mut stderr_done = false;
    let deadline = Instant::now() + IO_DRAIN_TIMEOUT;
    while !stdout_done || !stderr_done {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // 预算耗尽而读端仍在：写端被后代握着（见 [`Drained::orphaned_writers`]）。
            d.orphaned_writers = !stdout_done || !stderr_done;
            break;
        }
        let step = remaining.min(DRAIN_POLL_INTERVAL);
        if !stdout_done {
            match stdout_rx.recv_timeout(step) {
                Ok(StreamEvent::Chunk(chunk)) => d.stdout.extend_from_slice(&chunk),
                Ok(StreamEvent::Capped) => d.capped = true,
                Err(RecvTimeoutError::Disconnected) => stdout_done = true,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
        if !stderr_done {
            match stderr_rx.recv_timeout(step) {
                Ok(StreamEvent::Chunk(chunk)) => d.stderr.extend_from_slice(&chunk),
                Ok(StreamEvent::Capped) => d.capped = true,
                Err(RecvTimeoutError::Disconnected) => stderr_done = true,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
    d
}

/// 执行已配置好 stdio 的命令，并在 `bounds` 内收尾。
///
/// `cmd` 的调用方必须已设好 `stdin(null)` 与 `stdout/stderr(piped)`；本函数负责
/// spawn、读取、超时 kill/wait 与结果投影。
pub(crate) fn capture(
    mut cmd: ProcessCommand,
    sandbox: SandboxType,
    label: &str,
    bounds: ExecBounds,
) -> Result<ExecOutcome, SandboxError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| SandboxError::Spawn(format!("{label}: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| SandboxError::Spawn(format!("{label}: stdout pipe unavailable")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| SandboxError::Spawn(format!("{label}: stderr pipe unavailable")))?;
    let stdout_rx = spawn_reader(stdout, bounds.max_output_bytes);
    let stderr_rx = spawn_reader(stderr, bounds.max_output_bytes);

    let deadline = Instant::now() + bounds.timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    // kill 之后必须 wait：否则直接子进程留成僵尸。
                    let status = child
                        .wait()
                        .map_err(|e| SandboxError::Spawn(format!("{label}: {e}")))?;
                    break status;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(POLL_INTERVAL));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SandboxError::Spawn(format!("{label}: {e}")));
            }
        }
    };

    let drained = drain_both(stdout_rx, stderr_rx);
    // 事实级日志：有后代在 drain 预算后仍握着管道写端时，明确记一笔——不静默。
    if drained.orphaned_writers {
        eprintln!("{label}: descendants still hold stdout/stderr after drain budget");
    }
    Ok(ExecOutcome {
        status: to_exec_status(&status),
        stdout: drained.stdout,
        stderr: drained.stderr,
        sandbox,
        timed_out,
        truncated: drained.capped,
    })
}

fn to_exec_status(status: &std::process::ExitStatus) -> ExecStatus {
    use std::os::unix::process::ExitStatusExt;
    ExecStatus {
        code: status.code(),
        signal: status.signal(),
        success: status.success(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_capped_never_exceeds_cap() {
        let mut dst = Vec::new();
        append_capped(&mut dst, b"abc", 5);
        append_capped(&mut dst, b"defgh", 5);
        append_capped(&mut dst, b"ijk", 5);
        assert_eq!(dst, b"abcde");
        // 上限为 0 时一个字节也不保留。
        let mut empty = Vec::new();
        append_capped(&mut empty, b"abc", 0);
        assert!(empty.is_empty());
    }
}
