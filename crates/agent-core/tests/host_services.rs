//! HostServices 组装口（C1）契约测试。
//!
//! 覆盖矩阵：
//! - `HostServices::default()`（各字段 `None`）⇒ 既有行为逐字节不变（零回归）；
//! - `mcp:<server>:<tool>` 形状的调用被路由到注入的 `McpToolDispatcher`，
//!   且**不调用**注入的 [`ToolExecutor`]（两条路径互斥、不双跑）；
//! - 其他名字（含 `mcp:` 前缀畸形名）⇒ 仍走既有执行器路径，受影响为零；
//! - MCP 调用失败 ⇒ 既有 `Err(message)` 结算（`isError=true`、回合继续、不 panic）；
//! - `credentials: None` ⇒ `resolve` 保持既有 fail-closed 桩语义。
//!
//! 断言只守不变量，不断言 `HostServices` 的字段数量。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::host::{
    CredentialResolver, HostServices, McpToolDispatcher, NoopCredentialResolver,
};
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

// ---------------- 脚本化 provider（与 runtime_tool_exec 同款观测面） ----------------

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

    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("HostServices 测试只走带上下文接口"))
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
            .ok_or(CoreError::Unsupported("脚本已耗尽"))
    }
}

/// 单步：一个工具调用。
fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

/// 单步：纯文本收敛。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

// ---------------- 计数执行器（既有路径的观测面） ----------------

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

    fn calls(&self) -> usize {
        self.calls.lock().expect("锁").len()
    }

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

// ---------------- 记录型 MCP 端口（替身，不改 agent-mcp 公开接口） ----------------

/// `McpToolDispatcher` 的替身：记录 `(server, tool, args)`，回放脚本化结果。
///
/// 真实 `TrustedClient` 的构造需要 spawn 子进程且其 `call_tool` 是 `async`,
/// 对同步用例不友好；本替身实现同一 trait，故「路由到 MCP 端口」这条链路
/// 可被完整断言，而不必为测试改动 `agent-mcp`。
struct StubMcp {
    calls: Mutex<Vec<(String, String, Value)>>,
    outcome: Result<Value, String>,
}

impl StubMcp {
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

    fn calls(&self) -> Vec<(String, String, Value)> {
        self.calls.lock().expect("锁").clone()
    }

    fn as_dyn(self: Arc<Self>) -> Arc<dyn McpToolDispatcher> {
        self
    }
}

impl McpToolDispatcher for StubMcp {
    fn call_tool(&self, server: &str, tool: &str, args: &Value) -> Result<Value, String> {
        self.calls
            .lock()
            .expect("锁")
            .push((server.to_string(), tool.to_string(), args.clone()));
        self.outcome.clone()
    }
}

// ---------------- harness ----------------

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

/// 注册 + 白名单 + auto 放行 ⇒ `Decision::Allow`（与 runtime_tool_exec 同口径）。
fn allowing_gate(name: &str) -> PermissionGate {
    PermissionGate::new(registry_with(name))
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
        .with_auto_allowed(name)
}

fn runtime(
    provider: Arc<dyn Provider>,
    gate: PermissionGate,
    executor: Option<Arc<dyn ToolExecutor + Send + Sync>>,
    host: HostServices,
) -> Runtime {
    let dir = std::env::temp_dir().join(format!("host-services-{}", uuid::Uuid::new_v4()));
    let mut builder = Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .gate(gate)
        .config(RuntimeConfig::default_v0())
        .host_services(host);
    if let Some(executor) = executor {
        builder = builder.tool_executor(executor);
    }
    builder.build().expect("runtime")
}

fn craft_session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "宿主能力".into(),
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

// ---------------- 契约 ----------------

/// `mcp:<server>:<tool>` 形状的调用 ⇒ 路由到注入的 MCP 端口，且既有执行器零调用。
#[test]
fn mcp_prefixed_call_routes_to_dispatcher_not_executor() {
    let p = ScriptedProvider::new(vec![
        tool_step("mcp:fs-server:read_file", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ]);
    let mcp = StubMcp::ok(json!({ "content": "hello" }));
    let exec = ScriptedExecutor::ok(json!({ "should": "not-run" }));
    let host = HostServices {
        mcp_dispatch: Some(mcp.clone().as_dyn()),
        ..HostServices::default()
    };
    let rt = runtime(
        p.clone(),
        allowing_gate("mcp:fs-server:read_file"),
        Some(exec.clone().as_dyn()),
        host,
    );
    let s = craft_session(&rt);

    rt.send_text(s.id, "读 a.txt").expect("turn");

    assert_eq!(
        mcp.calls(),
        vec![(
            "fs-server".to_string(),
            "read_file".to_string(),
            json!({ "path": "a.txt" })
        )],
        "mcp: 形状调用必须投递到端口，server/tool/args 按 id 拆解"
    );
    assert_eq!(exec.calls(), 0, "命中 MCP 路由时既有执行器一次都不执行");
    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        json!({ "content": "hello" }).to_string(),
        "历史 ToolResult 必须是 MCP 端口真实返回"
    );
    assert_eq!(
        tr.tool.as_ref().expect("带 tool 信息").name,
        "mcp:fs-server:read_file",
        "历史工具名保持原始 id（不被拆解后的 tool 覆盖）"
    );
}

/// 非 `mcp:` 名字 ⇒ 既有执行器路径不受影响（零回归的正向对照）。
#[test]
fn non_mcp_name_still_uses_executor() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "b.txt" })),
        text_step("读完了"),
    ]);
    let mcp = StubMcp::ok(json!({ "never": true }));
    let exec = ScriptedExecutor::ok(json!({ "lines": 2 }));
    let host = HostServices {
        mcp_dispatch: Some(mcp.clone().as_dyn()),
        ..HostServices::default()
    };
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
        host,
    );
    let s = craft_session(&rt);

    rt.send_text(s.id, "读 b.txt").expect("turn");

    assert_eq!(exec.calls(), 1, "非 mcp: 名字走既有执行器");
    assert_eq!(mcp.calls(), Vec::new(), "既有路径不得触碰 MCP 端口");
    assert_eq!(
        tool_result_message(&rt, s.id).content,
        json!({ "lines": 2 }).to_string()
    );
}

/// `mcp:` 前缀但形状畸形（缺 tool 段）⇒ 不路由，回落既有执行器路径。
#[test]
fn malformed_mcp_id_falls_back_to_executor() {
    for malformed in ["mcp:only-server", "mcp::tool", "mcp:server:"] {
        let p = ScriptedProvider::new(vec![tool_step(malformed, json!({})), text_step("完了")]);
        let mcp = StubMcp::ok(json!({ "never": true }));
        let exec = ScriptedExecutor::ok(json!({ "ok": true }));
        let host = HostServices {
            mcp_dispatch: Some(mcp.clone().as_dyn()),
            ..HostServices::default()
        };
        let rt = runtime(
            p.clone(),
            allowing_gate(malformed),
            Some(exec.clone().as_dyn()),
            host,
        );
        let s = craft_session(&rt);

        rt.send_text(s.id, "试").expect("turn");

        assert_eq!(
            mcp.calls(),
            Vec::new(),
            "畸形 id `{malformed}` 不得被路由到 MCP"
        );
        assert_eq!(
            exec.calls(),
            1,
            "畸形 id `{malformed}` 必须回落既有执行器路径"
        );
    }
}

/// 注入了 MCP 端口，但某次调用的名字不是 `mcp:` 形状 ⇒ 该名字仍走执行器。
#[test]
fn dispatcher_present_does_not_capture_other_names() {
    let p = ScriptedProvider::new(vec![
        tool_step("shell.exec", json!({ "cmd": "ls" })),
        text_step("完了"),
    ]);
    let mcp = StubMcp::ok(json!({ "never": true }));
    let exec = ScriptedExecutor::ok(json!({ "stdout": "" }));
    let host = HostServices {
        mcp_dispatch: Some(mcp.clone().as_dyn()),
        ..HostServices::default()
    };
    let rt = runtime(
        p.clone(),
        allowing_gate("shell.exec"),
        Some(exec.clone().as_dyn()),
        host,
    );
    let s = craft_session(&rt);

    rt.send_text(s.id, "ls").expect("turn");

    assert_eq!(exec.calls(), 1);
    assert_eq!(mcp.calls(), Vec::new());
}

/// MCP 端口返回 `Err` ⇒ 走既有 `Err(message)` 结算路径：`isError=true`、
/// 回合继续到下一步（`failTool` 语义），不 panic。
#[test]
fn mcp_failure_settles_is_error_and_turn_continues() {
    let p = ScriptedProvider::new(vec![
        tool_step("mcp:fs-server:read_file", json!({ "path": "x" })),
        text_step("换个路径重试"),
    ]);
    let mcp = StubMcp::err("门拒绝：工具未登记");
    let exec = ScriptedExecutor::ok(json!({ "should": "not-run" }));
    let host = HostServices {
        mcp_dispatch: Some(mcp.clone().as_dyn()),
        ..HostServices::default()
    };
    let rt = runtime(
        p.clone(),
        allowing_gate("mcp:fs-server:read_file"),
        Some(exec.clone().as_dyn()),
        host,
    );
    let s = craft_session(&rt);

    let receipt = rt
        .send_text(s.id, "读 x")
        .expect("回合不得因 MCP 失败而中断");

    assert_eq!(mcp.calls().len(), 1, "失败也是一次真实投递");
    assert_eq!(exec.calls(), 0, "MCP 失败不回落执行器（不双跑）");
    let tr = tool_result_message(&rt, s.id);
    assert_eq!(
        tr.content,
        json!({ "type": "error", "message": "门拒绝：工具未登记" }).to_string(),
        "MCP 失败沿既有 error 结算形态"
    );
    let event = receipt
        .events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("失败结算必须发 tool_result 事件");
    assert_eq!(event.payload["isError"], true);
    assert_eq!(
        p.calls(),
        2,
        "MCP 失败后回合继续（下一步开新 provider 调用）"
    );
}

/// `HostServices::default()`（各字段 `None`）⇒ 既有行为逐字节不变：`mcp:` 形状
/// 的名字在未注入端口时**照常走执行器**，与 C1 之前完全一致。
#[test]
fn default_host_services_preserves_legacy_path() {
    let p = ScriptedProvider::new(vec![
        tool_step("mcp:fs-server:read_file", json!({ "path": "a.txt" })),
        text_step("read done"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "legacy": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("mcp:fs-server:read_file"),
        Some(exec.clone().as_dyn()),
        HostServices::default(),
    );
    let s = craft_session(&rt);

    rt.send_text(s.id, "读 a.txt").expect("turn");

    assert_eq!(
        exec.calls(),
        1,
        "未注入 MCP 端口时，mcp: 名字也不得改道，须走既有执行器（零回归）"
    );
    assert_eq!(
        tool_result_message(&rt, s.id).content,
        json!({ "legacy": true }).to_string()
    );
}

/// `credentials: None` ⇒ 用 fail-closed 桩语义：`resolve` 一律 `Err`，不返回明文。
#[test]
fn null_credentials_resolve_is_fail_closed() {
    let none = HostServices::default();
    assert!(none.credentials.is_none(), "默认构造下凭据解析未注入");
    // 桩本身即 fail-closed；未注入时运行时不构造任何明文。
    let stub = NoopCredentialResolver;
    let err = stub.resolve("cred_ref://abc").expect_err("必须拒绝");
    assert!(
        !err.contains("cred_ref://abc"),
        "错误文案不得回显引用原文（防泄漏）：{err}"
    );

    // 注入一个真实的解析器以证明 trait 可用于替换桩。
    struct Fixed;
    impl CredentialResolver for Fixed {
        fn resolve(&self, reference: &str) -> Result<String, String> {
            Ok(format!("resolved:{reference}"))
        }
    }
    let services = HostServices {
        credentials: Some(Arc::new(Fixed)),
        ..HostServices::default()
    };
    assert_eq!(
        services
            .credentials
            .as_ref()
            .expect("已注入")
            .resolve("r1")
            .expect("解析"),
        "resolved:r1"
    );
}

/// 端口句柄经 `Runtime` 只读面可取（后续卡 C2–C13 的注入点自检）。
#[test]
fn runtime_exposes_injected_host_services() {
    let host = HostServices {
        mcp_dispatch: Some(StubMcp::ok(json!({})).as_dyn()),
        ..HostServices::default()
    };
    let rt = runtime(
        ScriptedProvider::new(vec![text_step("ok")]),
        allowing_gate("noop"),
        None,
        host,
    );
    assert!(rt.host_services().has_mcp_dispatch());
}
