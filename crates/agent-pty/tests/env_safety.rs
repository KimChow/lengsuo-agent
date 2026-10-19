//! W3「PTY 请求上下文环境清洗」验收测试（Linux 真伪终端）。
//!
//! 覆盖三件事：
//! 1. 父进程环境里的请求上下文键（含 ASCII 大小写变种）不得进入子进程；
//! 2. `PtyConfig.env` 显式覆盖也不得把同样的键重新注入；
//! 3. 普通变量（PATH/TERM/自定义）与显式覆盖照常生效；EOF/kill 后输出流按时关闭。
//!
//! 所有异步等待都带超时（AGENTS.md §4）。仅在 Unix 运行：依赖真实伪终端与 `bash`。
#![cfg(unix)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_pty::{PtyConfig, PtyError, PtyManager};

/// 测试用等待上限；真 PTY 在研究机上远快于此值。
const WAIT: Duration = Duration::from_secs(20);

/// PTY 用例串行闸（与 `tests/pty.rs` 同构）：本二进制内所有用例都要建真 pty，
/// 同时并发创建会争抢 pty/tty 资源；本文件还有进程级 env 读写，必须串行。
/// 非阻塞独占、不持锁跨 `await`，`Drop` 时释放。
static PTY_BUSY: AtomicBool = AtomicBool::new(false);

struct PtyGate;

impl PtyGate {
    fn acquire() -> Self {
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
        PTY_BUSY.store(false, Ordering::Release);
    }
}

/// 进程环境变量的设置/还原守卫：测试里塞入敏感键，退出时逐个还原。
struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn set(pairs: &[(&str, &str)]) -> Self {
        let mut saved = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            saved.push((key.to_string(), std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn cfg(line: &str) -> PtyConfig {
    PtyConfig {
        shell: format!("bash --norc --noprofile -c {line:?}"),
        ..Default::default()
    }
}

/// 收集到流关闭（子进程退出 + reader EOF）为止的全部输出；超时即失败。
async fn drain_until_closed(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Vec<u8> {
    let mut acc = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, rx.recv()).await {
            Ok(Some(chunk)) => acc.extend(chunk),
            Ok(None) => return acc,
            Err(_) => panic!("等待输出超时，输出流未正常关闭"),
        }
    }
}

/// 取 `ENVPROBE[` 与 `]` 之间的探针字符串（探针由子进程 printf 原样打印）。
fn probe_value(out: &[u8]) -> String {
    let text = String::from_utf8_lossy(out);
    let start = text.find("ENVPROBE[").expect("输出缺少 ENVPROBE[") + "ENVPROBE[".len();
    let end = text[start..].find(']').expect("探针缺少右括号") + start;
    text[start..end].to_string()
}

// ---- T1：继承环境与显式覆盖都被清洗；普通变量保留 ----

#[tokio::test]
async fn request_context_env_scrubbed_and_normal_env_kept() {
    let _gate = PtyGate::acquire();
    // 父进程环境：4 个请求上下文键（含大小写变种）+ 2 个普通变量。
    let _env = EnvGuard::set(&[
        ("CONVERSATION_MESSAGE_ID", "leak-conv"),
        ("root_request_id", "leak-root"),
        ("Tool_Call_Id", "leak-tool"),
        ("TRACE_ID", "leak-trace"),
        ("W3_PTY_KEEP_INHERITED", "keep-inherited"),
        ("TERM", "xterm-w3-probe"),
    ]);

    let mut mgr = PtyManager::new();
    let mut c = cfg(
        "printf 'ENVPROBE[%s]' \"A:${CONVERSATION_MESSAGE_ID-X1}|B:${root_request_id-X2}\
         |C:${Tool_Call_Id-X3}|D:${trace_id-X4}|E:${W3_PTY_KEEP_INHERITED-X5}\
         |F:${W3_PTY_KEEP_CFG-X6}|G:${PATH-X7}|H:${TERM-X8}\"",
    );
    // 显式覆盖试图重新注入敏感键（两种大小写），并叠加普通自定义变量。
    c.env.insert("trace_id".into(), "inject-trace".into());
    c.env
        .insert("conversation_message_id".into(), "inject-conv".into());
    c.env
        .insert("W3_PTY_KEEP_INHERITED".into(), "cfg-override".into());
    c.env.insert("W3_PTY_KEEP_CFG".into(), "keep-cfg".into());

    let id = tokio::time::timeout(WAIT, mgr.create(c))
        .await
        .unwrap()
        .unwrap();
    let mut rx = mgr.take_output(id).await.expect("输出流可取回");
    let out = drain_until_closed(&mut rx).await;
    let probe = probe_value(&out);

    // 敏感键（继承的大小写变种 + 显式覆盖）一律缺失 ⇒ 展开为哨兵值。
    assert!(
        probe.starts_with("A:X1|B:X2|C:X3|D:X4"),
        "请求上下文键未被清洗，实际探针={probe}"
    );
    // 普通变量保留：继承值被显式覆盖时以覆盖值为准，纯自定义键照常注入。
    assert!(
        probe.contains("E:cfg-override|F:keep-cfg"),
        "普通自定义变量未按预期保留，实际探针={probe}"
    );
    // PATH 与 TERM 照常保留（TERM 由本测试显式设定，便于精确断言）。
    assert!(!probe.contains("G:X7"), "PATH 不得被清洗，实际探针={probe}");
    assert!(probe.contains("G:/"), "PATH 应为真实路径，实际探针={probe}");
    assert!(
        probe.ends_with("H:xterm-w3-probe"),
        "TERM 不得被清洗，实际探针={probe}"
    );
}

// ---- T2：真实子进程 env 输出里没有任何敏感键名，且 PATH/TERM 在列 ----

#[tokio::test]
async fn child_env_dump_has_no_request_context_names() {
    let _gate = PtyGate::acquire();
    let _env = EnvGuard::set(&[
        ("CONVERSATION_MESSAGE_ID", "leak-conv"),
        ("Root_Request_Id", "leak-root"),
        ("tool_call_id", "leak-tool"),
        ("trace_id", "leak-trace"),
        ("TERM", "xterm-w3-probe"),
    ]);

    let mut mgr = PtyManager::new();
    let id = tokio::time::timeout(WAIT, mgr.create(cfg("env")))
        .await
        .unwrap()
        .unwrap();
    let mut rx = mgr.take_output(id).await.expect("输出流可取回");
    let out = drain_until_closed(&mut rx).await;
    let text = String::from_utf8_lossy(&out);

    let names: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').split_once('='))
        .map(|(name, _)| name)
        .collect();

    let sensitive = [
        "CONVERSATION_MESSAGE_ID",
        "ROOT_REQUEST_ID",
        "TOOL_CALL_ID",
        "TRACE_ID",
    ];
    let leaked: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| sensitive.iter().any(|s| name.eq_ignore_ascii_case(s)))
        .collect();
    assert!(leaked.is_empty(), "子进程 env 中残留敏感键: {leaked:?}");
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case("PATH")),
        "PATH 应保留，实际键集合={names:?}"
    );
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case("TERM")),
        "TERM 应保留，实际键集合={names:?}"
    );
}

// ---- T3：kill 后输出流按时关闭（EOF 路径），重复 kill 报 AlreadyDead ----

#[tokio::test]
async fn kill_closes_output_stream_within_timeout() {
    let _gate = PtyGate::acquire();
    let mut mgr = PtyManager::new();
    let id = tokio::time::timeout(WAIT, mgr.create(cfg("sleep 60")))
        .await
        .unwrap()
        .unwrap();
    let mut rx = mgr.take_output(id).await.expect("输出流可取回");

    tokio::time::timeout(WAIT, mgr.kill(id))
        .await
        .unwrap()
        .expect("kill 应成功");

    // 子进程退出 ⇒ reader 得到 EOF/EIO ⇒ 发送端 drop ⇒ recv() 返回 None。
    let closed = tokio::time::timeout(WAIT, async { while rx.recv().await.is_some() {} }).await;
    assert!(closed.is_ok(), "kill 后输出流未在超时内关闭");

    assert!(matches!(
        tokio::time::timeout(WAIT, mgr.kill(id)).await.unwrap(),
        Err(PtyError::AlreadyDead(_))
    ));
}
