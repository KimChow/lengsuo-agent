//! 会话重建测试共享 harness：脚本化 provider、计数执行器、日志写入与断言小工具。
//!
//! `session_rebuild.rs` 与 `session_rebuild_edges.rs` 以 `mod session_rebuild_support;` 引入。
//! 拆自原 713 行的单文件（>700 硬上限），逻辑逐字未改。
#![allow(dead_code)]

//! 启动时从事件日志重建会话消息历史（W1-core）：读侧投影。
//!
//! 语义约定：durable 事件是权威，消息行只是它的投影——按 `seq` 升序读出会话
//! 消息行、逐行解码、坏行跳过。
//! 映射表、分页坑、坏事件策略与 `args_ref` 缺口见 `docs/session-rebuild-status.md`。
//!
//! 覆盖（对应卡面 6 例）：无工具回合重启逐条相等；工具回合 ToolCall/ToolResult 成对
//! 保留（`args_ref` 为 None 的显式断言）；提醒回合顺序 user/assistant/user/assistant；
//! 旧日志缺 `user` 事件只还原可还原部分；畸形 payload 跳过该条；`last_seq` 一致且
//! 重建后继续 `send_text` 接在历史之后。另加：分页取全量、只读、大输出 preview、
//! 非消息 kind 不投影共 4 例（合计 10 例）。

pub use std::collections::{BTreeSet, VecDeque};
pub use std::sync::{Arc, Mutex};

pub use serde_json::{json, Value};
pub use uuid::Uuid;

pub use agent_core::config::RuntimeConfig;
pub use agent_core::error::CoreError;
pub use agent_core::permission::PermissionGate;
pub use agent_core::provider::{Provider, ProviderStep, StepOptions, TurnContext};
pub use agent_core::runtime::{CreateSession, Runtime};
// CONTENT_PREVIEW_CHARS 由 _edges 侧直接引入（主路径用不到，避免 unused 警告）。
pub use agent_core::schema::{Isolation, Message, Mode, PermissionMode, Role, Session};
pub use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
pub use agent_core::ToolExecutor;

/// 提醒文案（逐字沿用 `loop/run.rs` 既有原文，与 `runtime_user_event.rs` 同表）。
pub const REMINDER_NOTE: &str = "尚未调用任何写/执行工具：请实际执行并给出交付物，不要声称已完成。";

// ---------------- 脚本化 provider（既有契约测试同款观测面） ----------------

pub struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<TurnContext>>,
}

impl ScriptedProvider {
    pub fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    pub fn serve(&self, context: &TurnContext) -> Result<Vec<ProviderStep>, CoreError> {
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

/// 建一个属于固定隔离主体的会话（重启对比用同一 workdir）。
pub fn create(rt: &Runtime, mode: Mode) -> Session {
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

pub fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

pub fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

// ---------------- 计数执行器 ----------------

pub struct ScriptedExecutor {
    calls: Mutex<Vec<(String, Value)>>,
    outcome: Result<Value, String>,
}

impl ScriptedExecutor {
    pub fn ok(value: Value) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            outcome: Ok(value),
        })
    }

    pub fn as_dyn(self: Arc<Self>) -> Arc<dyn ToolExecutor + Send + Sync> {
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
pub fn workdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("session-rebuild-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

/// 注册 + Craft 白名单 + auto 放行 ⇒ `Decision::Allow`。
pub fn allowing_gate(name: &str) -> PermissionGate {
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
pub fn reopen(dir: &std::path::Path, provider: Arc<dyn Provider>) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(provider)
        .gate(allowing_gate("fs.read"))
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("reopen runtime")
}

pub fn scripted(calls: usize) -> Arc<ScriptedProvider> {
    ScriptedProvider::new((0..calls).map(|_| text_step("重启后应答")).collect())
}

pub fn isolation() -> Isolation {
    Isolation {
        user_id: "u".into(),
        tenant_id: "t".into(),
        env_id: None,
    }
}

/// 手写一份旧格式日志（header + 逐行事件），供绕过运行期的重建用例使用。
pub fn write_log(dir: &std::path::Path, sid: Uuid, lines: &[String]) {
    // 日志目录由 `JsonlEventLog::new` 在真实运行时创建；测试直接手写文件，
    // 因此由本辅助函数保证父目录存在（不许每个用例各自记得建一次）。
    let events = dir.join("events");
    std::fs::create_dir_all(&events).expect("events dir");
    std::fs::write(
        events.join(format!("events-{sid}.jsonl")),
        lines.join("\n") + "\n",
    )
    .expect("write log");
}

/// 日志 header 行（`kind: "header"`，不入事件序列）。
pub fn header_line(sid: Uuid) -> String {
    format!(r#"{{"schemaVersion":1,"kind":"header","sessionId":"{sid}"}}"#)
}

/// 原始日志行（跳过 header）。
pub fn log_events(dir: &std::path::Path, id: Uuid) -> Vec<Value> {
    std::fs::read_to_string(dir.join("events").join(format!("events-{id}.jsonl")))
        .expect("read log")
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str::<Value>(l).expect("json line"))
        .collect()
}

/// 会话日志里四类**可投影**事件的 `ts_ms`（seq 升序，`replay` 返回即升序）；
/// 其余 kind（`session_update` / `done` 等）不产出消息，不在对比之列。
pub fn projected_event_timestamps(dir: &std::path::Path, id: Uuid) -> Vec<u64> {
    log_events(dir, id)
        .iter()
        .filter(|e| PROJECTED_KINDS.contains(&e["kind"].as_str().expect("kind")))
        .map(|e| e["tsMs"].as_u64().expect("tsMs"))
        .collect()
}

/// 四类可投影事件 kind（其余不产出消息行）。
pub const PROJECTED_KINDS: [&str; 4] = ["user", "text", "tool_use", "tool_result"];

/// 消息的可重建投影（role / content / ts_ms；`tool` 单独断言）。
pub fn shape(messages: &[Message]) -> Vec<(Role, String, u64)> {
    messages
        .iter()
        .map(|m| (m.role, m.content.clone(), m.ts_ms))
        .collect()
}

pub fn tool_of(message: &Message) -> &agent_core::schema::ToolInfo {
    message.tool.as_ref().expect("消息必须带 tool 信息")
}

/// 工具回合脚本：一步工具调用 + 一步纯文本收敛。
pub fn tool_turn_provider() -> Arc<ScriptedProvider> {
    ScriptedProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ])
}

pub fn runtime_with_executor(
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
