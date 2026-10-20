//! 审批链闭环（h3-approval-chain）：tool_call_id 关联、取消传播、回传闭环。
//!
//! 规格：`harness-permission-sandbox.md` §9（审批链）+ §17 的 P3 行。
//! 对照要点：审批前须先用 `tool_call_id` 把决策与工具调用关联，批准后才执行。
//!
//! 覆盖：批准真执行且回传 toolCallId；拒绝回「用户拒绝」isError；会话取消后
//! 批准被丢弃；等待中取消提前醒来；超时明确失败；审批人区分 user/自动策略。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{
    ApprovalActor, ApprovalRequest, ApprovalResolution, Event, Isolation, Mode, PermissionMode,
    Role,
};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

const WAIT: Duration = Duration::from_secs(5);

// ---------------- 脚本化 provider ----------------

struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
        })
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本测试只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        _context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
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

fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

/// 一次回合：先请求工具（步内落 Ask 挂起），随后自然收敛。
fn one_ask_turn(tool: &str) -> Vec<Vec<ProviderStep>> {
    vec![
        tool_step(tool, json!({ "path": "a.txt" })),
        text_step("等待审批"),
    ]
}

// ---------------- 计数执行器 ----------------

struct CountingExecutor {
    calls: Mutex<Vec<(String, Value)>>,
}

impl CountingExecutor {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.lock().expect("锁").len()
    }

    fn as_dyn(self: Arc<Self>) -> Arc<dyn ToolExecutor + Send + Sync> {
        self
    }
}

impl ToolExecutor for CountingExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.calls
            .lock()
            .expect("锁")
            .push((name.to_string(), args.clone()));
        Ok(json!({ "ok": true, "tool": name }))
    }
}

// ---------------- harness ----------------

/// 注册 + 白名单但未 auto 放行 ⇒ `Decision::Ask`（审批链入口）。
fn asking_gate(approval_timeout: Duration) -> PermissionGate {
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
        .with_approval_timeout(approval_timeout)
}

fn runtime(timeout: Duration, executor: Arc<CountingExecutor>) -> Runtime {
    let dir = std::env::temp_dir().join(format!("approval-chain-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(ScriptedProvider::new(one_ask_turn("fs.read")))
        .gate(asking_gate(timeout))
        .config(RuntimeConfig::default_v0())
        .tool_executor(executor.as_dyn())
        .build()
        .expect("runtime")
}

fn craft_session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession {
        title: "审批链".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Craft,
        permission_mode: PermissionMode::Full,
        env_binding: None,
        isolation: Isolation::local_default(),
    })
    .expect("create session")
    .id
}

fn only_pending(rt: &Runtime, session: Uuid) -> ApprovalRequest {
    let mut pending = rt.pending_approval_requests(session);
    assert_eq!(pending.len(), 1, "一次 Ask 恰一个挂起审批: {pending:?}");
    pending.remove(0)
}

fn last_tool_result(rt: &Runtime, session: Uuid) -> String {
    rt.get_session(session)
        .expect("session")
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::ToolResult)
        .map(|m| m.content.clone())
        .expect("回合历史里必须有 tool_result")
}

fn error_message(event: &Event) -> String {
    event.payload["result"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn approved_call_executes_and_result_returns_with_tool_call_id() {
    let executor = CountingExecutor::new();
    let rt = runtime(Duration::from_secs(120), Arc::clone(&executor));
    let session = craft_session(&rt);

    rt.send_text(session, "读取 a.txt").expect("turn");
    assert_eq!(executor.calls(), 0, "Ask 未批准前不得执行");

    let request = only_pending(&rt, session);
    let tool_call_id = request.tool_call_id.expect("审批请求必须携带 tool_call_id");
    assert_eq!(request.tool_name, "fs.read");
    assert!(!request.summary.is_empty(), "审批请求要带参数摘要");

    let outcome = rt
        .respond_approval_as(request.id, true, ApprovalActor::User)
        .expect("respond");
    assert!(outcome.approved);
    assert!(!outcome.cancelled);
    assert_eq!(outcome.actor, Some(ApprovalActor::User), "审批人字段落库");

    let event = rt
        .resume_approved_tool_call(
            session,
            tool_call_id,
            ApprovalResolution::Approved {
                actor: ApprovalActor::User,
            },
        )
        .expect("resume");

    assert_eq!(executor.calls(), 1, "批准后必须真正执行一次");
    assert_eq!(event.kind, "tool_result");
    assert_eq!(
        event.payload["toolCallId"],
        json!(tool_call_id.to_string()),
        "回传事件必须关联同一个 tool_call_id"
    );
    assert_eq!(event.payload["isError"], false, "成功不是错误结果");
    assert!(
        last_tool_result(&rt, session).contains("\"ok\":true"),
        "真实执行结果必须回到回合历史"
    );
    assert!(
        rt.pending_approval_requests(session).is_empty(),
        "应答后离开待审批集"
    );
}

#[test]
fn denied_call_returns_user_refusal_not_execution_failure() {
    let executor = CountingExecutor::new();
    let rt = runtime(Duration::from_secs(120), Arc::clone(&executor));
    let session = craft_session(&rt);

    rt.send_text(session, "读取 a.txt").expect("turn");
    let request = only_pending(&rt, session);
    let tool_call_id = request.tool_call_id.expect("tool_call_id");

    let outcome = rt
        .respond_approval_as(request.id, false, ApprovalActor::AutoPolicy)
        .expect("respond");
    assert!(!outcome.approved);
    assert!(!outcome.cancelled);
    assert_eq!(outcome.actor, Some(ApprovalActor::AutoPolicy));

    let event = rt
        .resume_approved_tool_call(
            session,
            tool_call_id,
            ApprovalResolution::Denied {
                actor: ApprovalActor::AutoPolicy,
            },
        )
        .expect("resume");

    assert_eq!(executor.calls(), 0, "拒绝后不得执行工具");
    assert_eq!(event.payload["isError"], true, "拒绝必须是 isError 结果");
    assert_eq!(event.payload["toolCallId"], json!(tool_call_id.to_string()));
    let text = error_message(&event);
    assert!(text.contains("用户拒绝"), "文案必须是用户拒绝语义: {text}");
    assert!(!text.contains("执行失败"), "不得误报成执行失败: {text}");
    assert!(last_tool_result(&rt, session).contains("用户拒绝"));
}

#[test]
fn approval_after_session_cancel_is_discarded() {
    let executor = CountingExecutor::new();
    let rt = runtime(Duration::from_secs(120), Arc::clone(&executor));
    let session = craft_session(&rt);

    rt.send_text(session, "读取 a.txt").expect("turn");
    let request = only_pending(&rt, session);
    let tool_call_id = request.tool_call_id.expect("tool_call_id");

    rt.cancel_session(session).expect("cancel");

    let outcome = rt
        .respond_approval_as(request.id, true, ApprovalActor::User)
        .expect("cancel 后仍要有明确结论");
    assert!(!outcome.approved, "取消后的批准必须被丢弃");
    assert!(outcome.cancelled, "结论必须标记为取消");

    let event = rt
        .resume_approved_tool_call(
            session,
            tool_call_id,
            ApprovalResolution::Approved {
                actor: ApprovalActor::User,
            },
        )
        .expect("resume");

    assert_eq!(executor.calls(), 0, "取消后即使批准也不得执行");
    assert_eq!(event.payload["isError"], true);
    assert!(
        error_message(&event).contains("取消"),
        "回传必须说明会话已取消: {}",
        error_message(&event)
    );
}

#[test]
fn cancel_wakes_pending_wait_and_returns_failure_to_turn() {
    let executor = CountingExecutor::new();
    let rt = runtime(Duration::from_secs(120), Arc::clone(&executor));
    let session = craft_session(&rt);

    rt.send_text(session, "读取 a.txt").expect("turn");
    let request = only_pending(&rt, session);
    let tool_call_id = request.tool_call_id.expect("tool_call_id");

    let waiter = rt.clone();
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let started = Instant::now();
        let outcome = waiter.wait_approval(request.id, Duration::from_secs(30));
        tx.send((outcome, started.elapsed())).expect("send");
    });

    std::thread::sleep(Duration::from_millis(50));
    rt.cancel_session(session).expect("cancel");

    let (outcome, elapsed) = rx
        .recv_timeout(WAIT)
        .expect("取消必须唤醒审批等待，不得无限挂起");
    let outcome = outcome.expect("wait 必须返回明确结果");
    handle.join().expect("waiter 不得 panic");
    assert!(outcome.cancelled, "等待中的审批必须随会话取消作废");
    assert!(!outcome.approved);
    assert!(elapsed < WAIT, "必须提前醒来，实际等待 {elapsed:?}");

    let event = rt
        .resume_approved_tool_call(session, tool_call_id, ApprovalResolution::Cancelled)
        .expect("resume");
    assert_eq!(executor.calls(), 0);
    assert_eq!(event.payload["isError"], true);
    assert!(
        error_message(&event).contains("取消"),
        "回合必须收到明确的取消失败结果: {}",
        error_message(&event)
    );
}

#[test]
fn approval_timeout_returns_failure_instead_of_hanging() {
    let executor = CountingExecutor::new();
    // 创建即过期的审批：wait 必须立刻返回 timed_out，而不是挂到调用方超时。
    let rt = runtime(Duration::ZERO, Arc::clone(&executor));
    let session = craft_session(&rt);

    rt.send_text(session, "读取 a.txt").expect("turn");
    let request = only_pending(&rt, session);
    let tool_call_id = request.tool_call_id.expect("tool_call_id");

    let started = Instant::now();
    let outcome = rt
        .wait_approval(request.id, Duration::from_secs(600))
        .expect("wait");
    assert!(outcome.timed_out, "必须标记等待超时");
    assert!(!outcome.approved);
    assert!(started.elapsed() < WAIT, "过期审批不得按调用方 600s 阻塞");

    let event = rt
        .resume_approved_tool_call(session, tool_call_id, ApprovalResolution::TimedOut)
        .expect("resume");
    assert_eq!(executor.calls(), 0, "超时不得执行");
    assert_eq!(event.payload["isError"], true);
    assert!(
        error_message(&event).contains("超时"),
        "回合必须收到明确的超时失败结果: {}",
        error_message(&event)
    );
}
