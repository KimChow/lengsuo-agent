//! 取消可打断在途工具（x-cancel-intra）：可取消执行器槽 + 取消信号 + 打断结算 +
//! 会话重建后的陈旧扫描。
//!
//! 卡面八例：立即应答 / 信号可达执行器 / 恰好一条 `aborted` 结果且与调用成对 /
//! 回合以 `done{status:"cancelled"}` 收口 / 结算段在标志已置位时仍然完成 /
//! 重建后没有悬空的 running 工具 / 普通执行器零回归 / 中断标记在事件流可观测。
//!
//! 口径要点：**工具执行区间可打断**（取消到达后由执行器自行收尾），
//! **返回后的结算段不可打断**（未结算的调用由回合收口的 `settle_cancelled`
//! 按 `TOOLS_INTERRUPTED` 结算，它不查取消标志）。所有阻塞等待都有超时。

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::r#loop::{CancelSignal, CancellableToolExecutor};
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Isolation, Mode, PermissionMode, Role, SessionStatus};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

/// 卡面要求「异步测试必须有超时」：所有等待都经本上限。
const WAIT: Duration = Duration::from_secs(10);
/// 白名单/门检放行的唯一工具名。
const TOOL: &str = "fs.read";

// ---------------- 脚本化 provider ----------------

struct ScriptProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    calls: Mutex<usize>,
}

impl ScriptProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            calls: Mutex::new(0),
        })
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("锁")
    }
}

impl Provider for ScriptProvider {
    fn next_steps(&self, _id: Uuid, _text: &str) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本测试只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        _context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        *self.calls.lock().expect("锁") += 1;
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported("脚本已耗尽：出现计划外的调用"))
    }

    fn next_steps_with_options(
        &self,
        _context: &TurnContext,
        _options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本测试不触发缴械步"))
    }
}

fn tool_step(name: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args: json!({ "path": "a.txt" }),
    }]
}

fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

// ---------------- 可取消执行器 ----------------

/// 可取消执行器的两种收尾方式：
/// - [`Behaviour::BlockThenCheck`]：执行中阻塞等放行，放行后按取消标志收尾；
/// - [`Behaviour::WakeOnSignal`]：直接用 [`CancelSignal::wait_cancelled`] 等信号。
enum Behaviour {
    BlockThenCheck {
        entered: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    },
    WakeOnSignal,
}

struct CancellableExec {
    calls: Mutex<Vec<String>>,
    /// 执行器在**执行区间内**是否观察到取消（信号可达的证据面）。
    observed: Arc<AtomicBool>,
    behaviour: Behaviour,
}

impl CancellableExec {
    fn blocking() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let exec = Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            observed: Arc::new(AtomicBool::new(false)),
            behaviour: Behaviour::BlockThenCheck {
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(release_rx),
            },
        });
        (exec, entered_rx, release_tx)
    }

    fn waiting() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            observed: Arc::new(AtomicBool::new(false)),
            behaviour: Behaviour::WakeOnSignal,
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("锁").clone()
    }

    fn observed_cancel(&self) -> bool {
        self.observed.load(Ordering::SeqCst)
    }
}

impl CancellableToolExecutor for CancellableExec {
    fn execute_cancellable(
        &self,
        name: &str,
        _args: &Value,
        cancel: &CancelSignal,
    ) -> Result<Value, String> {
        self.calls.lock().expect("锁").push(name.to_string());
        let interrupted = match &self.behaviour {
            Behaviour::BlockThenCheck { entered, release } => {
                if let Some(tx) = entered.lock().expect("锁").take() {
                    let _ = tx.send(());
                }
                release
                    .lock()
                    .expect("锁")
                    .recv_timeout(WAIT)
                    .map_err(|_| "放行超时".to_string())?;
                cancel.is_cancelled()
            }
            Behaviour::WakeOnSignal => cancel.wait_cancelled(WAIT),
        };
        if interrupted {
            self.observed.store(true, Ordering::SeqCst);
            Err("工具执行被取消打断".to_string())
        } else {
            Ok(json!({ "ok": true, "tool": name }))
        }
    }
}

// ---------------- 普通（不可取消）执行器：零回归对照 ----------------

struct PlainExec {
    calls: Mutex<Vec<String>>,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl PlainExec {
    fn blocking() -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let exec = Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(release_rx),
        });
        (exec, entered_rx, release_tx)
    }
}

impl ToolExecutor for PlainExec {
    fn execute(&self, name: &str, _args: &Value) -> Result<Value, String> {
        self.calls.lock().expect("锁").push(name.to_string());
        if let Some(tx) = self.entered.lock().expect("锁").take() {
            let _ = tx.send(());
        }
        self.release
            .lock()
            .expect("锁")
            .recv_timeout(WAIT)
            .map_err(|_| "放行超时".to_string())?;
        Ok(json!({ "ok": true, "tool": name }))
    }
}

// ---------------- harness ----------------

fn allowing_gate() -> PermissionGate {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: TOOL.to_string(),
        description: "测试工具".to_string(),
        kind: ToolKind::Read,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    PermissionGate::new(registry)
        .with_whitelist(Mode::Craft, BTreeSet::from([TOOL.to_string()]))
        .with_auto_allowed(TOOL)
}

fn build(dir: &std::path::Path, provider: Arc<dyn Provider>) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(provider)
        .gate(allowing_gate())
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("runtime")
}

fn tempdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cancel-intra-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

fn new_session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession {
        title: "取消可打断".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Craft,
        permission_mode: PermissionMode::Full,
        env_binding: None,
        isolation: Isolation {
            user_id: "u".into(),
            tenant_id: "t".into(),
            env_id: None,
        },
    })
    .expect("create session")
    .id
}

fn spawn_turn(
    rt: &Runtime,
    id: Uuid,
    text: &'static str,
) -> (
    std::thread::JoinHandle<Result<TurnReceipt, CoreError>>,
    Arc<AtomicBool>,
) {
    let rt = rt.clone();
    let finished = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&finished);
    let handle = std::thread::spawn(move || {
        let out = rt.send_text(id, text);
        flag.store(true, Ordering::SeqCst);
        out
    });
    (handle, finished)
}

fn join(handle: std::thread::JoinHandle<Result<TurnReceipt, CoreError>>) -> TurnReceipt {
    handle
        .join()
        .expect("回合线程不得 panic")
        .expect("回合不得失败")
}

fn done_events(receipt: &TurnReceipt) -> Vec<&agent_core::schema::Event> {
    receipt.events.iter().filter(|e| e.kind == "done").collect()
}

fn done_status(receipt: &TurnReceipt) -> Value {
    done_events(receipt)[0].payload["status"].clone()
}

fn aborted_results(receipt: &TurnReceipt) -> Vec<&Value> {
    receipt
        .events
        .iter()
        .filter(|e| e.kind == "tool_result" && e.payload["result"]["type"] == "aborted")
        .map(|e| &e.payload)
        .collect()
}

fn role_count(rt: &Runtime, id: Uuid, role: Role) -> usize {
    rt.get_session(id)
        .expect("session")
        .messages
        .iter()
        .filter(|m| m.role == role)
        .count()
}

// ---------------- 1. 立即应答 ----------------

/// 工具正在执行（尚未返回）时 `cancel_session` 必须立即应答，不等回合结束。
#[test]
fn cancel_acknowledges_immediately_while_cancellable_tool_is_in_flight() {
    let dir = tempdir("ack");
    let provider = ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]);
    let rt = build(&dir, provider.clone());
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = CancellableExec::blocking();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");

    let t0 = Instant::now();
    rt.cancel_session(id).expect("取消");
    let elapsed = t0.elapsed();
    assert!(elapsed < Duration::from_secs(1), "立即返回：{elapsed:?}");
    assert!(
        !finished.load(Ordering::SeqCst),
        "工具仍在执行，回合不得提前结束"
    );

    release_tx.send(()).expect("放行");
    let receipt = join(handle);
    assert_eq!(done_status(&receipt), "cancelled");
    assert_eq!(provider.calls(), 1, "打断后不得再发起 provider 调用");
}

// ---------------- 2. 信号可达执行器 ----------------

/// 取消信号必须真的到达在途执行器：执行器经 `wait_cancelled`（Condvar 唤醒，
/// 不轮询）在取消后立即收尾，无需测试线程再放行任何东西。
#[test]
fn cancel_signal_reaches_the_in_flight_executor() {
    let dir = tempdir("signal");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let exec = CancellableExec::waiting();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    // 等执行器进入 `wait_cancelled`（轮询调用记录即可，最多 WAIT）。
    let deadline = Instant::now() + WAIT;
    while exec.calls().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(exec.calls(), vec![TOOL.to_string()], "执行器已进入在途区间");

    rt.cancel_session(id).expect("取消");
    let receipt = join(handle);

    assert!(exec.observed_cancel(), "执行器必须在执行区间内观察到取消");
    assert_eq!(done_status(&receipt), "cancelled");
}

// ---------------- 3. 恰好一条 aborted 结果且与调用成对 ----------------

/// 被打断的工具只结算一次：`tool_result{isError:true, type:"aborted"}`，
/// 且历史上 ToolCall / ToolResult 数量相等（没有悬空、也没有半条）。
#[test]
fn interrupted_tool_settles_once_as_aborted_and_pairs_with_call() {
    let dir = tempdir("aborted");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = CancellableExec::blocking();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");
    rt.cancel_session(id).expect("取消");
    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    let aborted = aborted_results(&receipt);
    assert_eq!(aborted.len(), 1, "打断结算恰好一条 aborted");
    assert_eq!(aborted[0]["isError"], true);
    assert_eq!(
        aborted[0]["result"]["message"], "Tool execution interrupted",
        "结算口径逐字复用 TOOLS_INTERRUPTED"
    );
    assert_eq!(role_count(&rt, id, Role::ToolCall), 1);
    assert_eq!(
        role_count(&rt, id, Role::ToolResult),
        1,
        "调用与结果一一配对"
    );
    let messages = rt.get_session(id).expect("session").messages;
    let result_at = messages
        .iter()
        .position(|m| m.role == Role::ToolResult)
        .expect("打断已结算");
    assert!(
        messages[result_at].content.contains("\"type\":\"aborted\""),
        "结果消息内容即 TOOLS_INTERRUPTED：{}",
        messages[result_at].content
    );
}

// ---------------- 4. done{cancelled} 收口 ----------------

/// 打断不是失败、也不是拒绝：回合以唯一一条 `done{status:"cancelled"}` 收口。
#[test]
fn interrupted_turn_ends_with_done_cancelled_status() {
    let dir = tempdir("done");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = CancellableExec::blocking();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");
    rt.cancel_session(id).expect("取消");
    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    let done = done_events(&receipt);
    assert_eq!(done.len(), 1, "回合恰一条 done 收口");
    assert_eq!(done[0].payload["status"], "cancelled");
    assert_ne!(done[0].payload["status"], "failed");
    assert_ne!(done[0].payload["status"], "denied");
    assert_eq!(
        rt.get_session(id).expect("session").status,
        SessionStatus::Idle
    );
}

// ---------------- 5. 结算段不可打断 ----------------

/// 执行器返回时取消标志**已置位**；结算段仍必须把它结算掉（不可中断段）：
/// 证据是「执行器观察到取消」与「该调用仍被结算成 aborted」同时成立。
#[test]
fn settlement_completes_even_though_cancel_flag_is_still_set() {
    let dir = tempdir("settle");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = CancellableExec::blocking();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");
    rt.cancel_session(id).expect("取消");
    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    assert!(exec.observed_cancel(), "执行器在返回时已看到取消标志置位");
    assert_eq!(
        aborted_results(&receipt).len(),
        1,
        "标志已置位，结算段仍要结算未配对调用"
    );
    assert_eq!(role_count(&rt, id, Role::ToolCall), 1);
    assert_eq!(role_count(&rt, id, Role::ToolResult), 1);
}

// ---------------- 6. 重建后的陈旧扫描 ----------------

fn write_log(dir: &std::path::Path, sid: Uuid, lines: &[String]) {
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write log");
}

/// 会话重建后不得留下悬空的 `running` 工具：上一进程死在工具执行中时，日志里
/// 只有 `tool_use` 没有 `tool_result`；下次回合开始前必须补齐中断结果。
#[test]
fn stale_scan_after_rebuild_leaves_no_dangling_running_tool() {
    let dir = tempdir("stale");
    let sid = Uuid::new_v4();
    let lines = [
        format!(r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"session_update","payload":{{"status":"idle","title":"崩溃会话","mode":"craft"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"user","payload":{{"text":"读文件"}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"tool_use","payload":{{"name":"{TOOL}","args":{{"path":"a.txt"}}}},"tsMs":30}}"#
        ),
    ];
    write_log(&dir, sid, &lines);

    let rt = build(&dir, ScriptProvider::new(vec![text_step("继续")]));
    let restored = rt.get_session(sid).expect("回放认出会话");
    assert_eq!(
        restored
            .messages
            .iter()
            .filter(|m| m.role == Role::ToolCall)
            .count(),
        1,
        "回放留下了未配对的 ToolCall（崩溃残留）"
    );
    assert_eq!(
        restored
            .messages
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .count(),
        0
    );

    // 下一回合开始前的陈旧扫描补齐中断结果。
    let receipt = rt.send_text(sid, "继续").expect("turn");
    assert_eq!(done_status(&receipt), "completed", "陈旧扫描不改回合结局");

    let messages = rt.get_session(sid).expect("session").messages;
    let calls = messages.iter().filter(|m| m.role == Role::ToolCall).count();
    let results = messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .count();
    assert_eq!(calls, 1);
    assert_eq!(results, 1, "每个 ToolCall 都配对到结果，无悬空");
    // 回放的历史以崩溃前那条 user 消息打头，随后是未配对的 ToolCall；扫描在同一
    // 回合起点补上 ToolResult（追加在本轮 user 消息之后，栈配对仍认到该调用）。
    let call_at = messages
        .iter()
        .position(|m| m.role == Role::ToolCall)
        .expect("陈旧调用仍在历史里");
    let result_at = messages
        .iter()
        .position(|m| m.role == Role::ToolResult)
        .expect("陈旧调用已配对");
    assert!(result_at > call_at, "结果不得早于调用");
    assert!(
        messages[result_at].content.contains("\"type\":\"aborted\""),
        "陈旧调用按 TOOLS_INTERRUPTED 结算：{}",
        messages[result_at].content
    );
    assert_eq!(
        rt.settle_stale_tool_calls(sid).expect("公开扫描"),
        0,
        "自动扫描后公开扫描幂等：已配对的调用不动"
    );
}

// ---------------- 7. 普通执行器零回归 ----------------

/// 只注入普通 `ToolExecutor` 的会话保持既有语义：在途工具跑完并**正常**结算，
/// 取消只在步边界生效，不会被结算成 aborted。
#[test]
fn plain_executor_keeps_legacy_uninterruptible_settlement() {
    let dir = tempdir("plain");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = PlainExec::blocking();
    rt.set_tool_executor(exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");
    rt.cancel_session(id).expect("取消");
    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    let messages = rt.get_session(id).expect("session").messages;
    let results: Vec<_> = messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    assert_eq!(results.len(), 1, "普通执行器照常结算一次");
    assert!(results[0].content.contains("ok"), "是真实执行结果");
    assert!(
        aborted_results(&receipt).is_empty(),
        "普通执行器不得被结算成 aborted"
    );
    assert_eq!(done_status(&receipt), "cancelled", "取消仍在步边界收口");
}

// ---------------- 8. 中断标记可观测 ----------------

/// 中断标记在事件流里可观测：`tool_result` 事件带 `isError:true` 与
/// `result.type=="aborted"`，且发生在 `done` 之前（结算先于收口）。
#[test]
fn interrupted_tool_marker_is_observable_in_event_stream() {
    let dir = tempdir("marker");
    let rt = build(
        &dir,
        ScriptProvider::new(vec![tool_step(TOOL), text_step("不应到达")]),
    );
    let id = new_session(&rt);
    let (exec, entered_rx, release_tx) = CancellableExec::blocking();
    rt.set_cancellable_executor(id, exec.clone());

    let (handle, _finished) = spawn_turn(&rt, id, "读文件");
    entered_rx.recv_timeout(WAIT).expect("工具已开始执行");
    rt.cancel_session(id).expect("取消");
    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    let kinds: Vec<&str> = receipt.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds.first().copied(), Some("user"));
    assert_eq!(kinds.last().copied(), Some("done"));

    let marker = receipt
        .events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("有 tool_result 事件");
    assert_eq!(marker.payload["isError"], true);
    assert_eq!(marker.payload["result"]["type"], "aborted");
    assert_eq!(
        marker.payload["result"]["message"],
        "Tool execution interrupted"
    );
    assert_eq!(marker.payload["name"], TOOL);
}
