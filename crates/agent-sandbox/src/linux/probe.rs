//! bwrap 真实能力探测（分层对照 参考实现 `sandboxing/src/bwrap.rs`）。
//!
//! 分层：常量表 → 结论类型 → 注入执行器 → 探测入口 → 真实执行器（spawn/轮询/回收）。
//! 与参考实现的语义差异：参考实现把探测结果当**告警启发式**（失败也可能不告警）；
//! 本仓把它当**能力门**——只要最小隔离命令没跑通（非零退出/执行失败/超时）一律判不可用。
//! 探测命令复用执行层同一构造层（[`super::build_bwrap_argv`]），保证「探测的形状 == 执行的形状」。
//!
//! 副作用约束：命令无可写绑定、无 cwd、`--clearenv`、`--unshare-net`——不写用户文件、不触网；
//! 超时路径 kill 后必须 wait 回收，不留僵尸；stderr 读取共享同一 deadline，绝不越过总超时。

use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStderr, Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::types::{CommandSpec, NetworkMode};

use super::build_bwrap_argv;

/// 系统 bwrap 程序名（参考实现 `SYSTEM_BWRAP_PROGRAM`）。
pub const SYSTEM_BWRAP_PROGRAM: &str = "bwrap";
/// 探测总超时（参考实现 `SYSTEM_BWRAP_PROBE_TIMEOUT`）。
pub const SYSTEM_BWRAP_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// 存活轮询间隔（参考实现 `SYSTEM_BWRAP_PROBE_POLL_INTERVAL`）。
pub const SYSTEM_BWRAP_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// stderr 累计读取上限（参考实现 `SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES`）。
pub const SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES: u64 = 64 * 1024;
/// 失败原因里回填的 stderr 字符上限（防止超长报错灌爆日志）。
const REASON_STDERR_LIMIT_CHARS: usize = 200;
/// 已知「无法建 user/net 命名空间」的报错串（参考实现 `USER_NAMESPACE_FAILURES`）。
pub const USER_NAMESPACE_FAILURES: [&str; 4] = [
    "loopback: Failed RTM_NEWADDR",
    "loopback: Failed RTM_NEWLINK",
    "setting up uid map: Permission denied",
    "No permissions to create a new namespace",
];
/// 探测命令最终执行的程序（参考实现同用 /bin/true）。
const PROBE_PROGRAM: &str = "/bin/true";

/// 探测命令规格：经执行层同一构造层产出 argv。
/// 空环境、无 cwd、无可写/只读绑定、`NetworkMode::Blocked` → 探测本身零副作用。
pub fn probe_command_spec() -> CommandSpec {
    CommandSpec {
        program: PROBE_PROGRAM.into(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        network: NetworkMode::Blocked,
    }
}

/// 一次探测运行的原始结果（执行器自报；`pid` 供上层/测试核对进程已回收）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRun {
    /// 探测子进程 pid；spawn 失败时为 0。
    pub pid: u32,
    pub outcome: ProbeOutcome,
}

/// 探测运行的结局。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 子进程已退出并被回收。
    Exited {
        success: bool,
        code: Option<i32>,
        signal: Option<i32>,
        stderr: Vec<u8>,
    },
    /// 超时：执行器必须已 kill + wait 回收本次探测进程后才返回。
    TimedOut,
    /// 无法启动（文件不存在、无执行权限等）。
    SpawnFailed(String),
}

/// 探测执行器：真实进程（[`SystemProbeExec`]）或测试替身——失败路径由此可注入。
pub trait ProbeExec {
    /// 在 `timeout` 总预算内运行探测命令并回收子进程（超时必须先 kill 再 wait）。
    fn run_probe(&self, program: &Path, args: &[String], timeout: Duration) -> ProbeRun;
}

/// 真实执行器：直接 spawn 指定的 bwrap 可执行文件。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProbeExec;

/// 探测结论：比 bool 多一层原因，供上层审批说明/日志。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BwrapProbe {
    /// 最小隔离命令真实跑通（退出码 0）。
    Usable,
    /// PATH 中找不到 bwrap。
    Missing,
    /// 可执行文件在，但执行失败（非零退出/无法启动/权限不足）。
    Failed(String),
    /// 探测超时（本次探测进程已回收）。
    TimedOut(String),
}

impl BwrapProbe {
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Usable)
    }
    /// 不可用原因；可用时为 None。
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Usable => None,
            Self::Missing => Some(format!("{SYSTEM_BWRAP_PROGRAM} not found in PATH")),
            Self::Failed(message) | Self::TimedOut(message) => Some(message.clone()),
        }
    }
}

/// 真实系统探测：PATH 找 bwrap → 以默认超时跑最小隔离命令。
pub fn probe_system_bwrap() -> BwrapProbe {
    probe_system_bwrap_with(&SystemProbeExec, SYSTEM_BWRAP_PROBE_TIMEOUT)
}

/// 注入版系统探测：PATH 查找用真实环境，命令执行可注入。
/// WSL1 短路在 PATH 检查之前（参考实现 `system_bwrap_warning_for_path` 同序）。
pub fn probe_system_bwrap_with(exec: &impl ProbeExec, timeout: Duration) -> BwrapProbe {
    if super::wsl::is_wsl1() {
        return BwrapProbe::Failed(super::wsl::WSL1_BWRAP_WARNING.to_string());
    }
    let Some(bwrap) = super::find_system_bwrap_in_path() else {
        return BwrapProbe::Missing;
    };
    probe_bwrap_with(exec, &bwrap, timeout)
}

/// 对指定 bwrap 路径做真实能力探测：跑最小隔离命令并验证退出状态。
pub fn probe_bwrap_with(exec: &impl ProbeExec, bwrap: &Path, timeout: Duration) -> BwrapProbe {
    let args = build_bwrap_argv(&probe_command_spec(), &[]);
    match exec.run_probe(bwrap, &args, timeout).outcome {
        ProbeOutcome::Exited { success: true, .. } => BwrapProbe::Usable,
        ProbeOutcome::Exited {
            success: false,
            code,
            signal,
            stderr,
        } => BwrapProbe::Failed(failure_reason(&stderr, code, signal)),
        ProbeOutcome::TimedOut => BwrapProbe::TimedOut(format!(
            "{SYSTEM_BWRAP_PROGRAM} probe timed out after {}ms (probe process reaped)",
            timeout.as_millis()
        )),
        ProbeOutcome::SpawnFailed(error) => BwrapProbe::Failed(format!(
            "{SYSTEM_BWRAP_PROGRAM} probe spawn failed: {error}"
        )),
    }
}

/// 失败原因：命中已知命名空间失败串 → 明说「建不了 user namespaces」；否则给退出状态。
pub fn failure_reason(stderr: &[u8], code: Option<i32>, signal: Option<i32>) -> String {
    let detail = stderr_detail(stderr);
    if is_user_namespace_failure(stderr) {
        return format!("{SYSTEM_BWRAP_PROGRAM} cannot create user namespaces: {detail}");
    }
    let status = match (code, signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "unknown status".to_string(),
    };
    if detail.is_empty() {
        format!("{SYSTEM_BWRAP_PROGRAM} probe failed ({status})")
    } else {
        format!("{SYSTEM_BWRAP_PROGRAM} probe failed ({status}): {detail}")
    }
}

/// 参考实现 `is_user_namespace_failure`：stderr 命中已知失败串即判命名空间不可用。
pub fn is_user_namespace_failure(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr);
    USER_NAMESPACE_FAILURES.iter().any(|f| text.contains(f))
}

fn stderr_detail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    trimmed.chars().take(REASON_STDERR_LIMIT_CHARS).collect()
}

impl ProbeExec for SystemProbeExec {
    fn run_probe(&self, program: &Path, args: &[String], timeout: Duration) -> ProbeRun {
        let mut cmd = ProcessCommand::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                return ProbeRun {
                    pid: 0,
                    outcome: ProbeOutcome::SpawnFailed(error.to_string()),
                }
            }
        };
        let pid = child.id();
        let stderr_rx = spawn_stderr_reader(child.stderr.take());
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return ProbeRun {
                        pid,
                        outcome: ProbeOutcome::Exited {
                            success: status.success(),
                            code: status.code(),
                            signal: exit_signal(&status),
                            stderr: drain_stderr_within(stderr_rx, deadline),
                        },
                    };
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        reap(&mut child);
                        return ProbeRun {
                            pid,
                            outcome: ProbeOutcome::TimedOut,
                        };
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(SYSTEM_BWRAP_PROBE_POLL_INTERVAL));
                }
                Err(error) => {
                    reap(&mut child);
                    return ProbeRun {
                        pid,
                        outcome: ProbeOutcome::SpawnFailed(error.to_string()),
                    };
                }
            }
        }
    }
}

/// 超时/异常路径：先 kill 再 wait，确保不留僵尸。
fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// stderr 读取线程：按块搬运、有界累计；**只 send 不 join**，
/// 孙进程攥着写端时线程随管道关闭自行结束，绝不阻塞主流程。
fn spawn_stderr_reader(stderr: Option<ChildStderr>) -> Option<Receiver<Vec<u8>>> {
    let stderr = stderr?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = stderr.take(SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES);
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Some(rx)
}

/// 在 deadline 预算内尽力收 stderr：拿到已缓冲的字节就继续等下一个块，
/// 但预算耗尽立即放弃（共用探测总超时，绝不越过）。
fn drain_stderr_within(rx: Option<Receiver<Vec<u8>>>, deadline: Instant) -> Vec<u8> {
    let Some(rx) = rx else {
        return Vec::new();
    };
    let mut collected = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(chunk) => {
                collected.extend_from_slice(&chunk);
                if collected.len() as u64 >= SYSTEM_BWRAP_PROBE_STDERR_LIMIT_BYTES {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    collected
}

fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reason_names_program() {
        assert_eq!(
            BwrapProbe::Missing.reason(),
            Some("bwrap not found in PATH".to_string())
        );
        assert!(BwrapProbe::Usable.reason().is_none());
    }

    #[test]
    fn userns_failures_are_classified() {
        for failure in USER_NAMESPACE_FAILURES {
            let mut stderr = failure.as_bytes().to_vec();
            stderr.extend_from_slice(b"\n");
            assert!(is_user_namespace_failure(&stderr), "{failure}");
            let reason = failure_reason(&stderr, Some(1), None);
            assert!(reason.contains("user namespaces"), "{reason}");
        }
        let unrelated = b"bwrap: Unknown option --argv0\n".to_vec();
        assert!(!is_user_namespace_failure(&unrelated));
        let reason = failure_reason(&unrelated, Some(1), None);
        assert!(reason.contains("exit code 1"), "{reason}");
    }

    #[test]
    fn failure_reason_covers_signal_and_unknown_status() {
        assert!(failure_reason(b"", None, Some(9)).contains("signal 9"));
        assert!(failure_reason(b"", None, None).contains("unknown status"));
        // 超长 stderr 截断到原因上限。
        let long = vec![b'x'; REASON_STDERR_LIMIT_CHARS + 50];
        assert_eq!(
            failure_reason(&long, Some(1), None).chars().count(),
            200 + format!("{SYSTEM_BWRAP_PROGRAM} probe failed (exit code 1): ").len()
        );
    }

    #[test]
    fn probe_command_spec_is_minimal() {
        let spec = probe_command_spec();
        assert_eq!(spec.program, std::path::PathBuf::from("/bin/true"));
        assert!(spec.writable_roots.is_empty());
        assert!(spec.read_only_roots.is_empty());
        assert!(spec.cwd.is_none());
        assert!(spec.env.is_empty());
        assert_eq!(spec.network, NetworkMode::Blocked);
        let args = build_bwrap_argv(&spec, &[]);
        assert!(args.iter().any(|a| a == "--unshare-net"));
        assert!(args.iter().any(|a| a == "--clearenv"));
        assert!(!args.iter().any(|a| a == "--bind"));
    }
}
