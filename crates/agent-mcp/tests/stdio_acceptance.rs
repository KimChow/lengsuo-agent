//! Real Python fixture processes; no network or third-party server claim.
use agent_mcp::client::StdioServerCommand;
use agent_mcp::{ClientConfig, Framing, McpClient};
use serde_json::json;
use std::{sync::Arc, time::Duration};
fn config(framing: Framing) -> ClientConfig {
    let mode = if framing == Framing::ContentLength {
        "lsp"
    } else {
        "line"
    };
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp_fixture_server.py");
    let mut cfg = ClientConfig::stdio(
        StdioServerCommand::new("python3").with_args([script.to_str().unwrap(), mode]),
        framing,
    );
    // 请求超时同样留余量：并发/背压用例（20/50 个请求共用一个连接）在全量测试
    // 并行时会与其它 crate 抢 CPU，500ms 压线抖动。放宽只影响等待上限，不改变
    // 被测语义（实测 20 并发约 0.22s、50 并发背压约 0.22s，见
    // docs/mcp-stdio-tests-status.md）。
    cfg.request_timeout = Duration::from_secs(10);
    // 握手超时留充足余量：`cargo test --workspace` 会并行拉起大量用例，
    // 8 个 Python 子进程同时冷启动时 2s 会压线抖动（实测 --test-threads=1 稳定，
    // 并行偶发「连接结束，响应未送达」）。放宽只影响等待上限，不改变被测语义。
    cfg.handshake_timeout = Duration::from_secs(30);
    cfg
}
async fn connect(f: Framing) -> McpClient {
    tokio::time::timeout(Duration::from_secs(40), McpClient::connect(config(f)))
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn initialize_list_call_and_shutdown_both_framings() {
    for framing in [Framing::NewlineDelimited, Framing::ContentLength] {
        let mut c = connect(framing).await;
        assert!(c.has_transport());
        let listed = c.list_tools().await.unwrap();
        assert_eq!(listed.tools.len(), 3);
        let out = c
            .call_tool("echo", Some(json!({"text":"hello fixture"})))
            .await
            .unwrap();
        assert!(serde_json::to_string(&out)
            .unwrap()
            .contains("hello fixture"));
        c.ping().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), c.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(c.is_closed());
    }
}
#[tokio::test]
async fn concurrent_responses_are_correlated_by_id() {
    let c = Arc::new(connect(Framing::NewlineDelimited).await);
    let mut tasks = Vec::new();
    for i in 0..20 {
        let c = c.clone();
        tasks.push(tokio::spawn(async move {
            // 夹具的 `test/echo_index` 反序延迟是 `0.02 * (10 - index)`，index>=10
            // 会得到负 sleep 并让 Python 线程抛错；夹具不在本卡修改范围，因此把
            // index 折叠到 0..10，用唯一的 token 逐请求核对 id 关联。
            let index = i % 10;
            let token = format!("fake-{i}");
            let result = c
                .request(
                    "test/echo_index",
                    Some(json!({"index":index,"token":token})),
                )
                .await
                .unwrap();
            assert_eq!(result["index"], index);
            assert_eq!(result["echo"], token);
        }));
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(40), task)
            .await
            .unwrap()
            .unwrap();
    }
    let mut c = Arc::try_unwrap(c).ok().unwrap();
    c.shutdown().await.unwrap();
}
#[tokio::test]
async fn timed_out_connection_cannot_be_reused() {
    let mut cfg = config(Framing::NewlineDelimited);
    cfg.request_timeout = Duration::from_millis(30);
    let mut c = McpClient::connect(cfg).await.unwrap();
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        c.request("test/hang_then_respond", None)
    )
    .await
    .unwrap()
    .is_err());
    assert!(c.is_poisoned());
    assert!(c.ping().await.is_err());
    c.disconnect().await;
}
#[tokio::test]
async fn bad_frames_fail_closed_in_both_framings() {
    for framing in [Framing::NewlineDelimited, Framing::ContentLength] {
        let mut c = connect(framing).await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), c.request("test/bad_frame", None))
                .await
                .unwrap()
                .is_err()
        );
        assert!(c.is_poisoned());
        assert!(c.ping().await.is_err());
        c.disconnect().await;
    }
}
#[tokio::test]
async fn truncated_eof_rejects_pending_request() {
    let mut c = connect(Framing::NewlineDelimited).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), c.request("test/truncated", None))
            .await
            .unwrap()
            .is_err()
    );
    assert!(c.is_poisoned());
    c.disconnect().await;
}
#[tokio::test]
async fn unexpected_exit_does_not_hang_request() {
    let mut c = connect(Framing::NewlineDelimited).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(2), c.request("test/exit", None))
            .await
            .unwrap()
            .is_err()
    );
    assert!(c.is_closed());
    c.disconnect().await;
}
#[tokio::test]
async fn notifications_are_delivered_in_order() {
    let mut c = connect(Framing::NewlineDelimited).await;
    let mut rx = c.subscribe_notifications();
    c.request("test/notify", None).await.unwrap();
    for _ in 0..3 {
        let (method, _) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(method, "notifications/message");
    }
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn oversized_frames_poison_the_connection() {
    for f in [Framing::NewlineDelimited, Framing::ContentLength] {
        let mut cfg = config(f);
        cfg.max_frame_bytes = 512;
        let mut c = McpClient::connect(cfg).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), c.request("test/oversized", None))
                .await
                .unwrap()
                .is_err()
        );
        assert!(c.is_poisoned());
        assert!(c.ping().await.is_err());
        c.disconnect().await;
    }
}
#[cfg(unix)]
#[tokio::test]
async fn explicit_shutdown_reaps_fixture_process() {
    struct PidFile(std::path::PathBuf);
    impl Drop for PidFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = PidFile(
        std::env::temp_dir().join(format!("mcp-test-child-{}-{stamp}.pid", std::process::id())),
    );
    let mut cfg = config(Framing::NewlineDelimited);
    cfg.command = cfg
        .command
        .with_env("LENGSUO_TEST_PID_FILE", file.0.to_str().unwrap());
    let mut c = McpClient::connect(cfg).await.unwrap();
    let pid = std::fs::read_to_string(&file.0).unwrap();
    assert!(std::path::Path::new("/proc").join(&pid).exists());
    tokio::time::timeout(Duration::from_secs(3), c.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(
        !std::path::Path::new("/proc").join(&pid).exists(),
        "fixture must be reaped"
    );
}

/// 写侧背压：一次性并发发起 50 个请求（与既有并发用例一样共享一个连接），
/// 每个请求携带 16 KiB 填充，远大于管道缓冲区，迫使写侧排队。断言全部响应
/// 按唯一 token 对应、无 unmatched 响应、整体在超时保护内完成（不死锁）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_side_backpressure_does_not_lose_frames() {
    let c = Arc::new(connect(Framing::NewlineDelimited).await);
    let mut tasks = Vec::new();
    for i in 0..50 {
        let c = c.clone();
        tasks.push(tokio::spawn(async move {
            let index = i % 10;
            let token = format!("backpressure-{i}");
            let result = c
                .request(
                    "test/echo_index",
                    Some(json!({
                        "index": index,
                        "token": token,
                        "padding": "x".repeat(16 * 1024),
                    })),
                )
                .await
                .unwrap();
            assert_eq!(result["index"], index);
            assert_eq!(result["echo"], token);
        }));
    }
    for task in tasks {
        // 单个请求自身有 10s 超时；这里再包一层整体上限，确保死锁不会挂住测试。
        tokio::time::timeout(Duration::from_secs(60), task)
            .await
            .expect("backpressure writer deadlocked")
            .unwrap();
    }
    let mut c = Arc::try_unwrap(c).ok().unwrap();
    assert_eq!(c.unmatched_responses(), 0, "no frames may be lost");
    c.shutdown().await.unwrap();
}

#[cfg(unix)]
struct TempPidFile(std::path::PathBuf);

#[cfg(unix)]
impl Drop for TempPidFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(unix)]
fn temp_pid_file(tag: &str) -> TempPidFile {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    TempPidFile(std::env::temp_dir().join(format!(
        "mcp-test-child-{tag}-{}-{stamp}.pid",
        std::process::id()
    )))
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    std::path::Path::new("/proc").join(pid.to_string()).exists()
}

/// Drop 兜底回收：不调用 `shutdown`/`disconnect`，直接丢弃 client，子进程必须
/// 在 5s 内被 kill 并 reaped（`/proc/<pid>` 消失，僵尸进程也算未回收）。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_without_shutdown_reaps_child_process() {
    let pid_file = temp_pid_file("drop");
    let mut cfg = config(Framing::NewlineDelimited);
    cfg.command = cfg
        .command
        .with_env("LENGSUO_TEST_PID_FILE", pid_file.0.to_str().unwrap());
    let client = McpClient::connect(cfg).await.unwrap();
    let pid: u32 = std::fs::read_to_string(&pid_file.0)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(process_exists(pid), "fixture {pid} must run before drop");
    drop(client);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while process_exists(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        !process_exists(pid),
        "fixture {pid} must be reaped after Drop"
    );
}
