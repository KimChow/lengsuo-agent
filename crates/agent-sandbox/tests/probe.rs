//! 真实探测的行为测试：探测必须跑通最小隔离命令才算可用。
//!
//! 全部用例只使用**临时目录里的假 bwrap 脚本**或**注入的假执行器**，
//! 不写用户文件、不触网、不依赖宿主机是否具备 user namespace。

#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use agent_sandbox::{
    build_bwrap_argv, probe_bwrap_with, probe_command_spec, run_sandboxed_with, BwrapProbe,
    CommandSpec, NetworkMode, ProbeExec, ProbeOutcome, ProbeRun, SandboxError, SandboxPolicy,
    SandboxProbe, SandboxType, SandboxablePreference, SystemProbeExec,
};

/// 把假 bwrap 脚本落到临时目录（可执行位 0o755），脚本本身不产生任何副作用。
fn write_fake_bwrap(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bwrap");
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake bwrap");
    let mut perms = fs::metadata(&path).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    (dir, path)
}

/// 用真实执行器跑假 bwrap，但**经 /bin/sh 执行脚本**。
///
/// 直接 execve 刚写出的脚本在多线程进程里会瞬时 ETXTBSY（Text file busy，
/// 宿主层工件，与探测逻辑无关）；execve 目标换成稳定的 /bin/sh 后可复现率归零，
/// 而 spawn/轮询/kill/wait/stderr 全部走的仍是 [`SystemProbeExec`] 真实实现。
struct ScriptExec;

impl ProbeExec for ScriptExec {
    fn run_probe(&self, script: &Path, args: &[String], timeout: Duration) -> ProbeRun {
        let mut argv = vec![script.to_string_lossy().into_owned()];
        argv.extend(args.iter().cloned());
        SystemProbeExec.run_probe(Path::new("/bin/sh"), &argv, timeout)
    }
}

fn probe_args() -> Vec<String> {
    build_bwrap_argv(&probe_command_spec(), &[])
}

/// 探测命令的形状：只读绑定 + 封网 + 清空 env，没有可写绑定/cwd（无用户文件与网络副作用）。
#[test]
fn probe_command_shape_has_no_side_effects() {
    let argv = probe_args();
    assert!(argv.iter().any(|a| a == "--unshare-net"), "{argv:?}");
    assert!(argv.iter().any(|a| a == "--clearenv"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--bind"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--chdir"), "{argv:?}");
    assert_eq!(argv.last().map(String::as_str), Some("/bin/true"));
}

/// 可执行文件存在且最小隔离命令退出 0 → 可用。
#[test]
fn probe_exit_zero_is_usable() {
    let (_dir, path) = write_fake_bwrap("exit 0\n");
    let probe = probe_bwrap_with(&ScriptExec, &path, Duration::from_millis(500));
    assert_eq!(probe, BwrapProbe::Usable);
    assert!(probe.is_usable());
    assert_eq!(probe.reason(), None);
}

/// 可执行文件存在但执行失败（权限不足）→ 不可用，且原因可读。
#[test]
fn probe_permission_failure_is_unusable() {
    let (_dir, path) =
        write_fake_bwrap("echo 'setting up uid map: Permission denied' >&2\nexit 1\n");
    let probe = probe_bwrap_with(&ScriptExec, &path, Duration::from_millis(500));
    let reason = probe.reason().expect("must be unusable");
    assert!(!probe.is_usable());
    assert!(reason.contains("user namespaces"), "{reason}");
    assert!(reason.contains("setting up uid map"), "{reason}");
}

/// 非命名空间类失败同样不可用（fail-closed：任何非零退出都不算可用）。
#[test]
fn probe_other_failure_is_unusable() {
    let (_dir, path) = write_fake_bwrap("echo 'bwrap: Unknown option --argv0' >&2\nexit 1\n");
    let probe = probe_bwrap_with(&ScriptExec, &path, Duration::from_millis(500));
    assert!(matches!(probe, BwrapProbe::Failed(_)), "{probe:?}");
    let reason = probe.reason().unwrap();
    assert!(reason.contains("exit code 1"), "{reason}");
}

/// 可执行文件不存在/不可执行 → 不可用（不是「找不到就算不可用」的路径，而是执行失败路径）。
#[test]
fn probe_missing_executable_is_unusable() {
    let probe = probe_bwrap_with(
        &SystemProbeExec,
        Path::new("/nonexistent/definitely-not-bwrap"),
        Duration::from_millis(200),
    );
    assert!(matches!(probe, BwrapProbe::Failed(_)), "{probe:?}");
}

/// 挂起的探测进程必须被总超时截断，并在返回前 kill + wait 回收（/proc/<pid> 消失）。
#[test]
fn probe_hang_times_out_and_reaps_process() {
    let (_dir, path) = write_fake_bwrap("exec sleep 30\n");
    let started = Instant::now();
    let run = ScriptExec.run_probe(&path, &probe_args(), Duration::from_millis(150));
    let elapsed = started.elapsed();

    assert_eq!(run.outcome, ProbeOutcome::TimedOut);
    assert!(run.pid > 0, "real executor must report the probe pid");
    assert!(
        elapsed < Duration::from_secs(2),
        "探测必须在总超时内返回，实际 {elapsed:?}"
    );
    assert!(
        !Path::new(&format!("/proc/{}", run.pid)).exists(),
        "探测进程 {} 未回收（仍是僵尸或仍在运行）",
        run.pid
    );
}

/// 孙进程攥着 stderr 写端时：探测不得被拖住，且退出状态照旧判定为不可用。
#[test]
fn probe_descendant_holding_stderr_does_not_block() {
    let (_dir, path) = write_fake_bwrap(
        "echo 'No permissions to create a new namespace' >&2\nsleep 1 &\nexit 1\n",
    );
    let started = Instant::now();
    let probe = probe_bwrap_with(&ScriptExec, &path, Duration::from_millis(100));
    let elapsed = started.elapsed();

    assert!(matches!(probe, BwrapProbe::Failed(_)), "{probe:?}");
    assert!(
        elapsed < Duration::from_secs(1),
        "探测被孙进程拖住：{elapsed:?}"
    );
    // 已缓冲的 stderr 仍应被读到（不越过 deadline 的前提下尽力读取）。
    let reason = probe.reason().unwrap();
    assert!(reason.contains("user namespaces"), "{reason}");
}

/// 偏好三态：Forbid 不额外探测；Auto/Require 各探测一次；失败一律不静默直跑。
#[test]
fn preference_gating_counts_probes() {
    struct CountingProbe {
        runs: AtomicUsize,
    }
    impl SandboxProbe for CountingProbe {
        fn bwrap_available(&self) -> bool {
            self.bwrap_probe().is_usable()
        }
        fn landlock_available(&self) -> bool {
            false
        }
        fn bwrap_probe(&self) -> BwrapProbe {
            self.runs.fetch_add(1, Ordering::SeqCst);
            BwrapProbe::Failed("injected: bwrap probe failed".to_string())
        }
    }

    let spec = CommandSpec {
        program: PathBuf::from("/bin/true"),
        ..CommandSpec::default()
    };

    // Forbid：明确不沙箱 → 直跑，且**一次探测都不发生**。
    let probe = CountingProbe {
        runs: AtomicUsize::new(0),
    };
    let out = run_sandboxed_with(
        &probe,
        spec.clone(),
        SandboxablePreference::Forbid,
        SandboxPolicy::default(),
    )
    .expect("Forbid must run unsandboxed");
    assert_eq!(out.sandbox, SandboxType::None);
    assert_eq!(probe.runs.load(Ordering::SeqCst), 0, "Forbid 不得探测");

    // Require + 探测失败 → Unavailable（fail-closed）。
    let probe = CountingProbe {
        runs: AtomicUsize::new(0),
    };
    let r = run_sandboxed_with(
        &probe,
        spec.clone(),
        SandboxablePreference::Require,
        SandboxPolicy::default(),
    );
    assert!(matches!(r, Err(SandboxError::Unavailable(_))), "{r:?}");
    assert_eq!(probe.runs.load(Ordering::SeqCst), 1);

    // Auto + 探测失败 + 允许审批 → NeedsApproval。
    let probe = CountingProbe {
        runs: AtomicUsize::new(0),
    };
    let r = run_sandboxed_with(
        &probe,
        spec.clone(),
        SandboxablePreference::Auto,
        SandboxPolicy {
            allow_approval_fallback: true,
        },
    );
    assert!(matches!(r, Err(SandboxError::NeedsApproval(_))), "{r:?}");
    assert_eq!(probe.runs.load(Ordering::SeqCst), 1);

    // Auto + 探测失败 + 禁审批 → Denied。
    let probe = CountingProbe {
        runs: AtomicUsize::new(0),
    };
    let r = run_sandboxed_with(
        &probe,
        spec,
        SandboxablePreference::Auto,
        SandboxPolicy::default(),
    );
    assert!(matches!(r, Err(SandboxError::Denied(_))), "{r:?}");
}

/// 端到端：真实执行器 + 假 bwrap（存在但执行失败）→ 三种偏好都不得直跑；
/// 报错信息带上探测原因，供上层审批/日志说明。
#[test]
fn executor_failure_blocks_execution_end_to_end() {
    struct ScriptProbe {
        path: PathBuf,
        timeout: Duration,
    }
    impl SandboxProbe for ScriptProbe {
        fn bwrap_available(&self) -> bool {
            self.bwrap_probe().is_usable()
        }
        fn landlock_available(&self) -> bool {
            false
        }
        fn bwrap_probe(&self) -> BwrapProbe {
            probe_bwrap_with(&ScriptExec, &self.path, self.timeout)
        }
    }

    // 程序本身是 /bin/true：若上层「静默直跑」就会成功，因此 Err 即证明没有直跑。
    let spec = CommandSpec {
        program: PathBuf::from("/bin/true"),
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        network: NetworkMode::Blocked,
        ..CommandSpec::default()
    };

    for (script, needle) in [
        (
            "echo 'loopback: Failed RTM_NEWADDR: Operation not permitted' >&2\nexit 1\n",
            "user namespaces",
        ),
        ("exec sleep 30\n", "timed out"),
    ] {
        let (_dir, path) = write_fake_bwrap(script);
        let probe = ScriptProbe {
            path,
            timeout: Duration::from_millis(150),
        };

        let r = run_sandboxed_with(
            &probe,
            spec.clone(),
            SandboxablePreference::Require,
            SandboxPolicy::default(),
        );
        match r {
            Err(SandboxError::Unavailable(msg)) => assert!(msg.contains(needle), "{msg}"),
            other => panic!("Require 必须 fail-closed：{other:?}"),
        }

        let r = run_sandboxed_with(
            &probe,
            spec.clone(),
            SandboxablePreference::Auto,
            SandboxPolicy {
                allow_approval_fallback: true,
            },
        );
        assert!(
            matches!(r, Err(SandboxError::NeedsApproval(_))),
            "Auto 必须转人工审批：{r:?}"
        );

        let r = run_sandboxed_with(
            &probe,
            spec.clone(),
            SandboxablePreference::Auto,
            SandboxPolicy::default(),
        );
        assert!(matches!(r, Err(SandboxError::Denied(_))), "{r:?}");
    }
}

/// WSL1 内核版本串各形态都识别为 WSL1（对照参考实现同名用例向量）。
#[test]
fn detects_wsl1_proc_version_formats() {
    assert!(agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 4.4.0-22621-Microsoft"
    ));
    assert!(agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 5.15.0-microsoft-standard-WSL1"
    ));
    assert!(agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 5.15.0-wsl-microsoft-standard-WSL1"
    ));
}

/// WSL2 / 原生 Linux 不得误判为 WSL1。
#[test]
fn does_not_treat_wsl2_or_native_linux_as_wsl1() {
    assert!(!agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-microsoft-standard-WSL2"
    ));
    assert!(!agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-wsl-microsoft-standard-WSL2"
    ));
    assert!(!agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 4.19.104-microsoft-standard"
    ));
    assert!(!agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 6.6.87.2-microsoft-standard-WSL3"
    ));
    assert!(!agent_sandbox::proc_version_indicates_wsl1(
        "Linux version 6.8.0"
    ));
}

/// 注入假执行器：探测失败路径无需真实进程即可复现（含 spawn 失败与超时）。
#[test]
fn injected_executor_failure_paths() {
    struct FixedExec(ProbeOutcome);
    impl ProbeExec for FixedExec {
        fn run_probe(
            &self,
            _program: &Path,
            _args: &[String],
            _timeout: Duration,
        ) -> agent_sandbox::ProbeRun {
            agent_sandbox::ProbeRun {
                pid: 4242,
                outcome: self.0.clone(),
            }
        }
    }

    let exec = FixedExec(ProbeOutcome::TimedOut);
    let probe = probe_bwrap_with(
        &exec,
        Path::new("/usr/bin/bwrap"),
        Duration::from_millis(10),
    );
    assert!(matches!(probe, BwrapProbe::TimedOut(_)), "{probe:?}");

    let exec = FixedExec(ProbeOutcome::SpawnFailed("EACCES".to_string()));
    let probe = probe_bwrap_with(
        &exec,
        Path::new("/usr/bin/bwrap"),
        Duration::from_millis(10),
    );
    assert!(matches!(probe, BwrapProbe::Failed(_)), "{probe:?}");
    assert!(probe.reason().unwrap().contains("EACCES"));
}
