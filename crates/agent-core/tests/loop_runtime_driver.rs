//! `AgentLoop::with_runtime` 驱动路径的契约测试（缺口 B1/B4 的验收）。
//!
//! 三条不变式（卡面验收 3）：
//! 1. **每步从会话重读历史**：外部直接改写会话消息后，下一步请求即反映该改写
//!    ——证明 `loop` 没有用本层缓存的副本（对齐金标 `llm.ts` 的 `context.load`）；
//! 2. **工具由 `Runtime` 执行**：注入 Runtime 的执行器被调用、结果进入**会话消息**，
//!    `loop` 只补发既有形状的事件、不写会话历史；
//! 3. **步数上限与末步缴械仍成立**：`with_runtime` 路径同样在到限时以
//!    `toolChoice=none` 走缴械步收口。
//!
//! Runtime 侧 provider 用 `Provider`（不同回合上下文）；loop 侧不再自持 provider，
//! 故此处只需实现 Runtime 的 `Provider` 与执行器。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::{AgentLoop, Event, LoopLimits, LoopStatus, PlanGate, ToolExecutor};

// ---------------- Runtime 侧脚本 provider ----------------

/// 脚本化 provider：按队列出步，记录每次收到的 [`TurnContext`]；可选在每次调用
/// **之前**向会话注入一条外部消息（模拟「外部直接改写会话」）。
struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<TurnContext>>,
    /// 每次调用收到的请求选项（`None` = 走无选项入口）。
    seen_options: Mutex<Vec<Option<StepOptions>>>,
    /// `(runtime, session, text)`：每次 `next_steps_with_context` 前把 `text` 作为
    /// user 消息写进会话——证明 loop 每步都从会话**重读**，而非用缓存副本。
    inject: Mutex<Option<(Runtime, Uuid, String)>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
            seen_options: Mutex::new(Vec::new()),
            inject: Mutex::new(None),
        })
    }

    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }

    fn ctx_at(&self, n: usize) -> TurnContext {
        self.seen.lock().expect("锁")[n].clone()
    }

    /// 第 n 次调用（0 起）收到的请求选项。
    fn options_at(&self, n: usize) -> Option<StepOptions> {
        self.seen_options.lock().expect("锁")[n]
    }

    /// 装一个「调用前外部改写会话」的钩子（见字段注释）。
    fn inject_before_each_call(&self, runtime: Runtime, session: Uuid, text: &str) {
        *self.inject.lock().expect("锁") = Some((runtime, session, text.to_string()));
    }

    /// 记录上下文、执行外部注入钩子、按脚本出步（两个入口共用）。
    fn serve(&self, context: &TurnContext) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("锁").push(context.clone());
        // 外部改写：在本次请求上下文已快照之后、返回步之前，把消息写进会话——
        // 下一步的上下文若反映它，即证明 loop 每步重读会话。
        if let Some((runtime, session, text)) = self.inject.lock().expect("锁").clone() {
            runtime
                .append_message(session, Role::User, &text)
                .expect("注入外部消息");
        }
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported("脚本已耗尽"))
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "loop_runtime_driver 只走带上下文接口",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen_options.lock().expect("锁").push(None);
        self.serve(context)
    }

    /// 缴械步带选项入口：与无选项入口同款（记录上下文 + 注入 + 出步）。
    fn next_steps_with_options(
        &self,
        context: &TurnContext,
        options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen_options.lock().expect("锁").push(Some(*options));
        self.serve(context)
    }
}

fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

// ---------------- 计数执行器 ----------------

/// 记录每次调用并返回固定结果（`Send + Sync`，可注入 Runtime）。
struct CountingExecutor {
    calls: Mutex<Vec<String>>,
    outcome: Result<Value, String>,
}

impl CountingExecutor {
    fn ok(value: Value) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            outcome: Ok(value),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("锁").clone()
    }

    fn as_dyn(self: Arc<Self>) -> Arc<dyn ToolExecutor + Send + Sync> {
        self
    }
}

impl ToolExecutor for CountingExecutor {
    fn execute(&self, name: &str, _args: &Value) -> Result<Value, String> {
        self.calls.lock().expect("锁").push(name.to_string());
        self.outcome.clone()
    }
}

// ---------------- 组装 ----------------

fn registry_with(name: &str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: name.to_string(),
        description: "测试工具".to_string(),
        kind: ToolKind::Write,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    registry
}

/// 注册 + 白名单(Mode::Craft) + auto 放行 ⇒ Runtime 权限门 `Decision::Allow`。
fn allowing_gate(name: &str) -> PermissionGate {
    PermissionGate::new(registry_with(name))
        .with_whitelist(
            Mode::Craft,
            std::collections::BTreeSet::from([name.to_string()]),
        )
        .with_auto_allowed(name)
}

fn build_runtime(
    provider: Arc<dyn Provider>,
    gate: PermissionGate,
    executor: Arc<dyn ToolExecutor + Send + Sync>,
) -> Runtime {
    let dir = std::env::temp_dir().join(format!("loop-rt-{}", Uuid::new_v4()));
    Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .gate(gate)
        .config(RuntimeConfig::default_v0())
        .tool_executor(executor)
        .build()
        .expect("runtime")
}

fn craft_session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession {
        title: "loop runtime driver".into(),
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

/// 事件采集（loop 侧发出的既有形状事件）。
#[derive(Default)]
struct Rec {
    events: Mutex<Vec<Event>>,
}

impl Rec {
    fn push(&self, ev: Event) {
        self.events.lock().expect("锁").push(ev);
    }

    fn kinds(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("锁")
            .iter()
            .map(|e| e.kind.clone())
            .collect()
    }

    fn tool_ids(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .expect("锁")
            .iter()
            .filter(|e| e.kind == "tool_use" || e.kind == "tool_result")
            .map(|e| {
                (
                    e.kind.clone(),
                    e.payload["id"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }
}

/// 跑一次 `with_runtime` 回合：返回 (结果, 事件采集, 会话消息快照)。
fn run_with_runtime(
    rt: &Runtime,
    session: Uuid,
    executor: Arc<CountingExecutor>,
    limits: LoopLimits,
    input: &str,
) -> (
    Result<agent_core::LoopOutcome, String>,
    Arc<Rec>,
    Vec<agent_core::schema::Message>,
) {
    let rec = Arc::new(Rec::default());
    let rec_for_emit = Arc::clone(&rec);
    let emit = move |ev: Event| rec_for_emit.push(ev);
    let step_exec: Arc<CountingExecutor> = Arc::clone(&executor);
    let mut agent = AgentLoop::with_runtime(
        rt.clone(),
        session,
        Some(step_exec.as_ref() as &dyn ToolExecutor),
        Mode::Craft,
        limits,
        PlanGate::default(),
        &emit,
    );
    let outcome = agent.run(input);
    let messages = rt.get_session(session).expect("session").messages;
    (outcome, rec, messages)
}

// ---------------- 契约 1：每步重读会话 ----------------

#[test]
fn each_step_reloads_history_from_session() {
    // 步 1 返回工具请求（fs.write，由 Runtime 结算进会话），步 2 收敛。
    let provider = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({"path": "/a"})),
        text_step("写完了。"),
    ]);
    let executor = CountingExecutor::ok(json!({"path": "/a", "bytes": 12}));
    let rt = build_runtime(
        provider.clone(),
        allowing_gate("fs.write"),
        Arc::clone(&executor).as_dyn(),
    );
    let session = craft_session(&rt);
    // 外部钩子：每次 provider 调用前把一条标记消息写进会话。
    provider.inject_before_each_call(rt.clone(), session, "外部改写标记");

    let (outcome, _rec, messages) = run_with_runtime(
        &rt,
        session,
        executor,
        LoopLimits { max_steps: 5 },
        "写入 /a",
    );
    let outcome = outcome.expect("应正常收敛");
    assert_eq!(outcome.status, LoopStatus::Completed);

    // 第 2 次 provider 调用（0 起索引 1）看到的历史里，必须含第 1 步由 Runtime 结算
    // 的 fs.write 结果，以及外部注入的那条消息——两者都只在**会话**里，本层缓存
    // 不可能凭空造出「外部改写」这条。这就是「每步重读」的直接证据。
    let second = provider.ctx_at(1).history;
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::ToolResult && m.content.contains("\"bytes\":12")),
        "第 2 步请求须含第 1 步结算的工具结果: {second:?}"
    );
    assert!(
        second.iter().any(|m| m.content.contains("外部改写标记")),
        "第 2 步请求须反映外部写入会话的消息（证明重读、非缓存）: {second:?}"
    );
    // 会话本身也留着这些消息（真相源在会话）。
    assert!(messages.iter().any(|m| m.content.contains("外部改写标记")));
}

// ---------------- 契约 2：工具由 Runtime 执行，本层不写历史 ----------------

#[test]
fn tools_execute_in_runtime_and_session_records_result() {
    let provider = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({"path": "/a"})),
        text_step("完成。"),
    ]);
    let executor = CountingExecutor::ok(json!({"path": "/a", "bytes": 12}));
    let rt = build_runtime(
        provider,
        allowing_gate("fs.write"),
        Arc::clone(&executor).as_dyn(),
    );
    let session = craft_session(&rt);

    let (outcome, rec, messages) = run_with_runtime(
        &rt,
        session,
        executor.clone(),
        LoopLimits { max_steps: 5 },
        "创建文件",
    );
    let outcome = outcome.expect("收敛");
    assert_eq!(outcome.status, LoopStatus::Completed);

    // 执行器真被 Runtime 调用了一次。
    assert_eq!(executor.calls(), vec!["fs.write".to_string()]);

    // 调用与结果成对进**会话消息**（call 由 Runtime 落、result 由 Runtime 结算）。
    assert!(
        messages.iter().any(|m| m.role == Role::ToolCall),
        "会话须有 ToolCall 消息: {messages:?}"
    );
    let result_msg = messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("会话须有 ToolResult 消息");
    assert!(
        result_msg.content.contains("\"bytes\":12"),
        "会话里的结果来自执行器实际返回值: {}",
        result_msg.content
    );

    // 本层只补发既有形状的事件（tool_use / tool_result 以同一 id 配对）。
    let kinds = rec.kinds();
    assert!(kinds.contains(&"tool_use".to_string()));
    assert!(kinds.contains(&"tool_result".to_string()));
    let ids = rec.tool_ids();
    let use_ids: Vec<&String> = ids
        .iter()
        .filter(|(k, _)| k == "tool_use")
        .map(|(_, i)| i)
        .collect();
    let result_ids: Vec<&String> = ids
        .iter()
        .filter(|(k, _)| k == "tool_result")
        .map(|(_, i)| i)
        .collect();
    assert_eq!(use_ids, result_ids, "tool_use 与 tool_result 以 id 对应");
    // 本层**不**把历史写进会话（会话里的 ToolResult 只有 Runtime 结算的那一条）。
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == Role::ToolResult)
            .count(),
        1,
        "本层不得重复结算（会话里恰一条 ToolResult）"
    );
}

// ---------------- 契约 3：步数上限 + 末步缴械 ----------------

#[test]
fn max_steps_and_surrender_hold_on_runtime_path() {
    // max_steps=1：第 1 步工具结算（计 1 步），第 2 次调用即缴械步。
    let provider = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({"path": "/a"})),
        text_step("到限，缴械总结。"),
    ]);
    let executor = CountingExecutor::ok(json!({"path": "/a", "bytes": 12}));
    let rt = build_runtime(
        provider.clone(),
        allowing_gate("fs.write"),
        Arc::clone(&executor).as_dyn(),
    );
    let session = craft_session(&rt);

    let (outcome, rec, _messages) = run_with_runtime(
        &rt,
        session,
        executor,
        LoopLimits { max_steps: 1 },
        "写入 /a",
    );
    let outcome = outcome.expect("到限应正常返回");
    assert_eq!(outcome.status, LoopStatus::MaxStepsReached);
    assert_eq!(outcome.steps, 2, "1 工具步 + 1 缴械步");
    assert_eq!(outcome.final_text, "到限，缴械总结。");
    assert_eq!(provider.calls(), 2, "到限即缴械收口，不超调");
    // 末步以 toolChoice=none 发起（缴械步语义在 `with_runtime` 路径同样成立）。
    assert_eq!(
        provider.options_at(1),
        Some(StepOptions {
            tool_choice: agent_core::provider::ToolChoice::None
        }),
        "末步必须走带选项入口且 toolChoice=none"
    );
    // done 事件报 max_steps_reached。
    let done = rec
        .kinds()
        .iter()
        .position(|k| k == "done")
        .map(|i| rec.events.lock().expect("锁")[i].payload.clone())
        .expect("有 done 事件");
    assert_eq!(done["status"], "max_steps_reached");
}
