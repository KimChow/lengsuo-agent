//! 单步原语测试（W1-core）：`Runtime::step` 的走向语义 + `run_turn` 步循环化
//! 重构的「行为零变化」金标锚点。
//!
//! 语义出处：金标 opencode `packages/core/src/session/runner/step.ts`（一步 =
//! 一次调用 + 工具 + 结算 + 返回 Outcome）、`llm.ts:191-198`（外层 while 只做
//! 策略与推进）。本卡把「一步」从 `run_turn` 里抽成可复用原语，本文件断言
//! 四种走向与既有可观测行为逐条一致。
//!
//! 覆盖：
//! 1. 无工具 ⇒ `Converged`，事件/消息序列与既有 runtime 测试逐条一致；
//! 2. 有工具 ⇒ `ToolSettled`，会话历史里 ToolCall 与 ToolResult 成对；
//! 3. 末步到限 ⇒ `Surrendered` 恰好一次，`done{max_steps_reached}` 字段不变；
//! 4. 同一工具二次被拒 ⇒ `Aborted`，不再发起下一次 provider 调用；
//! 5. 步数与 `last_seq` 单调不回退；
//! 6. 固定脚本下 `run_turn` 的 `TurnReceipt`（事件序列 + last_seq）与重构前一致。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, ToolChoice, TurnContext};
use agent_core::r#loop::MAX_STEPS_PROMPT;
use agent_core::runtime::{CreateSession, Runtime, StepOutcome, TurnReceipt};
use agent_core::schema::{Event, Isolation, Message, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

/// Runtime 单回合步数上限（与 `runtime/turns.rs` 的 `MAX_TURN_STEPS` 对齐：30）。
const MAX_STEPS: usize = 30;

// ---------------- 脚本化 provider ----------------

/// 按队列出步并记录 (上下文, 选项) 的脚本化 provider：正常步走
/// `next_steps_with_context`，缴械步走 `next_steps_with_options`，两侧调用
/// 都入账供断言。
struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<(TurnContext, Option<StepOptions>)>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// provider 被调用次数 = 实际 LLM 调用数（含缴械步）。
    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }

    /// 第 n 次调用（0 起）的 (上下文, 选项)；选项 `None` 表示正常步入口。
    fn at(&self, n: usize) -> (TurnContext, Option<StepOptions>) {
        self.seen.lock().expect("锁")[n].clone()
    }

    /// 第 n 次调用（0 起）收到的上下文副本。
    fn ctx_at(&self, n: usize) -> TurnContext {
        self.at(n).0
    }

    fn serve(
        &self,
        context: &TurnContext,
        options: Option<StepOptions>,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen
            .lock()
            .expect("锁")
            .push((context.clone(), options));
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported(
                "脚本已耗尽：出现计划外的第 n+1 次调用",
            ))
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "单步原语测试只走带上下文接口（provider-context 卡约束）",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.serve(context, None)
    }

    fn next_steps_with_options(
        &self,
        context: &TurnContext,
        options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.serve(context, Some(*options))
    }
}

// ---------------- 计数执行器 ----------------

/// 记录每次调用并返回同一脚本化结果的执行器。
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

// ---------------- 权限门姿态 ----------------

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

// ---------------- harness ----------------

fn runtime(
    provider: Arc<dyn Provider>,
    gate: PermissionGate,
    executor: Option<Arc<dyn ToolExecutor + Send + Sync>>,
) -> Runtime {
    let dir = std::env::temp_dir().join(format!("step-primitive-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
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
        title: "单步原语".into(),
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

/// 事件 kind 序列。
fn kinds(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.kind.as_str()).collect()
}

/// 事件 seq 序列（单调性断言面）。
fn seqs(events: &[Event]) -> Vec<u64> {
    events.iter().map(|e| e.seq).collect()
}

/// receipt 里唯一的 `done` 事件载荷。
fn done_payload(receipt: &TurnReceipt) -> Value {
    let dones: Vec<&Event> = receipt.events.iter().filter(|e| e.kind == "done").collect();
    assert_eq!(dones.len(), 1, "回合必须恰一个 done 收口");
    dones[0].payload.clone()
}

/// 无工具、单步收敛的脚本（text + done）。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

/// 一步以工具调用终结（无文本、无 Finished）。
fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

/// 30 个工具步 + 末步脚本的完整回合脚本。
fn full_script(final_step: Vec<ProviderStep>) -> Vec<Vec<ProviderStep>> {
    let mut script: Vec<Vec<ProviderStep>> = (0..MAX_STEPS)
        .map(|i| tool_step("fs.read", json!({ "path": format!("a{i}.txt") })))
        .collect();
    script.push(final_step);
    script
}

// ---------------- 契约 ----------------

/// 以「回合首步」口径驱动一步原语：回合前历史快照 + 本轮输入。
fn first_step(
    rt: &Runtime,
    id: uuid::Uuid,
    text: &str,
    denied: &mut BTreeSet<String>,
    events: &mut Vec<Event>,
) -> Result<StepOutcome, CoreError> {
    let history = rt.get_session(id).expect("session").messages;
    rt.step(id, &history, text, &StepOptions::default(), denied, events)
}

/// 以「续步」口径驱动一步原语：重读会话历史、本轮输入不再重复出现。
fn reloaded_step(
    rt: &Runtime,
    id: uuid::Uuid,
    denied: &mut BTreeSet<String>,
    events: &mut Vec<Event>,
) -> Result<StepOutcome, CoreError> {
    let history = rt.get_session(id).expect("session").messages;
    rt.step(id, &history, "", &StepOptions::default(), denied, events)
}

/// 无工具 ⇒ `Converged`：步内只落产出事件（done 属回合收口，不在原语里），
/// 事件/消息序列与既有 runtime 测试逐条一致（`["text"]` / 整回合 `["text","done"]`）。
#[test]
fn no_tool_step_converges_with_existing_event_shape() {
    let p = ScriptedProvider::new(vec![text_step("收到，收工"), text_step("再跑答复")]);
    let rt = runtime(p.clone(), denying_gate(), None);
    let s = craft_session(&rt);

    let mut events = Vec::new();
    let mut denied = BTreeSet::new();
    let outcome = first_step(&rt, s.id, "开工", &mut denied, &mut events).expect("step");
    assert_eq!(outcome, StepOutcome::Converged);
    assert_eq!(kinds(&events), vec!["text"], "步内只落产出事件");
    assert_eq!(events[0].payload["text"], "收到，收工");
    let messages = rt.get_session(s.id).expect("session").messages;
    assert_eq!(messages.last().expect("有消息").role, Role::Assistant);
    assert_eq!(messages.last().expect("有消息").content, "收到，收工");

    // 同款脚本走整回合：receipt = 步事件 + done{completed}，与既有 runtime 测试
    // （step_semantics_contract C1b）逐条一致。
    // 因为 user 事件入流而更新期望（W1-core）：整回合事件面现以 user 开头。
    let receipt = rt.send_text(s.id, "再跑").expect("turn");
    assert_eq!(
        kinds(&receipt.events),
        vec!["user", "text", "done"],
        "因 user 事件入流而更新的整回合 kind 序列"
    );
    assert_eq!(receipt.events[0].payload["text"], "再跑");
    assert_eq!(receipt.events[1].payload["text"], "再跑答复");
    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 1 })
    );
    assert_eq!(p.calls(), 2);
}

/// 有工具 ⇒ `ToolSettled`：`tool_use → tool_result` 落事件，会话历史里
/// ToolCall 与 ToolResult 成对；续步（重读历史）后收敛，第 2 次调用的历史
/// 里成对在列。
#[test]
fn tool_step_settles_and_history_pairs_call_with_result() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "lines": 3 }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let mut events = Vec::new();
    let mut denied = BTreeSet::new();
    let outcome = first_step(&rt, s.id, "读 a.txt", &mut denied, &mut events).expect("step");
    assert_eq!(outcome, StepOutcome::ToolSettled);
    assert_eq!(kinds(&events), vec!["tool_use", "tool_result"]);
    assert_eq!(exec.calls(), 1, "Allow 必须真实执行恰一次");

    // 会话历史：ToolCall 与 ToolResult 成对在列。
    let messages = rt.get_session(s.id).expect("session").messages;
    let call = messages
        .iter()
        .find(|m| m.role == Role::ToolCall)
        .expect("ToolCall 在历史");
    assert_eq!(call.tool.as_ref().expect("带 tool 信息").name, "fs.read");
    let result = messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("ToolResult 在历史");
    assert_eq!(result.content, json!({ "lines": 3 }).to_string());

    // 续步（重读历史）⇒ 收敛；第 2 次调用的历史里两类消息成对在列。
    let outcome2 = reloaded_step(&rt, s.id, &mut denied, &mut events).expect("step 2");
    assert_eq!(outcome2, StepOutcome::Converged);
    assert_eq!(p.calls(), 2);
    let second = p.ctx_at(1);
    assert!(second.history.iter().any(|m| m.role == Role::ToolCall));
    assert!(second.history.iter().any(|m| m.role == Role::ToolResult));
}

/// 末步到限（原语面）：前 30 个工具步全部 `ToolSettled`（无一步
/// `Surrendered`），第 31 步（缴械步：会话历史 + `MAX_STEPS_PROMPT` 副本 +
/// `toolChoice=none`）恰返回一次 `Surrendered`。
#[test]
fn surrender_outcome_returned_exactly_once_at_limit() {
    let p = ScriptedProvider::new(full_script(text_step("到限总结")));
    let rt = runtime(p.clone(), denying_gate(), None);
    let s = craft_session(&rt);

    let mut events = Vec::new();
    let mut denied = BTreeSet::new();
    for n in 0..MAX_STEPS {
        let outcome = if n == 0 {
            first_step(&rt, s.id, "读全部", &mut denied, &mut events)
        } else {
            reloaded_step(&rt, s.id, &mut denied, &mut events)
        }
        .expect("工具步");
        assert_eq!(
            outcome,
            StepOutcome::ToolSettled,
            "第 {} 步不得提前 Surrendered",
            n + 1
        );
    }

    // 缴械步政策（run_turn 同款）：会话历史 + MAX_STEPS_PROMPT 临时副本 + none。
    let mut history = rt.get_session(s.id).expect("session").messages;
    history.push(Message {
        role: Role::Assistant,
        content: MAX_STEPS_PROMPT.to_string(),
        tool: None,
        ts_ms: 0,
    });
    let outcome = rt
        .step(
            s.id,
            &history,
            "",
            &StepOptions {
                tool_choice: ToolChoice::None,
            },
            &mut denied,
            &mut events,
        )
        .expect("缴械步");
    assert_eq!(outcome, StepOutcome::Surrendered);
    assert_eq!(p.calls(), MAX_STEPS + 1, "缴械步之后不再有调用");
}

/// 同一工具二次被拒 ⇒ `Aborted`：首次被拒结算 isError 结果（续跑），第二次
/// 直接 `Aborted{tool}`，不再发起下一次 provider 调用。
#[test]
fn same_tool_denied_twice_aborts_without_further_provider_call() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({ "path": "a.txt" })),
        tool_step("fs.write", json!({ "path": "b.txt" })),
        text_step("不该出现"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(p.clone(), denying_gate(), Some(exec.clone().as_dyn()));
    let s = craft_session(&rt);

    let mut events = Vec::new();
    let mut denied = BTreeSet::new();
    let first = first_step(&rt, s.id, "写两个文件", &mut denied, &mut events).expect("step 1");
    assert_eq!(first, StepOutcome::ToolSettled, "首次被拒结算结果后续跑");

    let second = reloaded_step(&rt, s.id, &mut denied, &mut events).expect("step 2");
    assert_eq!(
        second,
        StepOutcome::Aborted {
            tool: "fs.write".to_string()
        }
    );
    assert_eq!(exec.calls(), 0, "Deny 从未真实执行");
    assert_eq!(p.calls(), 2, "Aborted 后不再发起下一次 provider 调用");
    // 只有首次被拒结算了一条 isError tool_result；二次被拒不结算、回合终止。
    let errors: Vec<&Event> = events
        .iter()
        .filter(|e| e.kind == "tool_result" && e.payload["isError"] == true)
        .collect();
    assert_eq!(errors.len(), 1);
    // 二次请求的 tool_use 仍先落（模型确实发出了请求），随后即止。
    assert_eq!(kinds(&events), vec!["tool_use", "tool_result", "tool_use"]);
}

/// 末步到限（整回合面）：31 次调用 = 30 工具步 + 1 缴械步；缴械步以
/// `toolChoice=none` 发起、`MAX_STEPS_PROMPT` 作为 assistant 消息临时进请求
/// 历史；`done{max_steps_reached}` 字段与重构前逐字一致。
#[test]
fn max_steps_surrender_done_fields_unchanged() {
    let p = ScriptedProvider::new(full_script(text_step("到限总结：已读完全部文件。")));
    let rt = runtime(p.clone(), denying_gate(), None);
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读全部文件").expect("turn");

    assert_eq!(p.calls(), MAX_STEPS + 1, "30 个工具步 + 1 缴械步");
    let (ctx, options) = p.at(MAX_STEPS);
    assert_eq!(
        options,
        Some(StepOptions {
            tool_choice: ToolChoice::None
        }),
        "缴械步必须以 toolChoice=none 走带选项入口（金标 llm.ts:244）"
    );
    let last = ctx.history.last().expect("缴械请求有历史");
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content, MAX_STEPS_PROMPT);

    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], MAX_STEPS + 1, "steps 含缴械步");
    assert_eq!(done["maxSteps"], MAX_STEPS);
}

/// 步数与 `last_seq` 单调不回退：receipt 内 seq 严格递增、`last_seq` 收在最后
/// 一条；跨回合 `last_seq` 只增不减；每回合步计数从 1 重置且等于该回合调用数。
#[test]
fn steps_and_last_seq_never_go_backwards() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        tool_step("fs.read", json!({ "path": "b.txt" })),
        text_step("两个都读完了"),
        text_step("第二轮完成"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read"),
        Some(exec.clone().as_dyn()),
    );
    let s = craft_session(&rt);

    let first = rt.send_text(s.id, "读 a 和 b").expect("turn 1");
    assert_eq!(done_payload(&first)["steps"], 3, "3 步 = 3 次调用");
    let seqs1 = seqs(&first.events);
    assert!(
        seqs1.windows(2).all(|w| w[0] < w[1]),
        "回合内事件 seq 严格递增: {seqs1:?}"
    );
    let after_first = rt.get_session(s.id).expect("session").last_seq;
    assert_eq!(after_first, *seqs1.last().expect("有事件"));
    assert_eq!(first.last_seq, after_first);

    let second = rt.send_text(s.id, "再跑一轮").expect("turn 2");
    assert_eq!(done_payload(&second)["steps"], 1, "步计数按回合重置");
    assert_eq!(p.calls(), 4);
    let seqs2 = seqs(&second.events);
    assert!(seqs2.windows(2).all(|w| w[0] < w[1]), "seqs2 不回退");
    assert!(
        seqs2[0] > after_first,
        "第二回合事件 seq 必须大于第一回合收口: {seqs2:?} vs {after_first}"
    );
    assert!(second.last_seq > after_first, "last_seq 跨回合单调不回退");
}

/// 固定脚本下 `run_turn` 的 `TurnReceipt` 与重构前逐条一致：事件 kind 序列、
/// 逐条载荷、seq 单调、消息顺序（User → ToolCall → ToolResult → Assistant）、
/// 同批尾随文本不进本步（由 reload 后的新步重新产出）。
#[test]
fn run_turn_receipt_matches_pre_refactor_golden() {
    let p = ScriptedProvider::new(vec![
        vec![
            ProviderStep::Thinking("先想".into()),
            ProviderStep::ToolCall {
                name: "fs.read".into(),
                args: json!({ "path": "g.txt" }),
            },
            ProviderStep::Text("同批尾随文本不该被消费".into()),
        ],
        text_step("完事"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(p.clone(), allowing_gate("fs.read"), Some(exec.as_dyn()));
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读 g.txt").expect("turn");

    // 事件序列（重构前金标）：thinking → tool_use → tool_result → text → done；
    // 因 user 事件入流（W1-core）而更新期望：回合事件面多一条 user 开头事件，
    // 其后逐条不变。不变式未削弱——序列仍逐项相等。
    assert_eq!(
        kinds(&receipt.events),
        vec![
            "user",
            "thinking",
            "tool_use",
            "tool_result",
            "text",
            "done"
        ],
        "事件 kind 序列必须逐条一致"
    );
    assert_eq!(receipt.events[0].payload["text"], "读 g.txt");
    assert_eq!(receipt.events[1].payload["text"], "先想");
    assert_eq!(receipt.events[2].payload["name"], "fs.read");
    assert_eq!(
        receipt.events[2].payload["args"],
        json!({ "path": "g.txt" })
    );
    // Allow 结算的 tool_result 事件载荷 = push_tool_result 的摘要面。
    assert_eq!(receipt.events[3].payload["name"], "fs.read");
    assert_eq!(receipt.events[3].payload["preview"], "{\"ok\":true}");
    assert_eq!(receipt.events[3].payload["chars"], 11);
    assert_eq!(receipt.events[3].payload["resultRef"], Value::Null);
    assert_eq!(receipt.events[4].payload["text"], "完事");
    assert_eq!(
        receipt.events[5].payload,
        json!({ "status": "completed", "steps": 2 })
    );

    let receipt_seqs = seqs(&receipt.events);
    assert!(receipt_seqs.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(receipt.last_seq, *receipt_seqs.last().expect("有事件"));

    // 消息顺序与本步止于首个工具调用（同批尾随文本不落历史）。
    let messages = rt.get_session(s.id).expect("session").messages;
    let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            Role::User,
            Role::ToolCall,
            Role::ToolResult,
            Role::Assistant
        ]
    );
    assert_eq!(
        messages.last().expect("有消息").content,
        "完事",
        "assistant 文本只能来自 reload 后的新步"
    );
    assert_eq!(p.calls(), 2);
}
