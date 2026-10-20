//! agent-core：智能体运行时核心域逻辑。
//!
//! 只包含纯域逻辑（会话/事件 schema、事件日志、工具注册表、权限门、脱敏），
//! 不依赖 tokio 或任何 IO 框架；网关层负责运行时与网络。
//! 规格源：worktree 根 `AGENT-KICKOFF.md` §3–§5。

pub mod blobs;
pub mod bus;
pub mod config;
pub mod credentials;
pub mod error;
pub mod event_log;
pub mod executor;
pub mod host;
#[allow(clippy::module_inception)]
pub mod r#loop;
pub mod mode;
pub mod permission;
pub mod provider;
pub mod redact;
pub mod run_status; // 模块由 h1-run-status-schema 声明；`pub use` 由 h1-host-services 统一收口
pub mod runtime;
pub mod schema;
pub mod sensitive;
pub mod tool;
pub mod workspace_read;

pub use credentials::{
    CredentialError, StoreCredentialResolver, CRED_REF_SCHEME,
};
pub use error::CoreError;
pub use event_log::{EventLog, JsonlEventLog};
pub use executor::{ExecToolExecutor, EXEC_TOOL_NAME};
pub use host::{CredentialResolver, HostServices, McpToolDispatcher, NoopCredentialResolver};
pub use mode::{mode_policy, ModePolicy};
pub use permission::{Decision, PermissionGate};
pub use provider::{Provider, ProviderStep, TurnContext};
pub use r#loop::{
    AgentLoop, ChatProvider, ChatTurn, LoopLimits, LoopOutcome, LoopStatus, PlanGate, ToolCall,
    ToolExecutor, DEFAULT_MAX_STEPS,
};
pub use redact::Redactor;
pub use run_status::{
    recover, DisplayStatus, ExecutionStage, Recovery, RunEvent, RunState, RunStatus,
    TerminalOutcome, STALE_THRESHOLD_MS, TERMINATED_REASON,
};
pub use runtime::{CreateSession, Runtime, RuntimeBuilder, TurnReceipt};
pub use schema::{Event, Isolation, Message, Mode, PermissionMode, Role, Session, SCHEMA_VERSION};
pub use tool::{default_registry, ToolKind, ToolRegistry, ToolSpec};
