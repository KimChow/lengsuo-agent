//! 用户消息入事件（W1-core）：用户输入在进会话历史的同时落一条 `"user"`
//! durable 事件，使消息序列可被逐条重建（重建本身留给下一张卡）。
//!
//! 语义出处：金标 opencode `packages/core/src/session/projector.ts:618-651`
//! （`SessionEvent.InboxDelivered` 是会话事件面上的首条投递事件，投影出 user
//! 消息）与 `packages/core/src/session/history.ts:164-179`（读侧按事件面重建
//! 消息序列，user 消息不能是空洞）。本仓事件 kind 用字符串 `"user"`，payload
//! `{"text": <脱敏文本>}`——形状见 `docs/user-event-status.md`。
//!
//! 覆盖（对应卡面 5 例）：
//! 1. `send_text` 后日志出现一条 `user` 事件，`payload.text` 与脱敏后文本一致；
//! 2. `receipt.events` 首条即 `user`，且 receipt 的 seq 序列与 `log.replay` 逐项一致；
//! 3. 多条输入 ⇒ 每次一条 `user`，顺序一致，数量等于用户消息条数；
//! 4. 脱敏：输入含敏感片段 ⇒ 事件 text 与历史消息 content 逐字相同；
//! 5. 提醒回合（Craft + 关键词 + 未执行写工具）⇒ 出现两条 `user`（原始 + 提醒），
//!    提醒文案逐字相同。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};

/// 提醒文案（逐字沿用 `loop/run.rs` 既有原文）。
const REMINDER_NOTE: &str = "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

// ---------------- 脚本化 provider ----------------

/// 按队列出步并记录上下文的脚本化 provider：只走带上下文接口。
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
            "用户事件契约测试只走带上下文接口（provider-context 卡约束）",
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

/// 无工具、单步收敛的脚本（text + done）。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

// ---------------- harness ----------------

fn runtime(provider: Arc<dyn Provider>) -> Runtime {
    let dir = std::env::temp_dir().join(format!("user-event-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .gate(allowing_gate("fs.write", ToolKind::Write))
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("runtime")
}

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

fn session_in_mode(rt: &Runtime, mode: Mode) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "用户事件".into(),
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

/// 会话日志里全部 `user` 事件（seq 升序，`replay` 返回即升序）。
fn user_events(rt: &Runtime, id: uuid::Uuid) -> Vec<Event> {
    rt.event_log()
        .replay(id, 0, 500)
        .expect("replay")
        .into_iter()
        .filter(|e| e.kind == "user")
        .collect()
}

// ---------------- 契约 ----------------

/// 例 1：`send_text` 后日志出现一条 `user` 事件，`payload.text` 与脱敏后文本一致。
#[test]
fn send_text_records_one_user_event_with_redacted_text() {
    let p = ScriptedProvider::new(vec![text_step("收到，收工")]);
    let rt = runtime(p.clone());
    let s = session_in_mode(&rt, Mode::Quick);

    let receipt = rt.send_text(s.id, "开工").expect("turn");

    let users = user_events(&rt, s.id);
    assert_eq!(users.len(), 1, "一次输入恰一条 user 事件: {users:?}");
    assert_eq!(users[0].payload["text"], "开工");
    // receipt 与持久日志是同一批事件对象（不得旁路）。
    assert_eq!(users[0], receipt.events[0]);
}

/// 例 2：`receipt.events` 首条即 `user`，且 receipt 的 seq 序列与 `log.replay`
/// 逐项一致（durable record 单队列锚点）。
#[test]
fn receipt_starts_with_user_and_matches_replay_seq() {
    let p = ScriptedProvider::new(vec![text_step("落盘校验")]);
    let rt = runtime(p.clone());
    let s = session_in_mode(&rt, Mode::Quick);

    // 建会话本身先发一条 session_update；契约只比对回合区段。
    let pre_turn_seq = rt.get_session(s.id).expect("session").last_seq;
    let receipt = rt.send_text(s.id, "落盘").expect("turn");
    let replayed = rt
        .event_log()
        .replay(s.id, pre_turn_seq, 500)
        .expect("replay");

    let receipt_seq: Vec<u64> = receipt.events.iter().map(|e| e.seq).collect();
    let replay_seq: Vec<u64> = replayed.iter().map(|e| e.seq).collect();
    assert_eq!(receipt_seq, replay_seq, "receipt 即持久日志，不得旁路");

    let kinds: Vec<&str> = receipt.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds.first().copied(),
        Some("user"),
        "回合事件面第一条必须是 user: {kinds:?}"
    );
    assert_eq!(
        kinds.last().copied(),
        Some("done"),
        "回合事件面必须以 done 收口: {kinds:?}"
    );
    assert_eq!(receipt.events[0].payload["text"], "落盘");
}

/// 例 3：多条输入 ⇒ 每次一条 `user`，顺序一致，数量等于用户消息条数。
#[test]
fn multiple_sends_record_one_user_event_each_in_order() {
    let p = ScriptedProvider::new(vec![
        text_step("第一答复"),
        text_step("第二答复"),
        text_step("第三答复"),
    ]);
    let rt = runtime(p.clone());
    let s = session_in_mode(&rt, Mode::Quick);

    for text in ["第一条", "第二条", "第三条"] {
        rt.send_text(s.id, text).expect("turn");
    }

    let users = user_events(&rt, s.id);
    let texts: Vec<&Value> = users.iter().map(|e| &e.payload["text"]).collect();
    assert_eq!(
        texts,
        vec![&json!("第一条"), &json!("第二条"), &json!("第三条")],
        "user 事件顺序必须与输入顺序一致"
    );

    let user_messages = rt
        .get_session(s.id)
        .expect("session")
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .count();
    assert_eq!(
        users.len(),
        user_messages,
        "user 事件数必须等于用户消息条数（可逐条重建）"
    );
}

/// 例 4：脱敏——输入含敏感片段 ⇒ 事件 text 与历史消息 content 逐字相同
/// （脱敏只做一次，避免 `emit` 二次脱敏改成不同的值）。
#[test]
fn user_event_text_equals_redacted_message_content() {
    let p = ScriptedProvider::new(vec![text_step("收到")]);
    let rt = runtime(p.clone());
    let s = session_in_mode(&rt, Mode::Quick);

    rt.send_text(s.id, "请用 password=hunter2please 登录")
        .expect("turn");

    let users = user_events(&rt, s.id);
    assert_eq!(users.len(), 1, "恰一条 user 事件");
    let event_text = users[0].payload["text"].as_str().expect("text 是字符串");
    assert!(
        !event_text.contains("hunter2please"),
        "事件必须脱敏: {event_text:?}"
    );

    let message = rt
        .get_session(s.id)
        .expect("session")
        .messages
        .iter()
        .find(|m| m.role == Role::User)
        .expect("有 user 消息")
        .content
        .clone();
    assert_eq!(
        event_text, message,
        "事件 text 必须与历史消息 content 逐字相同（脱敏只做一次）"
    );
}

/// 例 5：提醒回合（Craft + 关键词 + 未执行写工具）⇒ 出现两条 `user`
/// （原始 + 提醒），提醒文案逐字相同。
#[test]
fn reminder_turn_records_user_event_for_original_and_reminder() {
    let p = ScriptedProvider::new(vec![text_step("好的，我完成了"), text_step("这次真的写了")]);
    let rt = runtime(p.clone());
    let s = session_in_mode(&rt, Mode::Craft);

    let receipt = rt.send_text(s.id, "创建 a.txt").expect("turn");

    assert_eq!(p.calls(), 2, "原回合 1 次 + 提醒回合 1 次");
    let users = user_events(&rt, s.id);
    assert_eq!(users.len(), 2, "原始输入 + 提醒各一条 user 事件: {users:?}");
    assert_eq!(users[0].payload["text"], "创建 a.txt");
    assert_eq!(users[1].payload["text"], REMINDER_NOTE);

    // 提醒文案与历史里的提醒消息逐字相同（同一份脱敏文本）。
    let session = rt.get_session(s.id).expect("session");
    let reminder_message = session
        .messages
        .iter()
        .find(|m| m.role == Role::User && m.content == REMINDER_NOTE)
        .expect("有提醒 user 消息");
    assert_eq!(users[1].payload["text"], reminder_message.content);

    // 提醒回合的 user 事件必须是回合事件面里的第二条（顺序与消息一致）。
    let kinds: Vec<&str> = receipt.events.iter().map(|e| e.kind.as_str()).collect();
    let user_positions: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == "user")
        .map(|(i, _)| i)
        .collect();
    // 第二条 user 事件的**相对位置**不变式：它在 reminding thinking 之后、
    // 在收口 done 之前（提醒回合自己的 text/done 属于其后）。断言"位置集合"
    // 而不是"距末尾两格"，避免把提醒回合的产出条数写死进断言。
    assert_eq!(
        user_positions.len(),
        2,
        "提醒回合必须恰好两条 user 事件: {kinds:?}"
    );
    assert_eq!(user_positions[0], 0, "首条必须是 user: {kinds:?}");
    let second = user_positions[1];
    assert!(
        second > 0 && second < kinds.len() - 1,
        "第二条 user 必须在 done 之前且在回合中段: {kinds:?}"
    );
    assert_eq!(
        kinds.last().copied(),
        Some("done"),
        "回合事件面必须以 done 收口: {kinds:?}"
    );
}
