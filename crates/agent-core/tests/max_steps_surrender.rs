//! 末步缴械（Runtime 面）：金标 opencode v2
//! `packages/core/src/session/runner/max-steps.ts`（`MAX_STEPS_PROMPT`）、
//! `llm.ts:226-247`（`runStep` 的 `stepLimitReached` 分支）、`step.ts:91-94`
//! （`executeTool` 对 `toolChoice=none` 下的工具请求回错误）。
//!
//! 语义：到限不硬停，追加一次 `toolChoice=none` 的真实 provider 调用并计入
//! steps；`MAX_STEPS_PROMPT` 作为 assistant 消息**临时**进本次请求的历史副本
//! （不写会话历史——它不是真实产出）；该步仍返回工具调用 ⇒ 不执行（不落
//! ToolCall 消息）、回一条错误 `tool_result`（消息 + 事件）；回合以
//! `done{status:"max_steps_reached", steps, maxSteps}` 收口，`steps` 含缴械步。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, StepOptions, ToolChoice, TurnContext};
use agent_core::r#loop::MAX_STEPS_PROMPT;
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode, Role};

/// Runtime 单回合步数上限（与 `runtime/turns.rs` 的 `MAX_TURN_STEPS` 对齐：30）。
const MAX_STEPS: usize = 30;

/// 缴械步工具请求被拒的固定错误（金标 step.ts:92-93 逐字）。
const TOOLS_DISABLED_MESSAGE: &str = "Tools are disabled after the maximum agent steps";

// ---------------- 脚本化 provider ----------------

/// 记录 (上下文, 选项) 的脚本化 provider：正常步走 `next_steps_with_context`，
/// 缴械步走 `next_steps_with_options`，两侧调用都入账供断言。
struct SurrenderProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<(TurnContext, Option<StepOptions>)>>,
}

impl SurrenderProvider {
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

    /// 第 n 次调用（0 起）的 (上下文, 选项)；选项 `None` 表示走旧入口（正常步）。
    fn at(&self, n: usize) -> (TurnContext, Option<StepOptions>) {
        self.seen.lock().expect("锁")[n].clone()
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

impl Provider for SurrenderProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "缴械契约测试只走带上下文接口（provider-context 卡约束）",
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

// ---------------- harness ----------------

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

fn runtime(provider: Arc<dyn Provider>) -> Runtime {
    let dir = std::env::temp_dir().join(format!("surrender-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("runtime")
}

fn craft_session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "末步缴械".into(),
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

/// receipt 里唯一的 `done` 事件载荷。
fn done_payload(receipt: &TurnReceipt) -> Value {
    let dones: Vec<&Event> = receipt.events.iter().filter(|e| e.kind == "done").collect();
    assert_eq!(dones.len(), 1, "回合必须恰一个 done 收口");
    dones[0].payload.clone()
}

/// 30 个只读工具步 + 第 31 步（缴械步）的完整脚本。
fn full_script(final_step: Vec<ProviderStep>) -> Vec<Vec<ProviderStep>> {
    let mut script: Vec<Vec<ProviderStep>> = (0..MAX_STEPS)
        .map(|i| tool_step("fs.read", json!({ "path": format!("a{i}.txt") })))
        .collect();
    script.push(final_step);
    script
}

// ---------------- 契约 ----------------

/// 到限不硬停：第 31 次调用即缴械步——以 `toolChoice=none` 发起、真实 provider
/// 调用并计入 steps；`MAX_STEPS_PROMPT` 作为 assistant 消息临时进请求历史
/// （不写会话历史）；`done{max_steps_reached}` 收口。
#[test]
fn surrender_step_is_a_none_tool_choice_call_counted_in_steps() {
    let p = SurrenderProvider::new(full_script(text_step("到限总结：已读完全部文件。")));
    let rt = runtime(p.clone());
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读全部文件").expect("turn");
    assert_eq!(p.calls(), MAX_STEPS + 1, "30 个工具步 + 1 缴械步");

    let (ctx, options) = p.at(MAX_STEPS);
    assert_eq!(
        options,
        Some(StepOptions {
            tool_choice: ToolChoice::None
        }),
        "缴械步必须以 toolChoice=none 发起（金标 llm.ts:244）"
    );
    let last = ctx.history.last().expect("缴械请求有历史");
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(
        last.content, MAX_STEPS_PROMPT,
        "缴械提示逐字作为 assistant 消息进请求（金标 llm.ts:240-241）"
    );

    // 不写进会话历史：缴械提示不是真实产出。
    let messages = rt.get_session(s.id).expect("session").messages;
    assert!(
        messages.iter().all(|m| m.content != MAX_STEPS_PROMPT),
        "缴械提示不得落进会话历史"
    );

    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], MAX_STEPS + 1, "steps 含缴械步");
    assert_eq!(done["maxSteps"], MAX_STEPS);
}

/// 缴械步仍返回工具调用 ⇒ 不执行：不落新的 ToolCall 消息（不调
/// `record_tool_call`）、不发待执行占位，只回一条错误 `tool_result`（消息 +
/// 事件，金标 step.ts:91-94），随后 `done{max_steps_reached}` 收口。
#[test]
fn surrender_step_tool_call_is_rejected_not_executed() {
    let p = SurrenderProvider::new(full_script(tool_step(
        "fs.write",
        json!({ "path": "z.txt" }),
    )));
    let rt = runtime(p.clone());
    let s = craft_session(&rt);

    let receipt = rt.send_text(s.id, "读全部文件然后写").expect("turn");
    assert_eq!(p.calls(), MAX_STEPS + 1);

    // 缴械步的工具请求不执行：ToolCall 消息仍只有 30 条（均来自正常步）。
    let messages = rt.get_session(s.id).expect("session").messages;
    let tool_calls = messages.iter().filter(|m| m.role == Role::ToolCall).count();
    assert_eq!(
        tool_calls, MAX_STEPS,
        "缴械步的 ToolCall 不落历史（不调 record_tool_call）"
    );

    // 恰一条错误 tool_result 事件，内容为金标错误文本。
    let errors: Vec<Value> = receipt
        .events
        .iter()
        .filter(|e| e.kind == "tool_result" && e.payload["isError"] == true)
        .map(|e| e.payload.clone())
        .collect();
    assert_eq!(errors.len(), 1, "缴械步只回一条错误 tool_result");
    assert_eq!(errors[0]["name"], "fs.write");
    assert_eq!(errors[0]["result"]["type"], "error");
    assert_eq!(errors[0]["result"]["message"], TOOLS_DISABLED_MESSAGE);

    // 错误结果也以消息落历史（ToolResult 角色，带 tool 信息）。
    let last_result = messages
        .iter()
        .rev()
        .find(|m| m.role == Role::ToolResult)
        .expect("错误结果消息在历史里");
    assert_eq!(
        last_result.tool.as_ref().expect("带 tool 信息").name,
        "fs.write"
    );
    assert!(last_result.content.contains(TOOLS_DISABLED_MESSAGE));

    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], MAX_STEPS + 1, "steps 含缴械步");
}
