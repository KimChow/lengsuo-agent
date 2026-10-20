//! 网关 `session:cancel`（h2-session-cancel）：立即应答 + 归属隔离。
//!
//! 金标 `packages/core/src/session/run-coordinator.ts:163-166`：
//! 「callers like the interrupt endpoint must acknowledge immediately even when
//! finalizers are slow」——设 flag 后立即返回 `{code:0}`，不等回合跑完。
//!
//! 仅在 Unix 运行：网关起服需写私有握手文件（ACL 保护），Windows 上该 ACL 未实现。
#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::error::CoreError;
use agent_core::provider::{EchoProvider, Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::Isolation;
use agent_gateway::{GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 卡面要求「异步测试必须有超时」。
const WAIT: Duration = Duration::from_secs(10);

async fn request(port: u16, token: &str, action: &str, payload: Value) -> (u16, Value) {
    tokio::time::timeout(WAIT, async {
        let body = json!({"action": action, "payload": payload}).to_string();
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let wire = format!("POST /rpc HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nX-Gateway-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        s.write_all(wire.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    })
    .await
    .expect("bounded RPC")
}

/// 首次调用阻塞（模拟回合并未结束），放行后返回工具调用，让回合有下一步可走；
/// 下一步在步首观察到取消标志后收口。
struct BlockingProvider {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
    calls: AtomicUsize,
}

impl BlockingProvider {
    fn new() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let provider = Arc::new(Self {
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
            calls: AtomicUsize::new(0),
        });
        (provider, entered_rx, release_tx)
    }
}

impl Provider for BlockingProvider {
    fn next_steps(&self, _id: uuid::Uuid, _text: &str) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本测试只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        _context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            if let Some(tx) = self.entered.lock().expect("锁").take() {
                tx.send(()).expect("signal entered");
            }
            self.release
                .lock()
                .expect("锁")
                .recv_timeout(WAIT)
                .map_err(|_| CoreError::Unsupported("provider 未放行"))?;
            return Ok(vec![ProviderStep::ToolCall {
                name: "fs.read".into(),
                args: json!({ "path": "a.txt" }),
            }]);
        }
        Ok(vec![
            ProviderStep::Text("不应到达".into()),
            ProviderStep::Finished,
        ])
    }
}

fn runtime(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

async fn start(dir: &std::path::Path, rt: Runtime) -> GatewayHandle {
    GatewayHandle::start(
        rt,
        GatewayConfig::new(dir.join("gw.json"), Isolation::local_default()),
    )
    .await
    .unwrap()
}

async fn create_session(g: &GatewayHandle, title: &str) -> Value {
    let (_, v) = request(
        g.port(),
        g.token(),
        "session:create",
        json!({ "title": title }),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    v["data"]["id"].clone()
}

#[tokio::test]
async fn cancel_acknowledges_immediately_while_turn_is_still_running() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, entered_rx, release_tx) = BlockingProvider::new();
    let rt = Runtime::builder()
        .workdir(dir.path())
        .provider(provider)
        .build()
        .unwrap();
    let g = start(dir.path(), rt).await;
    let id = create_session(&g, "cancel").await;

    let port = g.port();
    let token = g.token().to_string();
    let id_for_send = id.clone();
    let send_task = tokio::spawn(async move {
        request(
            port,
            &token,
            "session:send",
            json!({ "id": id_for_send, "text": "读文件" }),
        )
        .await
    });

    // 回合已进入 provider（仍在跑）。
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(WAIT))
        .await
        .unwrap()
        .expect("回合已开始");

    let t0 = Instant::now();
    let (status, cancelled) =
        request(g.port(), g.token(), "session:cancel", json!({ "id": id })).await;
    let elapsed = t0.elapsed();

    assert_eq!(status, 200);
    assert_eq!(cancelled["code"], 0, "{cancelled}");
    assert_eq!(cancelled["data"]["id"], id, "立即返回当前会话快照");
    assert!(
        elapsed < Duration::from_secs(1),
        "必须立即应答（不等 finalizer）：{elapsed:?}"
    );
    assert!(!send_task.is_finished(), "回合仍在跑时 cancel 已返回");

    // 放行回合：下一步开头观察到标志 ⇒ done{cancelled}。
    release_tx.send(()).expect("release");
    let (_, sent) = send_task.await.expect("send task");
    assert_eq!(sent["code"], 0, "{sent}");
    let events = sent["data"]["events"].as_array().expect("events");
    let done = events
        .iter()
        .find(|e| e["kind"] == "done")
        .expect("回合有 done");
    assert_eq!(done["payload"]["status"], "cancelled");

    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_isolated_by_ownership_and_rejects_unknown_session() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(dir.path());
    // 另一个 app（不同隔离键）的会话：网关的 `own` 必须拒绝。
    let foreign = rt
        .create_session(&CreateSession {
            title: "foreign".into(),
            workspace_root: None,
            mode: agent_core::schema::Mode::Ask,
            permission_mode: agent_core::schema::PermissionMode::ReadOnly,
            env_binding: None,
            isolation: Isolation {
                user_id: "other-user".into(),
                tenant_id: "other-tenant".into(),
                env_id: None,
            },
        })
        .unwrap();
    let g = start(dir.path(), rt).await;

    let (_, v) = request(
        g.port(),
        g.token(),
        "session:cancel",
        json!({ "id": foreign.id.to_string() }),
    )
    .await;
    assert_eq!(v["code"], 403, "非本 app 会话必须拒绝：{v}");
    assert_eq!(v["msg"], "forbidden");

    let (_, v) = request(
        g.port(),
        g.token(),
        "session:cancel",
        json!({ "id": uuid::Uuid::new_v4().to_string() }),
    )
    .await;
    assert_eq!(v["code"], 404, "未知会话：{v}");

    // 本 app 会话：无回合时取消也立即成功，且不改状态。
    let id = create_session(&g, "own").await;
    let (_, v) = request(g.port(), g.token(), "session:cancel", json!({ "id": id })).await;
    assert_eq!(v["code"], 0, "{v}");
    let (_, got) = request(g.port(), g.token(), "session:get", json!({ "id": id })).await;
    assert_eq!(got["data"]["status"], "idle", "cancel 不改 SessionStatus");

    g.shutdown().await.unwrap();
}
