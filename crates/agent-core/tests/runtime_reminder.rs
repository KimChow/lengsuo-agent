//! 交付物提醒（Runtime 面，W1-core）：Craft/Expert 模式下任务文本含写意图
//! （创建/写入/修改）却整回合未成功执行任何写/执行工具且自然收敛 ⇒ 追加
//! 一次提醒回合（`Role::User` 提醒消息 + 正常步循环），整个 `send_text`
//! 至多一次；缴械（`Surrendered`）与被拒终止（`Aborted`）不提醒。
//!
//! 行为出处：wb 参考件 18 册 §4（present outcome / final answer 为强制级，
//! 「含写意图却只回一句话」必须再推一步真实交付）；本仓既有实现
//! `src/loop/run.rs` 的 `needs_reminder` 分支（本卡把该规则下沉到
//! `Runtime::run_turn`，loop 侧收敛由下一张卡处理）。
//!
//! 记账点：`runtime/execute.rs` 执行成功分支（`Decision::Allow` 且执行器
//! 返回 `Ok`）；deny / 执行失败 / pending 都不算「成功执行」。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

/// Runtime 单回合步数上限（与 `runtime/turns.rs` 的 `MAX_TURN_STEPS` 对齐：30）。
const MAX_STEPS: usize = 30;

/// 提醒文案（逐字沿用 `loop/run.rs` 既有原文）。
const REMINDER_NOTE: &str = "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

// ---------------- 脚本化 provider ----------------

/// 按队列出步并记录上下文的脚本化 provider：只走带上下文接口（回合内不出现
/// 缴械步选项差异时 `next_steps_with_options` 不会被调用，两侧都入账供断言）。
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

    fn serve(&self, context: &TurnContext) -> Result<Vec<ProviderStep>, CoreError> {
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

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "提醒契约测试只走带上下文接口（provider-context 卡约束）",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.serve(context)
    }

    fn next_steps_with_options(
        &self,
        context: &TurnContext,
        options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let _ = options;
        self.serve(context)
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

    /// 被真实执行的次数。
    fn calls(&self) -> usize {
        self.calls.lock().expect("锁").len()
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

// ---------------- harness ----------------

/// 注册 + Craft 白名单 + auto 放行 ⇒ `Decision::Allow`。
fn allowing_gate(name: &str, kind: ToolKind) -> PermissionGate {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: name.to_string(),
        description: "测试工具".to_string(),
        kind,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    PermissionGate::new(registry)
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
        .with_auto_allowed(name)
}

fn runtime(
    provider: Arc<dyn Provider>,
    gate: PermissionGate,
    executor: Option<Arc<dyn ToolExecutor + Send + Sync>>,
) -> Runtime {
    let dir = std::env::temp_dir().join(format!("reminder-{}", uuid::Uuid::new_v4()));
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

fn session_in_mode(rt: &Runtime, mode: Mode) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "交付物提醒".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode,
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

/// receipt 里唯一的 `done` 事件载荷。
fn done_payload(receipt: &TurnReceipt) -> Value {
    let dones: Vec<&Event> = receipt.events.iter().filter(|e| e.kind == "done").collect();
    assert_eq!(dones.len(), 1, "回合必须恰一个 done 收口");
    dones[0].payload.clone()
}

/// 会话历史里提醒 user 消息的条数。
fn reminder_count(rt: &Runtime, id: uuid::Uuid) -> usize {
    rt.get_session(id)
        .expect("session")
        .messages
        .iter()
        .filter(|m| m.role == Role::User && m.content == REMINDER_NOTE)
        .count()
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

// ---------------- 契约 ----------------

/// Craft + 含「创建」关键词 + 全程未执行写工具：自然收敛后恰好追加 1 次
/// provider 调用（提醒回合），会话历史里出现提醒 user 消息；`done` 形状与
/// 不提醒路径一致（仅 steps 相应变大）。
#[test]
fn craft_keyword_without_write_appends_one_reminder_turn() {
    let p = ScriptedProvider::new(vec![
        text_step("好的，我已经完成了"),
        text_step("这次真的写入了"),
    ]);
    let rt = runtime(p.clone(), allowing_gate("fs.write", ToolKind::Write), None);
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");

    assert_eq!(p.calls(), 2, "原回合 1 次 + 提醒回合 1 次");
    assert_eq!(reminder_count(&rt, s.id), 1, "提醒 user 消息恰一条");

    // 提醒事件（thinking 载荷，与 loop 侧同形）在 receipt 里。
    let reminders: Vec<&Event> = receipt
        .events
        .iter()
        .filter(|e| e.kind == "thinking" && e.payload["reminder"] == REMINDER_NOTE)
        .collect();
    assert_eq!(reminders.len(), 1, "提醒本身落一条 thinking 事件");

    // 消息序：User(输入) → Assistant(答复) → User(提醒) → Assistant(再答复)。
    let messages = rt.get_session(s.id).expect("session").messages;
    let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );

    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 2 }),
        "done 形状不变，steps 含提醒回合首步"
    );
}

/// 提醒后模型才调用写工具 ⇒ 正常继续并收敛：写工具被真实执行、事件里有
/// `tool_result`，不再二次提醒。
#[test]
fn reminder_turn_write_tool_executes_and_converges() {
    let p = ScriptedProvider::new(vec![
        text_step("好的，我完成了"),
        tool_step("fs.write", json!({ "path": "a.txt" })),
        text_step("已写入 a.txt"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.write", ToolKind::Write),
        Some(exec.clone()),
    );
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");

    assert_eq!(p.calls(), 3, "原回合 1 + 提醒回合工具步 1 + 续步 1");
    assert_eq!(exec.calls(), 1, "写工具被真实执行恰一次");
    assert_eq!(reminder_count(&rt, s.id), 1);

    let results: Vec<&Event> = receipt
        .events
        .iter()
        .filter(|e| e.kind == "tool_result" && e.payload["name"] == "fs.write")
        .collect();
    assert_eq!(results.len(), 1, "事件里有 fs.write 的 tool_result");
    assert_ne!(results[0].payload["isError"], true, "成功执行不是错误结果");

    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 3 })
    );
}

/// 已成功执行写工具 ⇒ 不提醒：provider 调用次数与无提醒路径一致（脚本本
/// 身的 2 次调用，无第 3 次）。
#[test]
fn write_tool_executed_skips_reminder() {
    let p = ScriptedProvider::new(vec![
        tool_step("fs.write", json!({ "path": "a.txt" })),
        text_step("写完了"),
    ]);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.write", ToolKind::Write),
        Some(exec.clone()),
    );
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");

    assert_eq!(exec.calls(), 1);
    assert_eq!(p.calls(), 2, "与无提醒路径调用数一致（不多不少）");
    assert_eq!(reminder_count(&rt, s.id), 0, "已执行写工具不得提醒");
    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 2 })
    );
}

/// Ask / Plan / Quick 模式 ⇒ 不提醒：逐模式断言各恰 1 次调用、无提醒消息。
#[test]
fn ask_plan_quick_modes_never_remind() {
    let p = ScriptedProvider::new(vec![
        text_step("ask 答复"),
        text_step("plan 答复"),
        text_step("quick 答复"),
    ]);
    let rt = runtime(p.clone(), allowing_gate("fs.write", ToolKind::Write), None);

    for (mode, answer) in [
        (Mode::Ask, "ask 答复"),
        (Mode::Plan, "plan 答复"),
        (Mode::Quick, "quick 答复"),
    ] {
        let s = session_in_mode(&rt, mode);
        let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");
        assert_eq!(reminder_count(&rt, s.id), 0, "{mode:?} 不提醒");
        assert_eq!(
            receipt
                .events
                .iter()
                .find(|e| e.kind == "text")
                .expect("有文本事件")
                .payload["text"],
            answer
        );
        assert_eq!(
            done_payload(&receipt),
            json!({ "status": "completed", "steps": 1 }),
            "{mode:?} 单步收敛，无追加调用"
        );
    }
    assert_eq!(p.calls(), 3, "三模式各恰 1 次调用");
}

/// Craft + 无关键词（如「解释一下这个函数」）⇒ 不提醒。
#[test]
fn craft_input_without_keyword_skips_reminder() {
    let p = ScriptedProvider::new(vec![text_step("这是个加法函数")]);
    let rt = runtime(p.clone(), allowing_gate("fs.write", ToolKind::Write), None);
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "解释一下这个函数").expect("turn");

    assert_eq!(p.calls(), 1);
    assert_eq!(reminder_count(&rt, s.id), 0);
    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 1 })
    );
}

/// 提醒回合仍未调用写工具 ⇒ 不再提醒第二次：provider 调用次数恰好 2，
/// 提醒 user 消息恰一条。
#[test]
fn reminder_fires_at_most_once_per_send_text() {
    let p = ScriptedProvider::new(vec![text_step("好了"), text_step("还是没动工具")]);
    let rt = runtime(p.clone(), allowing_gate("fs.write", ToolKind::Write), None);
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");

    assert_eq!(p.calls(), 2, "提醒回合不再触发第二次提醒");
    assert_eq!(reminder_count(&rt, s.id), 1);
    assert_eq!(
        done_payload(&receipt),
        json!({ "status": "completed", "steps": 2 })
    );
}

/// 到限缴械路径（`Surrendered`）⇒ 不提醒：30 个工具步 + 1 缴械步收口，
/// 全程含关键词却无提醒消息。
#[test]
fn surrendered_turn_never_reminds() {
    let mut script: Vec<Vec<ProviderStep>> = (0..MAX_STEPS)
        .map(|i| tool_step("fs.read", json!({ "path": format!("a{i}.txt") })))
        .collect();
    script.push(text_step("到限总结"));
    let p = ScriptedProvider::new(script);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read", ToolKind::Read),
        Some(exec.clone()),
    );
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "写入全部文件").expect("turn");

    assert_eq!(p.calls(), MAX_STEPS + 1, "30 工具步 + 1 缴械步，无提醒调用");
    assert_eq!(reminder_count(&rt, s.id), 0, "缴械收口不提醒");
    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], MAX_STEPS + 1);
}

/// 步数上限共享：原回合恰在第 30 步收敛 ⇒ 提醒回合首步即到限转缴械步
/// （不重置计数），`done{max_steps_reached}` 收口且 steps 含缴械步。
#[test]
fn reminder_turn_shares_max_turn_steps_budget() {
    let mut script: Vec<Vec<ProviderStep>> = (0..MAX_STEPS - 1)
        .map(|i| tool_step("fs.read", json!({ "path": format!("a{i}.txt") })))
        .collect();
    script.push(text_step("收敛了"));
    script.push(text_step("到限总结"));
    let p = ScriptedProvider::new(script);
    let exec = ScriptedExecutor::ok(json!({ "ok": true }));
    let rt = runtime(
        p.clone(),
        allowing_gate("fs.read", ToolKind::Read),
        Some(exec.clone()),
    );
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "写入全部文件").expect("turn");

    assert_eq!(
        p.calls(),
        MAX_STEPS + 1,
        "29 工具步 + 1 收敛步 + 1 缴械步（提醒回合不重置步数）"
    );
    assert_eq!(reminder_count(&rt, s.id), 1, "提醒发生过恰一次");
    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], MAX_STEPS + 1);
}
