//! ExecRunner 验收：echo/cwd/退出码/超时/截断/PowerShell 不支持/无注入。
//!
//! 仅在 Unix 运行：用例驱动真实 `bash`（`allowed_shells: vec![Shell::Bash]`）。
#![cfg(unix)]

use std::time::Duration;

use agent_tools_exec::{ExecConfig, ExecRunner, Shell};

/// 测试用配置：短默认超时，小截断上限。
fn runner(cap: usize) -> ExecRunner {
    ExecRunner::new(ExecConfig {
        default_timeout: Duration::from_millis(500),
        max_output_bytes: cap,
        allowed_shells: vec![Shell::Bash],
    })
}

/// 等异步结果必须有超时：卡住的测试不能挂着 CI。
async fn with_deadline<T: Send + 'static>(f: impl std::future::Future<Output = T> + Send) -> T {
    tokio::time::timeout(Duration::from_secs(20), f)
        .await
        .expect("测试等待超时")
}

#[tokio::test]
async fn echo_captures_stdout_and_exit_zero() {
    let r = with_deadline(runner(1000).run(Shell::Bash, "echo hello", None, None))
        .await
        .expect("deadline");
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.stdout.trim(), "hello");
    assert!(r.stderr.is_empty());
    assert!(!r.timed_out);
    assert!(!r.truncated);
}

#[tokio::test]
async fn stderr_is_kept_separate_from_stdout() {
    let cmd = "echo out; echo err >&2";
    let r = with_deadline(runner(1000).run(Shell::Bash, cmd, None, None))
        .await
        .expect("deadline");
    assert_eq!(r.stdout.trim(), "out");
    assert_eq!(r.stderr.trim(), "err");
}

#[tokio::test]
async fn nonzero_exit_code_is_reported() {
    let r = with_deadline(runner(1000).run(Shell::Bash, "exit 7", None, None))
        .await
        .expect("deadline");
    assert_eq!(r.exit_code, 7);
    assert!(!r.timed_out);
}

#[tokio::test]
async fn cwd_is_applied_to_the_child() {
    let dir = tempfile::tempdir().expect("临时目录");
    let canonical = dir.path().canonicalize().expect("规范化临时目录");
    let r = with_deadline(runner(1000).run(Shell::Bash, "pwd", Some(&canonical), None))
        .await
        .expect("deadline");
    assert_eq!(r.exit_code, 0, "stderr: {:?}", r.stderr);
    // cwd 用 canonicalize 传入，避免 /tmp 符号链接造成字符串不等。
    assert_eq!(r.stdout.trim(), canonical.to_string_lossy().trim_end());
}

#[tokio::test]
async fn sleep_past_the_timeout_reports_timed_out() {
    let cfg = runner(1000);
    let r = with_deadline(cfg.run(
        Shell::Bash,
        "sleep 5",
        None,
        Some(Duration::from_millis(100)),
    ))
    .await
    .expect("run");
    assert!(r.timed_out, "sleep 5 + 100ms 超时必须置 timed_out");
    assert_eq!(r.exit_code, -1);
    assert!(r.duration_ms < 5_000, "不该等满 5 秒: {}ms", r.duration_ms);
}

#[tokio::test]
async fn timeout_keeps_output_produced_before_the_deadline() {
    let cmd = "echo first; sleep 5";
    let r =
        with_deadline(runner(1000).run(Shell::Bash, cmd, None, Some(Duration::from_millis(300))))
            .await
            .expect("run");
    assert!(r.timed_out);
    assert_eq!(
        r.stdout.trim(),
        "first",
        "超时前的输出必须保留: {:?}",
        r.stdout
    );
}

#[tokio::test]
async fn timeout_kills_the_whole_process_group() {
    // 后台后代在 400ms 后落一个标记文件；它同时持有 stdout 写端。
    // 只杀直接子进程 ⇒ 标记文件出现、管道不 EOF；杀干净整组 ⇒ 标记不存在。
    // （不用 pgrep：并行任务线在同一台机器上跑，全局匹配不可靠。）
    let dir = tempfile::tempdir().expect("临时目录");
    let marker = dir.path().join("survived");
    let cmd = format!(
        "( sleep 0.4; echo alive > '{}' ) & sleep 30",
        marker.display()
    );
    let r =
        with_deadline(runner(1000).run(Shell::Bash, &cmd, None, Some(Duration::from_millis(100))))
            .await
            .expect("run");
    assert!(r.timed_out);
    assert!(
        !marker.exists(),
        "后台后代活了下来：进程组没有被 SIGKILL 覆盖"
    );
}

#[tokio::test]
async fn output_is_truncated_at_the_byte_cap() {
    let cmd = "printf 'x%.0s' $(seq 1 3000)";
    let cap = 100;
    let r = with_deadline(runner(cap).run(Shell::Bash, cmd, None, None))
        .await
        .expect("deadline");
    assert!(r.truncated);
    assert_eq!(r.stdout.len(), cap, "stdout 必须正好截到上限");
    assert!(r.stdout.chars().all(|c| c == 'x'));
}

#[tokio::test]
async fn truncation_holds_even_across_many_chunks() {
    // head -c 64000 远大于 8 KiB 块：验证不会整体读进内存。
    let cmd = "head -c 64000 /dev/zero | tr '\\0' 'a'";
    let r = with_deadline(runner(4096).run(Shell::Bash, cmd, None, None))
        .await
        .expect("deadline");
    assert!(r.truncated);
    assert_eq!(r.stdout.len(), 4096);
}

#[tokio::test]
async fn a_bounded_child_is_not_mistaken_for_truncated() {
    let r = with_deadline(runner(30_000).run(Shell::Bash, "echo short", None, None))
        .await
        .expect("deadline");
    assert!(!r.truncated, "小输出不该置 truncated");
}

#[tokio::test]
async fn powershell_is_unsupported_on_linux() {
    let cfg = runner(1000);
    let err = cfg
        .run(Shell::PowerShell, "Get-Process", None, None)
        .await
        .expect_err("Linux 上 PowerShell 必须不支持");
    assert!(
        matches!(err, agent_tools_exec::ExecError::Unsupported(_)),
        "错误类型应为 Unsupported，实得 {err:?}"
    );
}

#[tokio::test]
async fn shell_not_in_allowed_shells_is_unsupported() {
    let cfg = ExecRunner::new(ExecConfig {
        default_timeout: Duration::from_millis(500),
        max_output_bytes: 1000,
        allowed_shells: Vec::new(),
    });
    let err = cfg
        .run(Shell::Bash, "echo hi", None, None)
        .await
        .expect_err("空白名单必须拒绝");
    assert!(matches!(err, agent_tools_exec::ExecError::Unsupported(_)));
}

#[tokio::test]
async fn command_text_is_passed_as_one_argv_without_host_side_expansion() {
    // 反引号/分号属于用户命令自身的 shell 语义：能生效说明没有被二次包裹。
    let cmd = "echo $(echo nested)";
    let r = with_deadline(runner(1000).run(Shell::Bash, cmd, None, None))
        .await
        .expect("deadline");
    assert_eq!(r.stdout.trim(), "nested");

    // 我们绝不把命令拼进 shell 的 argv：单引号内容原样保留。
    let quoted = "printf '%s' 'a;b c'";
    let r = with_deadline(runner(1000).run(Shell::Bash, quoted, None, None))
        .await
        .expect("deadline");
    assert_eq!(r.stdout, "a;b c");
}

#[tokio::test]
async fn default_config_matches_the_frozen_spec() {
    let cfg = ExecConfig::default();
    assert_eq!(cfg.default_timeout, Duration::from_secs(30));
    assert_eq!(cfg.max_output_bytes, 30_000);
    assert_eq!(cfg.allowed_shells, vec![Shell::Bash]);
}

// ---------------------------------------------------------------------------
// 环境清洗：派生命令前剔除不可继承的宿主上下文键。
//
// 隔离手段：不在测试进程里改宿主全局环境（并行用例会互相污染），而是重新拉起
// 本测试二进制的一个子进程，用 `Command::env` 只给那个子进程注入四键的大小写
// 变种与一个普通变量；子进程再真实跑 `bash`，从 bash 自己的环境里核对结果。
// ---------------------------------------------------------------------------

/// 子进程模式开关：只有被本用例重新拉起的测试进程才带此变量。
const ENV_SCRUB_CHILD_MODE: &str = "AGENT_TOOLS_EXEC_ENV_SCRUB_CHILD_MODE";

/// 本用例名：重新拉起子进程时用 `--exact` 只跑它。
const ENV_SCRUB_TEST_NAME: &str = "scrubbed_environment_reaches_real_bash_child";

/// 注入隔离子进程的键值：四键的多种大小写写法 + 一个必须保留的普通变量。
const INJECTED_CHILD_ENV: &[(&str, &str)] = &[
    ("CONVERSATION_MESSAGE_ID", "conversation"),
    ("Conversation_Message_Id", "conversation-mixed"),
    ("conversation_message_id", "conversation-lower"),
    ("ROOT_REQUEST_ID", "root"),
    ("Root_Request_Id", "root-mixed"),
    ("TOOL_CALL_ID", "tool"),
    ("Tool_Call_Id", "tool-mixed"),
    ("TRACE_ID", "trace"),
    ("Trace_Id", "trace-mixed"),
    ("AGENT_TOOLS_EXEC_ENV_CANARY", "canary"),
];

/// 子进程侧探针：只打印「有没有」与键名，绝不打印任何环境值。
const ENV_SCRUB_PROBE: &str = r#"
printf 'leak_count=%s\n' "$(env | sed 's/=.*//' | grep -icxE 'conversation_message_id|root_request_id|tool_call_id|trace_id')"
printf 'canary=%s\n' "${AGENT_TOOLS_EXEC_ENV_CANARY:+present}"
printf 'path=%s\n' "${PATH:+present}"
printf 'home=%s\n' "${HOME:+present}"
printf 'leaked=%s\n' "$(env | sed 's/=.*//' | grep -ixE 'conversation_message_id|root_request_id|tool_call_id|trace_id' | tr 'A-Z' 'a-z' | sort -u | tr '\n' ',')"
"#;

/// 取探针输出里 `key=value` 的值。
fn probe_field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

/// 阻塞等待子进程退出，超时即杀掉并失败（测试不许无限挂住）。
fn wait_bounded(mut child: std::process::Child, label: &str) -> std::process::Output {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().expect("轮询子进程退出状态") {
            Some(_) => return child.wait_with_output().expect("读取子进程输出"),
            None if started.elapsed() > Duration::from_secs(60) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{label} 超时未退出");
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// 子进程模式：断言真实 `bash` 子进程的环境已被清洗，且必需变量仍在。
async fn assert_real_bash_child_environment_is_scrubbed() {
    let r = with_deadline(runner(4096).run(
        Shell::Bash,
        ENV_SCRUB_PROBE,
        None,
        Some(Duration::from_secs(10)),
    ))
    .await
    .expect("deadline");
    assert_eq!(r.exit_code, 0, "stderr: {:?}", r.stderr);
    assert!(
        !r.truncated,
        "探针输出不该被截断: {:?}",
        r.stdout.chars().take(80).collect::<String>()
    );
    assert_eq!(
        probe_field(&r.stdout, "leak_count"),
        Some("0"),
        "bash 子进程仍能读到不可继承键: leaked=[{}]",
        probe_field(&r.stdout, "leaked").unwrap_or("<无>")
    );
    assert_eq!(
        probe_field(&r.stdout, "leaked"),
        Some(""),
        "bash 子进程环境里仍有不可继承键（仅列键名）"
    );
    // 必需 shell 环境与普通变量必须照常可用。
    assert_eq!(probe_field(&r.stdout, "path"), Some("present"), "PATH 丢失");
    assert_eq!(probe_field(&r.stdout, "home"), Some("present"), "HOME 丢失");
    assert_eq!(
        probe_field(&r.stdout, "canary"),
        Some("present"),
        "普通变量被误删"
    );
}

#[tokio::test]
async fn scrubbed_environment_reaches_real_bash_child() {
    if std::env::var_os(ENV_SCRUB_CHILD_MODE).is_some() {
        assert_real_bash_child_environment_is_scrubbed().await;
        return;
    }

    let exe = std::env::current_exe().expect("定位测试二进制");
    let mut command = std::process::Command::new(exe);
    command
        .args([ENV_SCRUB_TEST_NAME, "--exact", "--nocapture"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env(ENV_SCRUB_CHILD_MODE, "1");
    for (key, value) in INJECTED_CHILD_ENV {
        command.env(key, value);
    }
    let child = command.spawn().expect("拉起隔离子测试进程");
    let output = wait_bounded(child, "隔离子测试进程");
    assert!(
        output.status.success(),
        "隔离子进程失败\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
