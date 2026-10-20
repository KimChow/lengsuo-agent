//! 回合取消（h2-session-cancel）：令牌 + 每步检查 + 中断结算 + 不可中断段。
//!
//! 金标：`run-coordinator.ts:147-167`（`interrupt` fire-and-forget）、
//! `step.ts:62`（`TOOLS_INTERRUPTED`）、`step.ts:141`（工具与收口不可中断）。
//! 覆盖卡面五条：取消生效 / 不可中断段 / 标志重置 / 未跑回合时取消 /
//! stop 与 cancel 互不影响，外加立即应答证据。所有等待都有超时。

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
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode, Role, SessionStatus};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

/// 卡面要求「异步测试必须有超时」：所有等待都经本上限。
const WAIT: Duration = Duration::from_secs(10);

// ---------------- 脚本化 provider ----------------

/// 每步从脚本出队并记录调用次数（用于断言「取消后不再发起下一步」）。
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

// ---------------- 执行器（可挂「执行中」钩子） ----------------

/// 「执行中」钩子（`clippy::type_complexity`：收成别名）。
type ExecuteHook = Box<dyn Fn(&str) + Send + Sync>;

/// 记录执行次数；`on_execute` 在工具「正在执行」时调用（调用已进历史、
/// 结果未回填），用来验证取消标志**不会**打断正在执行的工具。
struct GateExecutor {
    calls: Mutex<Vec<String>>,
    on_execute: Mutex<Option<ExecuteHook>>,
}

impl GateExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            on_execute: Mutex::new(None),
        })
    }

    fn with_hook(self: &Arc<Self>, hook: impl Fn(&str) + Send + Sync + 'static) -> Arc<Self> {
        *self.on_execute.lock().expect("锁") = Some(Box::new(hook));
        Arc::clone(self)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("锁").clone()
    }
}

impl ToolExecutor for GateExecutor {
    fn execute(&self, name: &str, _args: &Value) -> Result<Value, String> {
        self.calls.lock().expect("锁").push(name.to_string());
        if let Some(hook) = self.on_execute.lock().expect("锁").as_ref() {
            hook(name);
        }
        Ok(json!({ "ok": true, "tool": name }))
    }
}

// ---------------- harness ----------------

/// 只注册 `fs.read` 的放行门：Craft 白名单 + auto allow ⇒ `Decision::Allow`。
fn allowing_gate() -> PermissionGate {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: "fs.read".to_string(),
        description: "测试工具".to_string(),
        kind: ToolKind::Read,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    PermissionGate::new(registry)
        .with_whitelist(Mode::Craft, BTreeSet::from(["fs.read".to_string()]))
        .with_auto_allowed("fs.read")
}

fn runtime(provider: Arc<dyn Provider>, executor: Option<Arc<GateExecutor>>) -> Runtime {
    let dir = std::env::temp_dir().join(format!("cancel-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let mut builder = Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .gate(allowing_gate())
        .config(RuntimeConfig::default_v0());
    if let Some(executor) = executor {
        builder = builder.tool_executor(executor);
    }
    builder.build().expect("runtime")
}

fn new_session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession {
        title: "取消".into(),
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

fn tool_step(name: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args: json!({ "path": "a.txt" }),
    }]
}

fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

fn done_events(receipt: &TurnReceipt) -> Vec<&Event> {
    receipt.events.iter().filter(|e| e.kind == "done").collect()
}

fn done_status(receipt: &TurnReceipt) -> &Value {
    &done_events(receipt)[0].payload["status"]
}

/// 在后台线程跑 `send_text`，返回 (线程句柄, 完成信号)。
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

// ---------------- 1. 取消生效 ----------------

/// 工具步结算后（步与步之间）置取消标志 ⇒ 不再发起第 2 次 provider 调用，
/// 回合以 `done{status:"cancelled"}` 收口；本步工具**已正常结算**。
#[test]
fn cancel_between_steps_stops_before_next_provider_call() {
    let executor = GateExecutor::new();
    let provider = ScriptProvider::new(vec![tool_step("fs.read"), text_step("不应到达")]);
    let rt = runtime(provider.clone(), Some(executor.clone()));
    let id = new_session(&rt);
    let rt_hook = rt.clone();
    executor.with_hook(move |_name| {
        rt_hook.cancel_session(id).expect("取消");
    });

    let receipt = rt.send_text(id, "读文件").expect("turn");

    assert_eq!(done_events(&receipt).len(), 1, "回合恰一条 done 收口");
    assert_eq!(done_status(&receipt), "cancelled");
    assert_eq!(executor.calls(), vec!["fs.read".to_string()]);
    assert_eq!(provider.calls(), 1, "取消后不得再发起 provider 调用");

    let messages = rt.get_session(id).expect("session").messages;
    assert_eq!(
        messages.iter().filter(|m| m.role == Role::ToolCall).count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .count(),
        1,
        "本步工具必须正常结算（不可中断段）"
    );
    assert!(
        messages
            .iter()
            .filter(|m| m.role == Role::Assistant)
            .all(|m| !m.content.contains("不应到达")),
        "取消后不得再发起下一步"
    );
    // 状态迁移 turn_completed → idle。
    assert_eq!(
        rt.get_session(id).expect("session").status,
        SessionStatus::Idle
    );
}

// ---------------- 2. 不可中断段 ----------------

/// 工具执行**中途**置标志 ⇒ `cancel_session` 立即应答；该工具仍跑完并正常
/// 结算（不是半条），取消只在下一步开头生效。
#[test]
fn tool_in_flight_completes_and_settles_while_cancel_acknowledges_immediately() {
    let executor = GateExecutor::new();
    let rt = runtime(
        ScriptProvider::new(vec![tool_step("fs.read"), text_step("不应到达")]),
        Some(executor.clone()),
    );
    let id = new_session(&rt);

    // 工具执行中阻塞，测试线程在「执行中」置取消标志后放行。
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    executor.with_hook(move |_name| {
        started_tx.send(()).expect("signal");
        release_rx.lock().expect("锁").recv().expect("release");
    });

    let (handle, finished) = spawn_turn(&rt, id, "读文件");
    started_rx.recv_timeout(WAIT).expect("工具已开始执行");
    let t0 = Instant::now();
    rt.cancel_session(id).expect("取消");
    let elapsed = t0.elapsed();
    assert!(elapsed < Duration::from_secs(1), "立即返回：{elapsed:?}");
    assert!(
        !finished.load(Ordering::SeqCst),
        "工具执行中回合不得提前结束"
    );
    release_tx.send(()).expect("放行");

    let receipt = join(handle);
    assert_eq!(
        executor.calls(),
        vec!["fs.read".to_string()],
        "工具跑完一次"
    );
    let messages = rt.get_session(id).expect("session").messages;
    let results: Vec<_> = messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    assert_eq!(results.len(), 1, "工具结果正常结算（非半条）");
    assert!(results[0].content.contains("ok"), "是真实执行结果");
    assert!(
        receipt
            .events
            .iter()
            .all(|e| { !(e.kind == "tool_result" && e.payload["result"]["type"] == "aborted") }),
        "执行中的工具不得被结算成 aborted"
    );
    assert_eq!(done_status(&receipt), "cancelled");
}

// ---------------- 3. 标志重置 ----------------

/// 取消一次后，下一次 `send_text` 必须正常跑完（不被上轮标志吃掉）。
#[test]
fn cancel_flag_is_reset_so_next_turn_runs_to_completion() {
    let executor = GateExecutor::new();
    let rt = runtime(
        ScriptProvider::new(vec![
            tool_step("fs.read"),
            text_step("不应到达"),
            text_step("第二轮正常收敛"),
        ]),
        Some(executor.clone()),
    );
    let id = new_session(&rt);
    let rt_hook = rt.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&cancelled);
    executor.with_hook(move |_name| {
        if !gate.swap(true, Ordering::SeqCst) {
            rt_hook.cancel_session(id).expect("取消");
        }
    });

    let first = rt.send_text(id, "第一轮").expect("turn");
    assert_eq!(done_status(&first), "cancelled");

    // 第二轮：脚本余下的文本步 ⇒ 自然收敛（若标志未复位，会在步首即取消）。
    let second = rt.send_text(id, "第二轮").expect("turn");
    assert_eq!(done_status(&second), "completed");
    assert_eq!(
        done_events(&second)[0].payload["steps"],
        1,
        "第二轮恰好一步"
    );
}

// ---------------- 4. 未跑回合时取消 ----------------

/// 没有回合在跑时设标志：不 panic、立即返回当前快照、不改状态；
/// 回合开始复位标志，随后 `send_text` 正常跑完。
#[test]
fn cancel_idle_session_returns_snapshot_without_panic() {
    let rt = runtime(ScriptProvider::new(vec![text_step("正常收敛")]), None);
    let id = new_session(&rt);
    let before = rt.get_session(id).expect("session");

    let snapshot = rt.cancel_session(id).expect("cancel");
    assert_eq!(snapshot.id, before.id);
    assert_eq!(snapshot.status, before.status, "取消不改 SessionStatus");
    assert_eq!(rt.get_session(id).expect("get").status, SessionStatus::Idle);

    // 回合开始复位：空闲期设下的标志不吞掉下一回合。
    let receipt = rt.send_text(id, "正常回合").expect("turn");
    assert_eq!(done_status(&receipt), "completed");
}

// ---------------- 5. stop 与 cancel 互不影响 ----------------

/// `stop_session` 只改状态、不设取消标志；`cancel_session` 只设标志、不改状态。
#[test]
fn stop_and_cancel_are_independent_actions() {
    let rt = runtime(
        ScriptProvider::new(vec![text_step("正常收敛"), text_step("正常收敛")]),
        None,
    );
    let id = new_session(&rt);

    // stop：状态回 idle；随后回合仍能正常跑完（stop 没设取消标志）。
    let stopped = rt.stop_session(id).expect("stop");
    assert_eq!(stopped.status, SessionStatus::Idle);
    let receipt = rt.send_text(id, "stop 之后").expect("turn");
    assert_eq!(done_status(&receipt), "completed");

    // cancel：只置标志、不改状态；应答立即返回（不等回合，也不触发回合）。
    let cancelled = rt.cancel_session(id).expect("cancel");
    assert_eq!(cancelled.status, SessionStatus::Idle, "cancel 不改状态");
    // 空闲期设下的标志在下一回合开始时复位，因此回合仍正常跑完。
    let receipt = rt.send_text(id, "cancel 之后").expect("turn");
    assert_eq!(done_status(&receipt), "completed");
}
