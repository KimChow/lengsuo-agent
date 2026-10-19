//! exec 链端到端验收：只用 crate 的公开出口（`GuardedExec` / `evaluate` /
//! `Verdict` / `ExecError`）驱动真实命令，不 mock 判定与执行。
//!
//! 覆盖三层：argv 判定 → 沙箱有界执行 → 错误映射；外加一条「先判定后执行」的
//! 链路顺序断言。仅在 Unix 运行（用例驱动真实 `sh`/`env` 与 POSIX 进程语义）。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;
use agent_sandbox::{ExecBounds, ExecOutcome, SandboxPolicy, SandboxablePreference};
use agent_tools_exec::{evaluate, ExecError, GuardedExec, Verdict};

/// 策略构造：逐条追加前缀规则（与 guard 层单测同写法，这里走公开 API 重演）。
fn policy_with(rules: &[(&[&str], Decision)]) -> Policy {
    let mut policy = Policy::empty();
    for (prefix, decision) in rules {
        let prefix: Vec<String> = prefix.iter().map(|token| (*token).to_string()).collect();
        policy
            .add_prefix_rule(&prefix, *decision)
            .expect("前缀规则构造必须成功");
    }
    policy
}

/// argv 构造。
fn argv(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_string()).collect()
}

/// 装配一条 guard 链：沙箱偏好 `Forbid`（真实直跑，不依赖本机沙箱能力）。
fn guarded(policy: Policy, pref: SandboxablePreference) -> GuardedExec {
    GuardedExec {
        policy,
        pref,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds::default(),
    }
}

/// `PATH` 中是否存在该程序（用例据此跳过依赖系统程序的场景）。
fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 缺依赖时显式跳过，并让「跳过」在输出里可见（不静默通过）。
macro_rules! need {
    ($program:literal) => {
        if which($program).is_none() {
            eprintln!("跳过：本机 PATH 中没有 {}", $program);
            return;
        }
    };
}

/// stdout 转字符串（执行层给的是字节）。
fn stdout_of(outcome: &ExecOutcome) -> String {
    String::from_utf8_lossy(&outcome.stdout).into_owned()
}

/// 标记文件路径：用绝对路径，避免子进程 cwd 影响解析。
fn marker_path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

/// 1. allow 规则 + `Forbid` 偏好 ⇒ 真实跑 `["echo", "chain-ok"]` 并拿到 stdout。
#[test]
fn allow_plus_forbid_runs_echo_and_returns_stdout() {
    need!("echo");

    let exec = guarded(
        policy_with(&[(&["echo", "chain-ok"], Decision::Allow)]),
        SandboxablePreference::Forbid,
    );

    let outcome = exec
        .run(&argv(&["echo", "chain-ok"]), None)
        .expect("allow + Forbid 必须真实执行");

    assert!(outcome.status.success, "echo 应正常退出: {outcome:?}");
    assert_eq!(stdout_of(&outcome).trim(), "chain-ok");
    assert!(!outcome.timed_out, "短命令不该判定超时");
}

/// 拒绝路径共用断言：`touch <标记>` 被判为 `expect` 对应的错误，且标记未产生。
///
/// 三条拒绝用例（deny / ask / 无规则命中）只差策略与期望错误，共用这一处；
/// 「文件确实没被创建」是它们的共同实质断言，不因合并而弱化。
fn assert_refused_without_side_effect(policy: Policy, name: &str, expect: fn(&ExecError) -> bool) {
    let dir = tempfile::tempdir().expect("临时目录");
    let marker = marker_path(&dir, name);

    let exec = guarded(policy, SandboxablePreference::Forbid);
    let result = exec.run(&argv(&["touch", marker.to_str().expect("标记路径")]), None);

    let error = result.expect_err("该命令不得被执行");
    assert!(expect(&error), "{name} 的错误变体不符: {error:?}");
    assert!(!marker.exists(), "{name}: 被拒绝的命令不得产生任何副作用");
}

/// 2. deny 规则 ⇒ `Denied`，且命令从未被执行（标记文件未产生）。
#[test]
fn deny_is_refused_and_leaves_no_side_effect() {
    need!("touch");
    assert_refused_without_side_effect(
        policy_with(&[(&["touch"], Decision::Deny)]),
        "deny-marker",
        |error| matches!(error, ExecError::Denied(_)),
    );
}

/// 3. ask 规则 ⇒ `NeedsApproval`，同样不 spawn。
#[test]
fn ask_is_refused_as_needing_approval_without_spawning() {
    need!("touch");
    assert_refused_without_side_effect(
        policy_with(&[(&["touch"], Decision::Ask)]),
        "ask-marker",
        |error| matches!(error, ExecError::NeedsApproval(_)),
    );
}

/// 3b. 无规则命中 ⇒ `NeedsApproval`，不 spawn（与 ask 同出口、不同来路）。
#[test]
fn unmatched_command_needs_approval_without_spawning() {
    need!("touch");
    assert_refused_without_side_effect(
        policy_with(&[(&["git", "status"], Decision::Allow)]),
        "unmatched-marker",
        |error| matches!(error, ExecError::NeedsApproval(_)),
    );
}

/// 4. 超时命令受 `bounds` 约束，且 `timed_out` 为真。
#[test]
fn timeout_is_bounded_and_flagged_timed_out() {
    need!("sleep");

    let mut exec = guarded(
        policy_with(&[(&["sleep"], Decision::Allow)]),
        SandboxablePreference::Forbid,
    );
    exec.bounds = ExecBounds {
        timeout: Duration::from_millis(200),
        ..ExecBounds::default()
    };

    let started = std::time::Instant::now();
    let outcome = exec
        .run(&argv(&["sleep", "5"]), None)
        .expect("超时应返回而非挂死");
    let elapsed = started.elapsed();

    assert!(outcome.timed_out, "超时必须置 timed_out: {outcome:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "超时必须受 bounds 约束，实际耗时 {elapsed:?}"
    );
}

/// 5. 环境清洗对经本链执行的子进程同样生效。
///
/// 隔离手段与 `tests/exec.rs` 一致：不在本进程改宿主全局环境（同机并行用例会
/// 互相污染），而是重新拉起本测试二进制的一个子进程，用 `Command::env` 只给那个
/// 子进程注入四键的大小写变种与若干普通变量；子进程再经 `GuardedExec` **真实**
/// 跑 `env`，从其输出里核对结果。
///
/// 注意直跑路径（`Forbid`）底下还有沙箱层的 env 白名单，未被白名单放行的普通
/// 变量同样到不了子进程。故「普通变量仍在」这条只对白名单内的键成立——探针选
/// `PATH` 与 `LANG`，它们既在清洗层的保留集合里、也在白名单里，任一环误删都会
/// 被抓到；用任意的自造变量名反而会因白名单而被误判为「清洗范围过大」。
#[cfg(test)]
mod env_scrub {
    use agent_execpolicy::Decision;
    use agent_sandbox::{ExecBounds, SandboxPolicy, SandboxablePreference};
    use agent_tools_exec::GuardedExec;

    use super::{argv, policy_with, stdout_of};

    /// 子进程模式开关：只有被本用例重新拉起的测试进程才带此变量。
    const CHILD_MODE: &str = "AGENT_TOOLS_EXEC_GUARDED_ENV_SCRUB_CHILD_MODE";

    /// 本用例名：重新拉起子进程时用 `--exact` 只跑它。
    const TEST_NAME: &str = "env_scrub::guarded_child_cannot_see_uninheritable_keys";

    /// 注入隔离子进程的键值：四键各覆盖大写/首字母大写/全小写三种写法（值不同，
    /// 便于区分是哪一条漏了），外加一个必须保留的普通变量。
    const INJECTED: &[(&str, &str)] = &[
        ("CONVERSATION_MESSAGE_ID", "conversation"),
        ("Conversation_Message_Id", "conversation-mixed"),
        ("conversation_message_id", "conversation-lower"),
        ("ROOT_REQUEST_ID", "root"),
        ("Root_Request_Id", "root-mixed"),
        ("TOOL_CALL_ID", "tool"),
        ("Tool_Call_Id", "tool-mixed"),
        ("TRACE_ID", "trace"),
        ("Trace_Id", "trace-mixed"),
        ("trace_id", "trace-lower"),
        ("LANG", "C"),
    ];

    /// 子进程侧探针：经本链真实执行 `env`，由测试侧解析其原始输出。
    fn run_env_through_chain() -> String {
        let exec = GuardedExec {
            policy: policy_with(&[(&["env"], Decision::Allow)]),
            pref: SandboxablePreference::Forbid,
            sandbox_policy: SandboxPolicy::default(),
            bounds: ExecBounds {
                timeout: std::time::Duration::from_secs(10),
                max_output_bytes: 64 * 1024,
            },
        };
        let outcome = exec.run(&argv(&["env"]), None).expect("env 必须真实执行");
        assert!(outcome.status.success, "env 应正常退出: {outcome:?}");
        assert!(!outcome.truncated, "env 输出不该被截断，否则断言不完整");
        stdout_of(&outcome)
    }

    /// 子进程模式：断言 `env` 的原始输出里看不到四键的任何写法。
    fn assert_child_environment_is_scrubbed() {
        let output = run_env_through_chain();

        let bounds = [
            "CONVERSATION_MESSAGE_ID",
            "ROOT_REQUEST_ID",
            "TOOL_CALL_ID",
            "TRACE_ID",
        ];
        let mut leaked = Vec::new();
        let mut has_path = false;
        let mut lang_c = false;
        for line in output.lines() {
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            // 只记录键名，绝不回显值（值本身也不断言）。
            if bounds.iter().any(|key| key.eq_ignore_ascii_case(name)) {
                leaked.push(name.to_string());
            }
            has_path |= name == "PATH" && !value.is_empty();
            if name == "LANG" {
                assert_eq!(value, "C", "普通变量的值被篡改");
                lang_c = true;
            }
        }

        assert!(
            leaked.is_empty(),
            "子进程环境里仍有不可继承键（仅列键名）: {leaked:?}"
        );
        // 反向证明清洗/白名单没有把环境整个清空：必需变量照常可用。
        assert!(has_path, "PATH 丢失，子命令将无法解析程序名");
        assert!(lang_c, "LANG 未透传，白名单误伤普通变量");
    }

    /// 阻塞等待子进程退出，超时即杀掉并失败（测试不许无限挂住）。
    fn wait_bounded(mut child: std::process::Child, label: &str) -> std::process::Output {
        let deadline = std::time::Duration::from_secs(60);
        let started = std::time::Instant::now();
        loop {
            match child.try_wait().expect("轮询子进程退出状态") {
                Some(_) => return child.wait_with_output().expect("读取子进程输出"),
                None if started.elapsed() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{label} 超时未退出");
                }
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    }

    #[test]
    fn guarded_child_cannot_see_uninheritable_keys() {
        if std::env::var_os(CHILD_MODE).is_some() {
            assert_child_environment_is_scrubbed();
            return;
        }

        let exe = std::env::current_exe().expect("定位测试二进制");
        let mut command = std::process::Command::new(exe);
        command
            .args([TEST_NAME, "--exact", "--nocapture"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .env(CHILD_MODE, "1")
            .envs(INJECTED.iter().copied());
        let child = command.spawn().expect("拉起隔离子测试进程");
        let output = wait_bounded(child, "隔离子测试进程");
        assert!(
            output.status.success(),
            "隔离子进程失败\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// 6. 链路顺序：deny 时**先判定后执行**。
///
/// 用一条「若被执行就必然留下痕迹」的破坏性命令（`rm -f` 一个真实存在的文件），
/// 规则判 deny。判定在先 ⇒ 文件完好；若实现颠倒成先执行后判定，文件必被删。
/// 这里不断言错误文本，只断言顺序本身。
#[test]
fn deny_is_decided_before_any_execution_ordering() {
    need!("rm");

    let dir = tempfile::tempdir().expect("临时目录");
    let victim = marker_path(&dir, "victim.txt");
    std::fs::write(&victim, b"still-here").expect("写入受害文件");
    assert!(victim.exists(), "前置条件：受害文件必须存在");

    let policy = policy_with(&[(&["rm"], Decision::Deny)]);
    let command = argv(&["rm", "-f", victim.to_str().expect("路径")]);
    // 判定层独立复核：这一条确实该 deny。
    assert!(
        matches!(evaluate(&policy, &command), Verdict::Deny(_)),
        "前置条件：该命令必须被判 deny"
    );

    let exec = guarded(policy, SandboxablePreference::Forbid);
    let result = exec.run(&command, None);

    assert!(
        matches!(result, Err(ExecError::Denied(_))),
        "deny 必须映射为 Denied: {result:?}"
    );
    assert!(
        victim.exists(),
        "破坏性命令被执行了：判定发生在执行之后（顺序颠倒）"
    );
    assert_eq!(
        std::fs::read(&victim).expect("读回受害文件"),
        b"still-here",
        "受害文件内容被改动"
    );
}

/// 补充：`evaluate` 的公开语义在端到端用例里被直接观察（判定 → 执行的分界）。
#[test]
fn evaluate_is_observable_through_the_public_exit() {
    let policy = policy_with(&[
        (&["git"], Decision::Allow),
        (&["git", "push"], Decision::Deny),
    ]);

    assert_eq!(evaluate(&policy, &argv(&["git", "status"])), Verdict::Allow);
    assert!(matches!(
        evaluate(&policy, &argv(&["git", "push", "origin", "main"])),
        Verdict::Deny(_)
    ));
    assert!(matches!(
        evaluate(&policy, &argv(&["cargo", "publish"])),
        Verdict::NeedsApproval(_)
    ));
    // 内层命令不可见 ⇒ 外层 allow 不适用（不因外层规则放行内联脚本）。
    let shell_policy = policy_with(&[(&["sh"], Decision::Allow)]);
    assert!(matches!(
        evaluate(&shell_policy, &argv(&["sh", "-c", "echo hi"])),
        Verdict::NeedsApproval(_)
    ));
}

/// 补充：cwd 透传到真实子进程（执行层参数确实生效，而非被忽略）。
#[test]
fn cwd_reaches_the_real_child() {
    need!("pwd");

    let dir = tempfile::tempdir().expect("临时目录");
    // canonicalize：避免 /tmp 符号链接让字符串比较不等。
    let canonical: &Path = &dir.path().canonicalize().expect("规范化临时目录");

    let exec = guarded(
        policy_with(&[(&["pwd"], Decision::Allow)]),
        SandboxablePreference::Forbid,
    );
    let outcome = exec
        .run(&argv(&["pwd"]), Some(canonical))
        .expect("pwd 必须真实执行");

    assert!(outcome.status.success, "pwd 应正常退出: {outcome:?}");
    assert_eq!(
        stdout_of(&outcome).trim(),
        canonical.to_string_lossy().trim_end()
    );
}
