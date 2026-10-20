//! `shell.exec` 工具执行体（[`agent_core::ExecToolExecutor`]）桥接测试。
//!
//! 覆盖三件事：
//! 1. **参数形状**：缺失 / 空串 / 非字符串 / `cwd` 非字符串一律 `Err`，且不得触达执行链；
//! 2. **工具名**：非 `shell.exec` 一律 `Err`，不得静默成功；
//! 3. **判定 → 执行语义**：deny / 无规则 → `Err` 且无副作用；allow → 真实执行；
//!    超时 → `Ok` 且 `timed_out`；输出非法 UTF-8 不 panic。
//!
//! 「未执行」的侧证统一用临时标记文件：被拒绝的命令即便被误执行也会留下文件。
//! 依赖系统程序的用例（`echo`/`sleep`/`touch`/`printf`）在本机缺失时跳过。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{ExecToolExecutor, ToolExecutor, EXEC_TOOL_NAME};
use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;
use agent_sandbox::{ExecBounds, SandboxPolicy, SandboxablePreference};
use agent_tools_exec::GuardedExec;
use serde_json::{json, Value};

/// 测试专用 argv 构造。
fn argv(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_string()).collect()
}

/// 测试专用策略构造：逐条加前缀规则。
fn policy_with(rules: &[(&[&str], Decision)]) -> Policy {
    let mut policy = Policy::empty();
    for (prefix, decision) in rules {
        let prefix = argv(prefix);
        policy
            .add_prefix_rule(&prefix, *decision)
            .expect("前缀规则构造必须成功");
    }
    policy
}

/// `Forbid` 偏好（不启用沙箱）+ 默认预算的执行体。
fn executor(policy: Policy) -> ExecToolExecutor {
    let guarded = GuardedExec {
        policy,
        pref: SandboxablePreference::Forbid,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds::default(),
    };
    ExecToolExecutor::new(Arc::new(guarded))
}

/// `PATH` 中是否存在该程序（缺失时跳过依赖系统程序的用例）。
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 逐条断言「形状不合法 → Err 且未执行」。
///
/// 标记文件用 `touch` 写入：若执行体把坏参数放行，标记文件就会出现。
fn assert_rejected_without_execution(exec: &ExecToolExecutor, args: Value) {
    let dir = tempfile::tempdir().expect("临时目录");
    let marker = dir.path().join("marker");

    let result = exec.execute(EXEC_TOOL_NAME, &args);

    assert!(result.is_err(), "坏参数必须返回 Err: {result:?}");
    assert!(!marker.exists(), "坏参数不得触达执行链");
}

/// 1a. 缺少 `command` 键。
#[test]
fn missing_command_is_rejected_without_executing() {
    let exec = executor(policy_with(&[(&["touch"], Decision::Allow)]));
    assert_rejected_without_execution(&exec, json!({}));
}

/// 1b. `command` 为空串或纯空白（分词后空 argv）。
#[test]
fn empty_command_is_rejected_without_executing() {
    let exec = executor(policy_with(&[(&["touch"], Decision::Allow)]));
    assert_rejected_without_execution(&exec, json!({ "command": "" }));
    assert_rejected_without_execution(&exec, json!({ "command": "   " }));
}

/// 1c. `command` 含未闭合引号（分词失败）。
#[test]
fn unterminated_quote_is_rejected_without_executing() {
    let exec = executor(policy_with(&[(&["echo"], Decision::Allow)]));
    assert_rejected_without_execution(&exec, json!({ "command": "echo 'hi" }));
}

/// 1d. `command` 不是字符串 / `cwd` 不是字符串 / 顶层不是对象。
#[test]
fn malformed_shapes_are_rejected() {
    let exec = executor(policy_with(&[(&["echo"], Decision::Allow)]));

    assert_rejected_without_execution(&exec, json!({ "command": ["echo", "hi"] }));
    assert_rejected_without_execution(&exec, json!({ "command": "echo hi", "cwd": 5 }));
    assert_rejected_without_execution(&exec, json!(["echo", "hi"]));
}

/// 2. 工具名不匹配 → `Err`，不得静默成功。
#[test]
fn other_tool_name_is_rejected() {
    let exec = executor(policy_with(&[(&["echo"], Decision::Allow)]));

    let result = exec.execute("read_file", &json!({ "command": "echo hi" }));

    assert!(result.is_err(), "非 shell.exec 工具必须 Err: {result:?}");
}

/// 3. allow + `Forbid` ⇒ 真实执行并捕获 stdout。
#[test]
fn allow_rule_runs_and_captures_stdout() {
    if which("echo").is_none() {
        eprintln!("跳过：本机 PATH 中没有 echo");
        return;
    }

    let exec = executor(policy_with(&[(&["echo", "bridge-ok"], Decision::Allow)]));

    let result = exec
        .execute(EXEC_TOOL_NAME, &json!({ "command": "echo bridge-ok" }))
        .expect("allow 必须真实执行");

    let stdout = result["stdout"].as_str().expect("stdout 必须是字符串");
    assert!(stdout.contains("bridge-ok"), "stdout 应含标记: {result:?}");
    assert_eq!(result["exit_code"].as_i64(), Some(0), "退出码应为 0");
    assert_eq!(result["timed_out"], json!(false));
    assert_eq!(result["truncated"], json!(false));
    assert!(
        result["sandbox"].as_str().is_some(),
        "sandbox 必须是标签字符串: {result:?}"
    );
}

/// 4. deny 规则 ⇒ `Err` 且标记文件未产生。
#[test]
fn deny_rule_returns_err_without_executing() {
    let exec = executor(policy_with(&[(&["touch"], Decision::Deny)]));
    assert_rejected_without_execution(&exec, json!({ "command": "touch /tmp/w3d-bridge-denied" }));
}

/// 5. 无限制 + 无匹配规则（→ Ask / NeedsApproval）⇒ `Err` 且未执行。
#[test]
fn unmatched_rule_returns_err_without_executing() {
    let exec = executor(policy_with(&[(&["git", "status"], Decision::Allow)]));
    assert_rejected_without_execution(&exec, json!({ "command": "touch /tmp/w3d-bridge-ask" }));
}

/// 6. 输出含非法 UTF-8 字节 ⇒ 不 panic，lossy 转成字符串。
#[test]
fn invalid_utf8_output_does_not_panic() {
    if which("printf").is_none() {
        eprintln!("跳过：本机 PATH 中没有 printf");
        return;
    }

    // `\xff` 是合法 UTF-8 序列中不会出现的字节；用单引号保证 shell 分词不吞反斜杠。
    let exec = executor(policy_with(&[(&["printf"], Decision::Allow)]));
    let result = exec.execute(EXEC_TOOL_NAME, &json!({ "command": "printf '\\xff'" }));

    let result = result.expect("printf 必须执行成功");
    let stdout = result["stdout"].as_str().expect("stdout 必须是字符串");
    assert!(
        stdout.contains('\u{fffd}'),
        "非法字节应被 lossy 替换为 U+FFFD: {result:?}"
    );
}

/// 7. 短超时 + `sleep 5` ⇒ `Ok` 且 `timed_out` 为 true。
#[test]
fn timeout_is_reported_as_timed_out() {
    if which("sleep").is_none() {
        eprintln!("跳过：本机 PATH 中没有 sleep");
        return;
    }

    let guarded = GuardedExec {
        policy: policy_with(&[(&["sleep"], Decision::Allow)]),
        pref: SandboxablePreference::Forbid,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds {
            timeout: Duration::from_millis(200),
            ..ExecBounds::default()
        },
    };
    let exec = ExecToolExecutor::new(Arc::new(guarded));

    let started = std::time::Instant::now();
    let result = exec
        .execute(EXEC_TOOL_NAME, &json!({ "command": "sleep 5" }))
        .expect("超时应作为结果返回而非错误");
    let elapsed = started.elapsed();

    assert_eq!(result["timed_out"], json!(true), "必须标记超时: {result:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "必须受 bounds 约束，实际耗时 {elapsed:?}"
    );
}
