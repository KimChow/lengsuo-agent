//! W3b「PTY 启动路径 argv 策略门」验收测试（Linux 真伪终端）。
//!
//! 覆盖：
//! - deny 规则下不建 PTY、不起子进程（标记文件 + `/proc` 进程探测双证）；
//! - allow 规则下正常启动并可读写输出；
//! - `bash -c '…'` 内层命令不可见 ⇒ 保守 Ask，外层 allow 不能放行；
//! - 无规则 / 大小写变体 / 前缀边界 / 无法分词 ⇒ Ask；
//! - 未走 `open_guarded` 的既有入口不受策略约束（残余缺口，显式钉住）。
//!
//! 所有异步等待都带超时；只操作本用例自己创建的会话，不杀无关进程。
#![cfg(unix)]

use std::time::Duration;

use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;
use agent_pty::{PtyConfig, PtyError, PtyManager, PtySession};

/// 测试用等待上限。
const WAIT: Duration = Duration::from_secs(20);

/// 本二进制内串行创建真 pty（与 `tests/pty.rs` 同构，避免争抢 pty 资源）。
static PTY_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct PtyGate;

impl PtyGate {
    fn acquire() -> Self {
        use std::sync::atomic::Ordering;
        while PTY_BUSY
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::yield_now();
        }
        PtyGate
    }
}

impl Drop for PtyGate {
    fn drop(&mut self) {
        PTY_BUSY.store(false, std::sync::atomic::Ordering::Release);
    }
}

fn cfg(line: &str) -> PtyConfig {
    PtyConfig {
        shell: format!("bash --norc --noprofile -c {line:?}"),
        ..Default::default()
    }
}

fn policy_with(prefix: &[&str], decision: Decision) -> Policy {
    let mut policy = Policy::empty();
    let prefix = prefix.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    policy.add_prefix_rule(&prefix, decision).unwrap();
    policy
}

async fn open_guarded(cfg: PtyConfig, policy: &Policy) -> Result<PtySession, PtyError> {
    tokio::time::timeout(WAIT, PtySession::open_guarded(cfg, policy))
        .await
        .expect("open_guarded 超时")
}

/// 读输出直到出现 `needle`；超时即失败。
async fn read_until(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>, needle: &[u8]) -> Vec<u8> {
    let mut acc = Vec::new();
    loop {
        if acc.windows(needle.len()).any(|w| w == needle) {
            return acc;
        }
        match tokio::time::timeout(WAIT, rx.recv()).await {
            Ok(Some(chunk)) => acc.extend(chunk),
            Ok(None) => panic!("输出流在出现 {needle:?} 前关闭，实际={acc:?}"),
            Err(_) => panic!("等待 {needle:?} 超时，实际={acc:?}"),
        }
    }
}

/// 在 `/proc/<pid>/cmdline` 中查找唯一 token：确认没有对应子进程。
fn process_with_token(token: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        if cmdline
            .windows(token.len())
            .any(|window| window == token.as_bytes())
        {
            return true;
        }
    }
    false
}

fn unique_token(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("w3b-{tag}-{}-{nanos}", std::process::id())
}

// ---- deny：不建 PTY、不起子进程（标记文件 + 进程探测双证） ----

#[tokio::test]
async fn deny_rule_does_not_spawn_child_or_touch_marker() {
    let _gate = PtyGate::acquire();
    let token = unique_token("deny");
    let marker = std::env::temp_dir().join(format!("{token}.marker"));
    let _ = std::fs::remove_file(&marker);
    let line = format!("touch {}; sleep 30 # {token}", marker.display());

    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&["bash".to_string()], Decision::Deny)
        .unwrap();

    let mut mgr = PtyManager::new();
    let res = tokio::time::timeout(WAIT, mgr.create_guarded(cfg(&line), &policy))
        .await
        .expect("create_guarded 超时");
    assert!(
        matches!(res, Err(PtyError::PolicyDenied(_))),
        "deny 必须返回 PolicyDenied，实际={res:?}"
    );
    assert!(!marker.exists(), "deny 后子进程不得执行（标记文件出现）");
    assert!(
        !process_with_token(&token),
        "deny 后不得存在带唯一 token 的子进程"
    );
    assert!(mgr.list().is_empty(), "deny 不得留下会话（未创建 PTY）");
}

// ---- allow：正常启动、可写可读 ----

#[tokio::test]
async fn allow_rule_launches_and_reads_output() {
    let _gate = PtyGate::acquire();
    let policy = policy_with(&["bash"], Decision::Allow);
    let cfg = PtyConfig {
        shell: "bash --norc --noprofile".to_string(),
        ..Default::default()
    };
    let mut session = open_guarded(cfg, &policy).await.expect("allow 应放行");
    let mut rx = session.take_output().await.expect("输出流可取回");
    tokio::time::timeout(WAIT, session.write(b"echo W3B-ALLOW-42-OK\r"))
        .await
        .expect("write 超时")
        .expect("write 应成功");
    let out = read_until(&mut rx, b"W3B-ALLOW-42-OK").await;
    assert!(
        out.windows(b"W3B-ALLOW-42-OK".len())
            .any(|w| w == b"W3B-ALLOW-42-OK"),
        "allow 会话未读到回显，实际={out:?}"
    );
    tokio::time::timeout(WAIT, session.kill())
        .await
        .expect("kill 超时")
        .expect("kill 应成功");
}

// ---- bash -c：内层命令不可见，外层 allow 不得当作已审核 ----

#[tokio::test]
async fn bash_c_requires_approval_even_with_outer_allow() {
    let _gate = PtyGate::acquire();
    // 外层允许 bash -c；内层 `rm -rf` 之类对策略不可见，必须仍然 Ask。
    let policy = policy_with(&["bash", "-c"], Decision::Allow);
    let res = open_guarded(cfg("echo inner-not-audited"), &policy).await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "bash -c 必须保守返回 NeedsApproval，实际={res:?}"
    );
}

#[tokio::test]
async fn lowercase_and_uppercase_shell_variants_require_approval() {
    let _gate = PtyGate::acquire();
    let policy = policy_with(&["bash", "-c"], Decision::Allow);
    let cfg = PtyConfig {
        shell: "BASH -c \"echo case-variant\"".to_string(),
        ..Default::default()
    };
    let res = open_guarded(cfg, &policy).await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "大小写变体不得借用 bash 的 allow（且大小写不敏感识别为内层不可见），实际={res:?}"
    );
}

// ---- 无规则 / 前缀边界 ----

#[tokio::test]
async fn no_matching_rule_needs_approval() {
    let _gate = PtyGate::acquire();
    let policy = Policy::empty();
    let res = open_guarded(cfg("echo no-rule"), &policy).await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "无规则必须 fail-closed 返回 NeedsApproval，实际={res:?}"
    );
}

#[tokio::test]
async fn prefix_boundary_and_case_variants_need_approval() {
    let _gate = PtyGate::acquire();
    let policy = policy_with(&["sleep", "5"], Decision::Allow);

    // 精确前缀命中 ⇒ Allow（真起 `sleep 5`，随后立即 kill）。
    let mut ok = open_guarded(
        PtyConfig {
            shell: "sleep 5".to_string(),
            ..Default::default()
        },
        &policy,
    )
    .await
    .expect("精确前缀应放行");
    tokio::time::timeout(WAIT, ok.kill())
        .await
        .expect("kill 超时")
        .expect("kill 应成功");

    // token 边界：`sleep 50` 不命中 `sleep 5`。
    let res = open_guarded(
        PtyConfig {
            shell: "sleep 50".to_string(),
            ..Default::default()
        },
        &policy,
    )
    .await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "前缀边界必须按 token 比较，实际={res:?}"
    );

    // 大小写变体不是同一 token。
    let res = open_guarded(
        PtyConfig {
            shell: "SLEEP 5".to_string(),
            ..Default::default()
        },
        &policy,
    )
    .await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "大小写变体不得命中小写规则，实际={res:?}"
    );

    // 模式长于命令 ⇒ 不命中。
    let long = policy_with(&["git", "status", "--short"], Decision::Allow);
    let res = open_guarded(
        PtyConfig {
            shell: "git status".to_string(),
            ..Default::default()
        },
        &long,
    )
    .await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "模式长于命令不得命中，实际={res:?}"
    );
}

// ---- 错误文本不得回显环境值或凭据 ----

#[tokio::test]
async fn approval_error_does_not_echo_env_or_credentials() {
    let _gate = PtyGate::acquire();
    let secret = "w3b-super-secret-value";
    let arg_secret = "w3b-arg-token-abc";
    let mut config = PtyConfig {
        shell: format!("sleep 30 --token={arg_secret}"),
        ..Default::default()
    };
    config
        .env
        .insert("W3B_SECRET".to_string(), secret.to_string());
    let res = open_guarded(config, &Policy::empty()).await;
    let Err(PtyError::NeedsApproval(message)) = res else {
        panic!("应返回 NeedsApproval，实际={res:?}");
    };
    assert!(
        !message.contains(secret),
        "错误文本回显了环境变量值: {message}"
    );
    assert!(
        !message.contains(arg_secret),
        "错误文本回显了 argv 参数: {message}"
    );
}

// ---- 无法分词 ⇒ fail-closed Ask ----

#[tokio::test]
async fn unparsable_command_line_needs_approval() {
    let _gate = PtyGate::acquire();
    let cfg = PtyConfig {
        shell: "bash -c \"unterminated".to_string(),
        ..Default::default()
    };
    let res = open_guarded(cfg, &Policy::empty()).await;
    assert!(
        matches!(res, Err(PtyError::NeedsApproval(_))),
        "无法分词必须 fail-closed，实际={res:?}"
    );
}

// ---- 残余缺口：未走 open_guarded 的入口不受策略约束 ----

#[tokio::test]
async fn unguarded_open_ignores_policy() {
    let _gate = PtyGate::acquire();
    // 同一个会在 guarded 下被 deny 的命令，走 open 时照常执行。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&["bash".to_string()], Decision::Deny)
        .unwrap();
    let _ = &policy; // 显式说明：open 不接收策略。

    let mut session = tokio::time::timeout(WAIT, PtySession::open(cfg("echo W3B-UNGUARDED-OK")))
        .await
        .expect("open 超时")
        .expect("未受策略约束的 open 应成功");
    let mut rx = session.take_output().await.expect("输出流");
    let out = read_until(&mut rx, b"W3B-UNGUARDED-OK").await;
    assert!(
        out.windows(b"W3B-UNGUARDED-OK".len())
            .any(|w| w == b"W3B-UNGUARDED-OK"),
        "open 应正常输出，实际={out:?}"
    );
}
