//! Runtime 工具执行接线测试（W1-core）：注入执行器后工具步返回真实结果。
//!
//! 语义出处：金标 opencode `step.ts:91-134`（先 toolChoice 门检再 executeTool，
//! 失败走 failTool 结算成结果而非中断回合）、`llm.ts:191-198`（步循环：一步一次
//! 调用，工具步结算后重读历史开新步）。
//!
//! 覆盖矩阵：
//! - Allow + 执行成功 ⇒ 真实结果进历史与事件，下一步 provider 重读到；
//! - Deny ⇒ 执行器零调用，历史是 deny 结果；同一工具二次被拒 ⇒ 回合终止；
//! - 执行 Err ⇒ `isError=true` 结算，回合继续（failTool 语义）；
//! - 大输出 ⇒ 消息只留 preview，`result_ref` 进 blob；
//! - 未注入执行器 / Ask ⇒ 保持 pending 占位，不发 `tool_result` 事件。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt, CONTENT_PREVIEW_CHARS};
use agent_core::schema::{Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

// ---------------- 脚本化 provider（步语义契约测试同款观测面） ----------------

/// 按队列出步并记录每次收到的 `TurnContext`（含 `next_steps_with_options`
/// 经默认实现转发的调用），供「相邻两次调用之间历史已重读」断言。
struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<TurnContext>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// provider 被调用次数 = 实际 LLM 调用数。
    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }

    /// 第 n 次调用（0 起）收到的上下文副本。
    fn ctx_at(&self, n: usize) -> TurnContext {
        self.seen.lock().expect("锁")[n].clone()
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "工具执行测试只走带上下文接口（provider-context 卡约束）",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("锁").push(context.clone());
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported(
                "脚本已耗尽：出现计划外的第 n+1 次调用",
            ))
    }
}

/// 无工具、单步收敛的脚本（text + done）。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

/// 一步以工具调用终结。
fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

// ---------------- 计数执行器（Mutex 版，满足 Send + Sync） ----------------

/// 记录每次调用的执行器：全部调用返回同一脚本化结果。
struct ScriptedExecutor {
    calls: Mutex<Vec<(String, Value)>>,
    outcome: Result<Value, String>,
}

impl ScriptedExecutor {
    fn ok(value: Value) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            outcome: Ok(value),
        })
    }

    fn err(message: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            outcome: Err(message.to_string()),
        })
    }

    /// 被执行的次数（deny 断言 0 次的计数面）。
    fn calls(&self) -> usize {
        self.calls.lock().expect("锁").len()
    }

    /// 转成可注入的 trait 对象（调用计数仍经原句柄观测）。
    fn as_dyn(self: Arc<Self>) -> Arc<dyn ToolExecutor + Send + Sync> {
        self
    }
}

impl ToolExecutor for ScriptedExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.calls
            .lock()
            .expect("锁")
            .push((name.to_string(), args.clone()));
        self.outcome.clone()
    }
}

// ---------------- 权限门三种姿态 ----------------

fn registry_with(name: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: name.to_string(),
        description: "测试工具".to_string(),
        kind: ToolKind::Read,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    registry
}

/// 注册 + 白名单 + auto 放行 ⇒ `Decision::Allow`。
fn allowing_gate(name: &str) -> PermissionGate {
    PermissionGate::new(registry_with(name))
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
        .with_auto_allowed(name)
}

/// 空注册表 ⇒ `Decision::Deny`（工具未注册）。
fn denying_gate() -> PermissionGate {
    PermissionGate::new(ToolRegistry::new())
}

/// 注册 + 白名单但未 auto 放行 ⇒ `Decision::Ask`。
fn asking_gate(name: &str) -> PermissionGate {
    PermissionGate::new(registry_with(name))
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
}

// ---------------- harness ----------------

fn runtime(
    provider: Arc<dyn Provider>,
    gate: PermissionGate,
    executor: Option<Arc<dyn ToolExecutor + Send + Sync>>,
) -> Runtime {
    let dir = std::env::temp_dir().join(format!("tool-exec-{}", uuid::Uuid::new_v4()));
    let mut builder = Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .gate(gate)
        .config(RuntimeConfig::default_v0());
    if let Some(executor) = executor {
        builder = builder.tool_executor(executor);
    }
    builder.build().expect("runtime")
}

fn craft_session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "工具执行接线".into(),
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
}

/// 历史里第一条 `ToolResult` 消息。
fn tool_result_message(rt: &Runtime, id: uuid::Uuid) -> agent_core::schema::Message {
    rt.get_session(id)
        .expect("session")
        .messages
        .into_iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("ToolResult 消息在历史里")
}

/// receipt 中 `done` 事件载荷（要求恰一条）。
fn done_payload(receipt: &TurnReceipt) -> Value {
    let dones: Vec<_> = receipt.events.iter().filter(|e| e.kind == "done").collect();
    assert_eq!(dones.len(), 1, "回合必须恰一个 done 收口");
    dones[0].payload.clone()
}

// ---------------- 契约 ----------------

/// Allow + 执行成功 ⇒ 历史出现真实结果的 `ToolResult` 消息，下一步 provider
/// 重读会话历史时能看到它（llm.ts:191-198 步循环的 reload 语义）。
#[test]
fn allow_executes_and_settles_real_result() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ]);
    let result = json!({ "lines": 3, "first": "hi" });
    let exec = ScriptedExecutor::ok(result.clone());
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 a.txt").expect("turn");

    assert_eq!(exec.calls(), 1, "Allow 必须真实执行恰一次");
    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        result.to_string(),
        "历史 ToolResult 必须是执行器真实结果（序列化原文）"
    );
    assert_eq!(tr.tool.as_ref().expect("带 tool 信息").name, "fs.read");

    assert_eq!(p.calls(), 2, "工具步结算后续步开新调用");
    let second = p.ctx_at(1);
    let seen = second
        .history
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("第 2 次调用的历史必须含真实 ToolResult");
    assert_eq!(seen.content, result.to_string());
    assert!(receipt.events.iter().any(|e| e.kind == "tool_result"));
}

/// Deny ⇒ 执行器一次都没被调用，历史落 deny 结果，回合继续（首次被拒不终止）。
#[test]
fn deny_never_executes_and_settles_denied_result() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({ "path": "z.txt" })),
        text_step("改走说明"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(p.clone(), denying_gate(), Some(exec.clone().as_dyn()));
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "写 z.txt").expect("turn");

    assert_eq!(exec.calls(), 0, "Deny 必须一次都不执行");
    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        json!({ "status": "denied", "reason": "工具未注册: fs.write" }).to_string(),
        "deny 结果沿用既有 status/reason 形态与权限门文案"
    );
    let event = receipt
        .events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("deny 结算必须发 tool_result 事件");
    assert_eq!(event.payload["isError"], true);
    assert_eq!(event.payload["result"]["status"], "denied");
    assert_eq!(p.calls(), 2, "首次被拒后回合继续（既有 Handled 语义）");
}

/// 同一工具二次被拒 ⇒ 回合以 `done{denied}` 终止，不再发第三次调用（既有
/// `LoopStatus::Denied` 语义在 Runtime 工具步的对应）。
#[test]
fn same_tool_denied_twice_terminates_turn() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({ "path": "a.txt" })),
        tool_step("fs.write", json!({ "path": "b.txt" })),
        text_step("不该出现"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(p.clone(), denying_gate(), Some(exec.clone().as_dyn()));
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "写两个文件").expect("turn");

    assert_eq!(exec.calls(), 0);
    assert_eq!(p.calls(), 2, "二次被拒 ⇒ 终止，不发第 3 次调用");
    let denied_events: Vec<_> = receipt
        .events
        .iter()
        .filter(|e| e.kind == "tool_result" && e.payload["isError"] == true)
        .collect();
    assert_eq!(
        denied_events.len(),
        1,
        "只有首次被拒结算结果，二次被拒直接终止"
    );
    let done = done_payload(&receipt);
    assert_eq!(done["status"], "denied");
    assert_eq!(done["tool"], "fs.write");
    assert_eq!(done["steps"], 2);
}

/// 执行返回 Err ⇒ `{"type":"error","message":..}` 且 `isError=true` 结算成
/// 结果，回合继续到下一步（step.ts:129-131 failTool 语义，不中断回合）。
#[test]
fn executor_error_settles_is_error_and_turn_continues() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "bad.txt" })),
        text_step("换个路径重试"),
    ]);
    let exec = ScriptedExecutor::err("读失败: 文件不存在");
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 bad.txt").expect("turn");

    assert_eq!(exec.calls(), 1, "Allow 下执行过一次（失败也算执行）");
    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        json!({ "type": "error", "message": "读失败: 文件不存在" }).to_string(),
        "失败结算成结果进历史，模型下一步可见"
    );
    let event = receipt
        .events
        .iter()
        .find(|e| e.kind == "tool_result" && e.payload["isError"] == true)
        .expect("失败结算必须发 isError 事件");
    assert_eq!(event.payload["result"]["type"], "error");
    assert_eq!(event.payload["result"]["message"], "读失败: 文件不存在");
    assert_eq!(p.calls(), 2, "失败后回合继续（failTool 而非中断）");
    assert_eq!(done_payload(&receipt)["status"], "completed");
}

/// 大输出（> `CONTENT_PREVIEW_CHARS`）⇒ 消息只留 preview，`tool.result_ref`
/// 非空（完整结果进 blob）。
#[test]
fn large_output_keeps_preview_and_goes_to_blob() {
    let big = "x".repeat(CONTENT_PREVIEW_CHARS + 100);
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "big.txt" })),
        text_step("读完"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "data": big }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    rt.send_text(s.id, "读 big.txt").expect("turn");

    let tr = tool_result_message(&rt, s.id);
    let full = json!({ "data": big }).to_string();
    assert!(
        tr.content.chars().count() < full.chars().count(),
        "消息只留 preview，不得全文进历史"
    );
    assert!(
        tr.content.contains("截断"),
        "preview 必须带截断标记: {}",
        tr.content
    );
    let result_ref = tr.tool.as_ref().expect("带 tool 信息").result_ref.clone();
    assert!(
        result_ref.as_deref().is_some_and(|r| !r.is_empty()),
        "大输出必须进 blob（result_ref 非空）"
    );
}

/// 未注入执行器 ⇒ 仍是 pending 占位，不产生 `tool_result` 事件（兼容项）。
#[test]
fn without_executor_keeps_pending_placeholder() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("收工"),
    ]);
    let rt = runtime(p.clone(), allowing_gate("fs.read"), None);
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 a.txt").expect("turn");

    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        r#"{"status":"pending","note":"工具调用已持久化，执行由宿主承担"}"#
    );
    assert!(
        !receipt.events.iter().any(|e| e.kind == "tool_result"),
        "占位结算不冒充真实结果，不发 tool_result 事件"
    );
    assert_eq!(p.calls(), 2);
}

/// Ask（需用户确认）⇒ 不执行、不阻塞，落 pending 占位交宿主审批链
/// （`request_approval` → `record_tool_result` 既有契约），回合继续。
#[test]
fn ask_settles_pending_for_host_adjudication() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "secret.txt" })),
        text_step("等你确认"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        asking_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 secret.txt").expect("turn");

    assert_eq!(exec.calls(), 0, "Ask 未获确认不得执行");
    let tr = tool_result_message(&rt, s.id);
    assert!(
        tr.content.contains("pending"),
        "Ask 落 pending 占位（宿主审批后回填真实结果）: {}",
        tr.content
    );
    assert!(
        !receipt.events.iter().any(|e| e.kind == "tool_result"),
        "Ask 占位不发 tool_result 事件"
    );
    assert_eq!(p.calls(), 2, "回合继续，由宿主审批链接管");
}

/// 一步一调用：相邻两次 provider 调用之间历史已重读——第 2 次调用的历史里
/// 第 1 步的 ToolCall 与 ToolResult 成对在列（llm.ts:191-198 步循环）。
#[test]
fn one_provider_call_per_step_with_reloaded_history() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        tool_step("fs.read", json!({ "path": "b.txt" })),
        text_step("两个都读完了"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 a 和 b").expect("turn");

    assert_eq!(p.calls(), 3, "三个步 = 三次调用，无 in-memory tool loop");
    assert_eq!(exec.calls(), 2);

    let second = p.ctx_at(1);
    assert!(
        second
            .history
            .iter()
            .any(|m| m.role == Role::ToolCall
                && m.tool.as_ref().is_some_and(|t| t.name == "fs.read")),
        "第 2 次调用的历史必须含第 1 步 ToolCall"
    );
    assert!(
        second
            .history
            .iter()
            .any(|m| m.role == Role::ToolResult
                && m.tool.as_ref().is_some_and(|t| t.name == "fs.read")),
        "第 2 次调用的历史必须含第 1 步 ToolResult（成对在列）"
    );

    let third = p.ctx_at(2);
    let paired = third
        .history
        .iter()
        .filter(|m| matches!(m.role, Role::ToolCall | Role::ToolResult))
        .count();
    assert_eq!(
        paired, 4,
        "第 3 次调用历史里两步 ToolCall/ToolResult 共 4 条"
    );
    assert_eq!(done_payload(&receipt)["steps"], 3);
}
