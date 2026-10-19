//! 执行有界性行为测试：超时、输出上限与「不可用时绝不静默直跑」。
//!
//! 全部用例只跑 `/bin/sh`/`/bin/true` 等只读系统程序与临时目录文件，
//! 不写用户文件、不触网、不依赖宿主是否具备 user namespace。

#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_sandbox::{
    build_bwrap_argv, run_sandboxed_bounded, run_sandboxed_with_bounded, BwrapProbe, CommandSpec,
    ExecBounds, NetworkMode, SandboxError, SandboxPolicy, SandboxProbe, SandboxType,
    SandboxablePreference,
};

/// 探测恒不可用：用来证明 Forbid 不依赖探测、Auto/Require 仍然 fail-closed。
struct UnavailableProbe;

impl SandboxProbe for UnavailableProbe {
    fn bwrap_available(&self) -> bool {
        false
    }
    fn landlock_available(&self) -> bool {
        false
    }
    fn bwrap_probe(&self) -> BwrapProbe {
        BwrapProbe::Failed("injected: bwrap unavailable".to_string())
    }
}

fn shell_spec(script: &str) -> CommandSpec {
    CommandSpec {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        cwd: None,
        writable_roots: vec![],
        read_only_roots: vec![],
        network: NetworkMode::Blocked,
    }
}

fn forbid(spec: CommandSpec, bounds: ExecBounds) -> agent_sandbox::ExecOutcome {
    run_sandboxed_with_bounded(
        &UnavailableProbe,
        spec,
        SandboxablePreference::Forbid,
        SandboxPolicy::default(),
        bounds,
    )
    .expect("Forbid must run unsandboxed")
}

/// 200ms 超时跑 sleep 5：必须 1s 内返回、带已产出的 partial 输出、置 `timed_out`，
/// 且直接子进程已被 kill + wait 回收（/proc/<pid> 消失，无僵尸/残留）。
#[test]
fn forbid_timeout_is_bounded_and_keeps_partial_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("child.pid");
    let spec = shell_spec(&format!(
        "echo started; echo $$ > '{}'; sleep 5",
        pid_file.display()
    ));
    let bounds = ExecBounds {
        timeout: Duration::from_millis(200),
        ..ExecBounds::default()
    };

    let started = Instant::now();
    let out = forbid(spec, bounds);
    let elapsed = started.elapsed();

    assert!(out.timed_out, "timeout must be reported: {out:?}");
    assert!(!out.status.success, "killed child must not look successful");
    assert!(
        elapsed < Duration::from_secs(1),
        "bounded run returned too late: {elapsed:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("started"),
        "partial stdout must be preserved: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(out.sandbox, SandboxType::None);

    let pid: i32 = fs::read_to_string(&pid_file)
        .expect("child pid recorded")
        .trim()
        .parse()
        .expect("numeric pid");
    assert!(pid > 0);
    // 主流程返回前已 kill + wait；给内核/进程表一个极短窗口后仍必须消失。
    let deadline = Instant::now() + Duration::from_millis(500);
    while PathBuf::from(format!("/proc/{pid}")).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "direct child {pid} was not reaped (zombie or still running)"
    );
}

/// 输出上限按流生效：超限部分丢弃、标记 `truncated`，正常退出语义不变；
/// 读线程在超限后继续读（否则大输出会把子进程堵死在写管道上，最终超时）。
#[test]
fn output_cap_truncates_without_deadlock() {
    let bounds = ExecBounds {
        timeout: Duration::from_secs(5),
        max_output_bytes: 64,
    };
    let started = Instant::now();
    // 200KB 远超 64 字节上限，也远超管道缓冲；读线程必须持续排空。
    let out = forbid(shell_spec("head -c 200000 /dev/zero"), bounds);
    let elapsed = started.elapsed();

    assert!(!out.timed_out, "large output must not turn into a timeout");
    assert!(out.status.success, "child should exit 0: {out:?}");
    assert_eq!(out.stdout.len(), 64, "stdout must be capped per stream");
    assert!(out.truncated, "cap hit must be reported");
    assert!(
        elapsed < Duration::from_secs(5),
        "capped read must keep draining: {elapsed:?}"
    );
}

/// stdout 与 stderr 各自独立计量上限；任一流超限都置 truncated。
#[test]
fn stderr_has_its_own_cap() {
    let bounds = ExecBounds {
        timeout: Duration::from_secs(5),
        max_output_bytes: 32,
    };
    let out = forbid(
        shell_spec("printf 'abcdefghij'; printf '0123456789' >&2; exit 7"),
        bounds,
    );
    assert!(!out.timed_out);
    assert!(!out.status.success);
    assert_eq!(out.status.code, Some(7), "exit code must propagate");
    assert_eq!(out.stdout, b"abcdefghij");
    assert_eq!(out.stderr, b"0123456789");
    assert!(!out.truncated, "under-cap streams must not be flagged");
}

/// 正常快命令在默认 bounds 下行为不变：退出码、输出、成功标志原样透传。
#[test]
fn fast_command_behaviour_is_unchanged() {
    let out = forbid(shell_spec("printf hello"), ExecBounds::default());
    assert!(!out.timed_out);
    assert!(!out.truncated);
    assert!(out.status.success);
    assert_eq!(out.status.code, Some(0));
    assert_eq!(out.stdout, b"hello");
    assert!(out.stderr.is_empty());
}

/// 探测不可用时仍按原有语义拒绝或转审批，绝不静默直跑。
#[test]
fn unavailable_sandbox_never_runs_silently() {
    let require = run_sandboxed_with_bounded(
        &UnavailableProbe,
        shell_spec("true"),
        SandboxablePreference::Require,
        SandboxPolicy::default(),
        ExecBounds::default(),
    );
    assert!(matches!(require, Err(SandboxError::Unavailable(_))));

    let auto = run_sandboxed_with_bounded(
        &UnavailableProbe,
        shell_spec("true"),
        SandboxablePreference::Auto,
        SandboxPolicy {
            allow_approval_fallback: true,
        },
        ExecBounds::default(),
    );
    assert!(matches!(auto, Err(SandboxError::NeedsApproval(_))));

    let auto_denied = run_sandboxed_with_bounded(
        &UnavailableProbe,
        shell_spec("true"),
        SandboxablePreference::Auto,
        SandboxPolicy::default(),
        ExecBounds::default(),
    );
    assert!(matches!(auto_denied, Err(SandboxError::Denied(_))));
}

/// `ExecBounds` 默认值与 agent-tools-exec 既有默认一致。
#[test]
fn default_bounds_match_exec_config() {
    let bounds = ExecBounds::default();
    assert_eq!(bounds.timeout, Duration::from_secs(30));
    assert_eq!(bounds.max_output_bytes, 30_000);
}

/// bwrap 路径同样受 bounds 约束，且 `--die-with-parent` 必须留在 PREFIX 里：
/// 直接子进程被 kill 后，沙箱内孙进程由 bwrap 随父死亡退出，不留下写管道。
///
/// 宿主不可用 bwrap（无 user namespace）时本用例无法验证，如实打印跳过原因。
#[test]
fn bwrap_path_is_bounded_and_leaves_no_descendants() {
    assert!(
        build_bwrap_argv(&shell_spec("true"), &[])
            .windows(2)
            .any(|w| w == ["--new-session", "--die-with-parent"]),
        "bwrap PREFIX must keep --die-with-parent"
    );
    if !agent_sandbox::probe_system_bwrap().is_usable() {
        eprintln!("skipped: host bwrap is not usable (no user namespace?)");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join("grandchild.pid");
    let spec = shell_spec(&format!(
        "echo started; echo $$ > '{}'; sleep 30",
        pid_file.display()
    ));
    let bounds = ExecBounds {
        timeout: Duration::from_millis(300),
        ..ExecBounds::default()
    };
    let started = Instant::now();
    let out = run_sandboxed_bounded(
        spec,
        SandboxablePreference::Require,
        SandboxPolicy::default(),
        bounds,
    )
    .expect("bwrap path runs");
    let elapsed = started.elapsed();

    assert!(out.timed_out, "timeout must be reported: {out:?}");
    assert_eq!(out.sandbox, SandboxType::LinuxSeccomp);
    assert!(elapsed < Duration::from_secs(3), "too late: {elapsed:?}");

    let pid: i32 = fs::read_to_string(&pid_file)
        .expect("grandchild pid recorded")
        .trim()
        .parse()
        .expect("numeric pid");
    // --die-with-parent 生效时该进程随 bwrap 一起消失；直接子进程已由 kill+wait 回收。
    let deadline = Instant::now() + Duration::from_millis(500);
    while PathBuf::from(format!("/proc/{pid}")).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "descendant {pid} survived the timeout kill"
    );
}
