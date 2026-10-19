//! 启动时从事件日志重建会话消息历史（W1-core）：读侧投影。
//!
//! 语义出处：金标 opencode `packages/core/src/session/history.ts`（`messageEntries`
//! 按 `seq` 升序读出会话消息行、逐行 `decodeMessageRow` 解码、坏行跳过）与
//! `packages/core/src/session/projector.ts`（durable 事件是权威，消息行是它的投影）。
//! 本仓把这条读侧链路落到 `RuntimeBuilder::build` 的启动回放：进程重启后按事件
//! seq 重放 `user` / `text` / `tool_use` / `tool_result` 四类事件，逐条重建
//! `Session.messages`；其余 kind 与坏事件跳过并计数，不得让运行时起不来。
//!
//! 覆盖（对应卡面 6 例）：
//! 1. 无工具回合后重启 ⇒ 消息逐条相等（role / content / ts_ms）；
//! 2. 工具回合后重启 ⇒ ToolCall + ToolResult 成对保留（`args_ref` 为 None 的显式断言）；
//! 3. 提醒回合 ⇒ 重启后顺序 user / assistant / user / assistant；
//! 4. 旧日志缺 `user` 事件 ⇒ 只重建可还原部分，不 panic；
//! 5. 畸形 payload ⇒ 跳过该条，其余正常重建；
//! 6. `last_seq` 与重启前一致，且重建后继续 `send_text` 接在历史之后。

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, CONTENT_PREVIEW_CHARS};
use agent_core::schema::{Isolation, Message, Mode, PermissionMode, Role, Session};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::ToolExecutor;

/// 提醒文案（逐字沿用 `loop/run.rs` 既有原文，与 `runtime_user_event.rs` 同表）。
const REMINDER_NOTE: &str = "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

// ---------------- 脚本化 provider（既有契约测试同款观测面） ----------------

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
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "会话重建测试只走带上下文接口（provider-context 卡约束）",
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
        _options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
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

// ---------------- harness ----------------

/// 固定 workdir（重启测试必须在同一 workdir 上再建 Runtime）。
fn workdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("session-rebuild-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

/// 注册 + Craft 白名单 + auto 放行 ⇒ `Decision::Allow`。
fn allowing_gate(name: &str) -> PermissionGate {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: name.to_string(),
        description: "测试工具".to_string(),
        kind: ToolKind::Read,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    PermissionGate::new(registry)
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
        .with_auto_allowed(name)
}

/// 在同一 workdir 上新建 Runtime（等价于进程重启后的启动回放）。
fn reopen(dir: &std::path::Path, provider: Arc<dyn Provider>) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(provider)
        .gate(allowing_gate("fs.read"))
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("reopen runtime")
}

fn scripted(calls: usize) -> Arc<ScriptedProvider> {
    ScriptedProvider::new((0..calls).map(|_| text_step("重启后应答")).collect())
}

fn isolation() -> Isolation {
    Isolation {
        user_id: "u".into(),
        tenant_id: "t".into(),
        env_id: None,
    }
}

fn create(rt: &Runtime, mode: Mode) -> Session {
    rt.create_session(&CreateSession {
        title: "会话重建".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode,
        permission_mode: PermissionMode::Full,
        env_binding: None,
        isolation: isolation(),
    })
    .expect("create session")
}

/// 消息的可重建投影（role / content / ts_ms；`tool` 单独断言）。
fn shape(messages: &[Message]) -> Vec<(Role, String, u64)> {
    messages
        .iter()
        .map(|m| (m.role, m.content.clone(), m.ts_ms))
        .collect()
}

fn tool_of(message: &Message) -> &agent_core::schema::ToolInfo {
    message.tool.as_ref().expect("消息必须带 tool 信息")
}

/// 工具回合脚本：一步工具调用 + 一步纯文本收敛。
fn tool_turn_provider() -> Arc<ScriptedProvider> {
    ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ])
}

fn runtime_with_executor(
    dir: &std::path::Path,
    provider: Arc<dyn Provider>,
    exec: Arc<ScriptedExecutor>,
) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(provider)
        .gate(allowing_gate("fs.read"))
        .config(RuntimeConfig::default_v0())
        .tool_executor(exec.as_dyn())
        .build()
        .expect("runtime")
}

// ---------------- 契约 ----------------

/// 例 1：一轮无工具回合后重启 ⇒ 消息与重启前逐条相等（role / content / ts_ms）。
#[test]
fn plain_turn_survives_restart() {
    let dir = workdir("plain");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "你好").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let expected = [
        (Role::User, "你好".to_string()),
        (Role::Assistant, "重启后应答".to_string()),
    ];
    assert_eq!(
        before.messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        expected.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        "前置：重启前历史形状"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        shape(&restored.messages),
        shape(&before.messages),
        "重启后消息必须与重启前逐条相等（role / content / ts_ms）"
    );
}

/// 例 2：工具回合后重启 ⇒ ToolCall + ToolResult 成对保留，`ToolResult.content`
/// 为 preview、`result_ref` 与重启前一致；`ToolCall.args_ref` 为 `None`
/// （`tool_use` 事件 payload 无 `args_ref` 的已知缺口，显式断言）。
#[test]
fn tool_turn_rebuilds_paired_call_and_result() {
    let dir = workdir("tool");
    let before = {
        let rt = runtime_with_executor(
            &dir,
            tool_turn_provider(),
            ScriptedExecutor::ok(json!({"lines": 3})),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "读 a.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let calls: Vec<&Message> = before
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolCall)
        .collect();
    let results: Vec<&Message> = before
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    assert_eq!(
        (calls.len(), results.len()),
        (1, 1),
        "前置：一对 ToolCall/ToolResult"
    );
    assert_eq!(tool_of(results[0]).name, "fs.read");

    let after = runtime_with_executor(&dir, scripted(0), ScriptedExecutor::ok(json!({})));
    let restored = after.get_session(before.id).expect("restored session");
    let rebuilt_calls: Vec<&Message> = restored
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolCall)
        .collect();
    let rebuilt_results: Vec<&Message> = restored
        .messages
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .collect();
    assert_eq!(
        rebuilt_calls.len(),
        1,
        "ToolCall 必须重建: {:?}",
        restored.messages
    );
    assert_eq!(
        rebuilt_results.len(),
        1,
        "ToolResult 必须重建: {:?}",
        restored.messages
    );
    assert_eq!(
        tool_of(rebuilt_calls[0]).name,
        "fs.read",
        "ToolCall 的工具名取自 `tool_use` 事件 payload"
    );
    assert_eq!(
        rebuilt_calls[0].content, calls[0].content,
        "ToolCall.content 经同一 `summarize_args` 重建，必须逐字相同"
    );
    assert_eq!(
        rebuilt_calls[0]
            .tool
            .as_ref()
            .and_then(|t| t.args_ref.clone()),
        None,
        "已知缺口：`tool_use` 事件没有 args_ref，重建只能留 None"
    );
    assert_eq!(
        rebuilt_results[0].content, results[0].content,
        "ToolResult.content 取自事件 payload.preview"
    );
    assert_eq!(
        tool_of(rebuilt_results[0]).result_ref,
        tool_of(results[0]).result_ref,
        "result_ref 必须与重启前一致"
    );
}

/// 例 3：提醒回合（两条 `user` 事件）⇒ 重启后顺序为 user / assistant / user / assistant。
#[test]
fn reminder_turn_keeps_message_order_across_restart() {
    let dir = workdir("reminder");
    let before = {
        let rt = reopen(
            &dir,
            ScriptedProvider::new(vec![text_step("好的，我完成了"), text_step("这次真的写了")]),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "创建 a.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let roles: Vec<Role> = before.messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant],
        "前置：提醒回合的消息序列"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        shape(&restored.messages),
        shape(&before.messages),
        "提醒回合重启后顺序必须为 user/assistant/user/assistant"
    );
    assert_eq!(
        restored.messages[2].content, REMINDER_NOTE,
        "第二条 user 是提醒文案"
    );
}

/// 例 4：旧日志缺 `user` 事件 ⇒ 只重建可还原的部分，不 panic、不伪造用户行。
#[test]
fn legacy_log_without_user_events_rebuilds_partial_history() {
    let dir = workdir("legacy");
    // 直接手写旧格式日志：只有 `text` 事件（本卡之前的日志没有 `user` 事件）。
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let lines = [
        format!(r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"session_update","payload":{{"status":"idle","title":"旧会话","mode":"ask"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"text","payload":{{"text":"旧回复一"}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"text","payload":{{"text":"旧回复二"}},"tsMs":30}}"#
        ),
    ];
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write legacy log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("旧会话必须被启动回放认出来");
    assert_eq!(
        shape(&restored.messages),
        vec![
            (Role::Assistant, "旧回复一".to_string(), 20),
            (Role::Assistant, "旧回复二".to_string(), 30),
        ],
        "缺 user 事件的旧日志只重建可还原部分，不伪造用户行"
    );
    assert_eq!(restored.title, "旧会话", "既有标题回放行为不变");
}

/// 例 5：畸形 payload（`tool_use` 缺 `name`、另一条 `text` 非字符串）⇒ 跳过该条，
/// 其余正常重建；启动不 panic、不中断。
#[test]
fn malformed_payloads_are_skipped_without_aborting_startup() {
    let dir = workdir("malformed");
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let lines = [
        format!(r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#),
        format!(
            r#"{{"seq":1,"sessionId":"{sid}","kind":"user","payload":{{"text":"正常用户"}},"tsMs":10}}"#
        ),
        format!(
            r#"{{"seq":2,"sessionId":"{sid}","kind":"tool_use","payload":{{"args":{{"path":"a.txt"}}}},"tsMs":20}}"#
        ),
        format!(
            r#"{{"seq":3,"sessionId":"{sid}","kind":"text","payload":{{"text":123}},"tsMs":30}}"#
        ),
        format!(
            r#"{{"seq":4,"sessionId":"{sid}","kind":"text","payload":{{"text":"坏事件之后仍要重建"}},"tsMs":40}}"#
        ),
        format!(
            r#"{{"seq":5,"sessionId":"{sid}","kind":"tool_result","payload":{{"name":"fs.read","preview":"ok","resultRef":"blob-1"}},"tsMs":50}}"#
        ),
    ];
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write malformed log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("坏事件不得让运行时起不来");
    assert_eq!(
        shape(&restored.messages),
        vec![
            (Role::User, "正常用户".to_string(), 10),
            (Role::Assistant, "坏事件之后仍要重建".to_string(), 40),
            (Role::ToolResult, "ok".to_string(), 50),
        ],
        "坏事件跳过，其余按 seq 顺序照常重建"
    );
    let last = restored.messages.last().expect("有 ToolResult");
    assert_eq!(tool_of(last).name, "fs.read");
    assert_eq!(tool_of(last).result_ref.as_deref(), Some("blob-1"));
}

/// 例 6：`last_seq` 与重启前一致；重建后继续 `send_text` 接在历史之后（不重复旧消息）。
#[test]
fn last_seq_is_preserved_and_new_turn_appends_after_history() {
    let dir = workdir("seq");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "第一句").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let after = reopen(&dir, scripted(1));
    let restored = after.get_session(before.id).expect("restored session");
    assert_eq!(
        restored.last_seq, before.last_seq,
        "last_seq 仍取 log.last_seq(id)，重启前后一致"
    );

    // 重建后继续对话：历史接在原历史之后，旧消息不重复。
    after
        .send_text(before.id, "第二句")
        .expect("turn after restart");
    let live = after.get_session(before.id).expect("session");
    assert_eq!(
        shape(&live.messages),
        vec![
            (Role::User, "第一句".to_string(), before.messages[0].ts_ms),
            (
                Role::Assistant,
                "重启后应答".to_string(),
                before.messages[1].ts_ms
            ),
            (Role::User, "第二句".to_string(), live.messages[2].ts_ms),
            (
                Role::Assistant,
                "重启后应答".to_string(),
                live.messages[3].ts_ms
            ),
        ],
        "新回合必须接在重建历史之后"
    );
    let users: Vec<&Message> = live
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .collect();
    assert_eq!(users.len(), 2, "旧用户消息不得重复");
    assert_eq!(users[0].content, "第一句");
    assert_eq!(users[1].content, "第二句");
    assert!(live.last_seq > before.last_seq, "last_seq 继续单调前进");
    assert!(
        after.event_log().last_seq(before.id).expect("last_seq") == live.last_seq,
        "会话 last_seq 与日志一致"
    );
}

/// 分页取全量：超过 `DEFAULT_PAGE_LIMIT`（500）条事件的重建不得只取首页。
#[test]
fn rebuild_pages_through_all_events() {
    let dir = workdir("page");
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let mut lines = vec![format!(
        r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#
    )];
    // 600 条 user + 600 条 text 交替（远超单页 500），全部必须重建。
    let total = 600u64;
    for i in 0..total {
        let user_seq = 2 * i + 1;
        let text_seq = 2 * i + 2;
        lines.push(format!(
            r#"{{"seq":{user_seq},"sessionId":"{sid}","kind":"user","payload":{{"text":"u{i}"}},"tsMs":{user_seq}}}"#
        ));
        lines.push(format!(
            r#"{{"seq":{text_seq},"sessionId":"{sid}","kind":"text","payload":{{"text":"a{i}"}},"tsMs":{text_seq}}}"#
        ));
    }
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write long log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("session");
    assert_eq!(
        restored.messages.len(),
        (total * 2) as usize,
        "必须分页取全量，不得只取 500 条就停"
    );
    assert_eq!(restored.messages[0].content, "u0");
    assert_eq!(
        restored.messages.last().expect("last").content,
        format!("a{}", total - 1),
        "分页边界之后的事件也必须重建"
    );
    assert_eq!(restored.last_seq, total * 2);
}

/// 重建是只读操作：启动回放不得为「重建」额外写任何事件。
#[test]
fn rebuild_writes_no_events() {
    let dir = workdir("readonly");
    let before = {
        let rt = reopen(&dir, scripted(1));
        let s = create(&rt, Mode::Quick);
        rt.send_text(s.id, "只读回放").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let log_seq_before = before.last_seq;
    let _after = reopen(&dir, scripted(0));
    let raw = std::fs::read_to_string(
        dir.join("events")
            .join(format!("events-{}.jsonl", before.id)),
    )
    .expect("read log");
    let events: Vec<Value> = raw
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).expect("json line"))
        .collect();
    assert_eq!(
        events.len() as u64,
        log_seq_before,
        "重建是只读操作：事件条数不得变化"
    );
    assert!(
        events
            .iter()
            .all(|e| e["kind"] != "rebuild" && e["kind"] != "replay"),
        "不得为重建写事件"
    );
}

/// 大输出（> `CONTENT_PREVIEW_CHARS`）⇒ 重启后 `content` 仍是 preview、
/// `result_ref` 仍指向 blob（与运行期同一口径）。
#[test]
fn truncated_result_preview_survives_restart() {
    let big = "x".repeat(CONTENT_PREVIEW_CHARS + 100);
    let dir = workdir("trunc");
    let before = {
        let rt = runtime_with_executor(
            &dir,
            ScriptedProvider::new(vec![
                tool_step("fs.read", json!({"path": "big.txt"})),
                text_step("读完"),
            ]),
            ScriptedExecutor::ok(json!({ "data": big })),
        );
        let s = create(&rt, Mode::Craft);
        rt.send_text(s.id, "读 big.txt").expect("turn");
        rt.get_session(s.id).expect("session")
    };
    let expected = before
        .messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("前置：运行期有 ToolResult")
        .clone();
    assert!(
        expected.content.contains("截断"),
        "前置：运行期只留 preview"
    );

    let after = reopen(&dir, scripted(0));
    let restored = after.get_session(before.id).expect("session");
    let got = restored
        .messages
        .iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("ToolResult");
    assert_eq!(got.content, expected.content, "preview 逐字一致");
    assert_eq!(
        tool_of(got).result_ref,
        tool_of(&expected).result_ref,
        "result_ref 指向同一 blob"
    );
}

/// 未识别的 kind 一律跳过：`thinking` / `done` / `warning` / `approval_*` /
/// `task.*` / `session_update` / `files` / `error` 都不产出消息。
#[test]
fn non_message_kinds_are_not_projected() {
    let dir = workdir("kinds");
    let sid = Uuid::new_v4();
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    let kinds = [
        ("thinking", json!({"text": "想一下"})),
        ("done", json!({"status": "completed"})),
        ("warning", json!({"kind": "x"})),
        ("approval_request", json!({"summary": "s"})),
        ("approval_resolved", json!({"approved": true})),
        ("task.created", json!({"id": "t1"})),
        ("session_update", json!({"status": "idle"})),
        ("files", json!({"files": []})),
        ("error", json!({"message": "boom"})),
    ];
    let mut lines = vec![format!(
        r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#
    )];
    for (i, (kind, payload)) in kinds.iter().enumerate() {
        lines.push(format!(
            r#"{{"seq":{},"sessionId":"{sid}","kind":"{kind}","payload":{},"tsMs":{}}}"#,
            i + 1,
            payload,
            (i + 1) * 10
        ));
    }
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write kinds log");

    let rt = reopen(&dir, scripted(0));
    let restored = rt.get_session(sid).expect("session");
    assert!(
        restored.messages.is_empty(),
        "非消息 kind 不得投影出消息: {:?}",
        restored.messages
    );
    assert_eq!(restored.last_seq, kinds.len() as u64, "seq 锚点照常恢复");
}
