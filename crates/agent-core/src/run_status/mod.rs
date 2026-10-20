//! 会话运行状态机（wb 26 册 §1）。
//!
//! 本模块只有纯域逻辑：状态三元组 + 九事件转移表 + 两条规则（A26-1 终态保护、
//! A26-2 stale 恢复判定）。不做持久化、不做回放、不发射事件——那是 P0-5。
//!
//! 与既有 [`crate::schema::SessionStatus`] 的关系：`SessionStatus` 是已冻结的
//! 4 值会话状态（`session_update` 事件与网关信封依赖它），本模块的 [`RunStatus`]
//! 是 wb 26 册的 5 状态运行态，挂到 `Session.run_status` 新字段上，两者并存不互改。
//!
//! 按职责拆分（参考 codex `memories/read`：门面只做 mod 声明 + re-export，
//! 单元测试用 `#[path]` 指向同目录测试文件）：
//! - `types`：枚举与常量；
//! - `status`：`RunStatus` 结构与转移/派生；
//! - `recovery`：`Recovery` 决策与 `recover()`。

mod recovery;
mod status;
mod types;

pub use recovery::{recover, Recovery};
pub use status::RunStatus;
pub use types::{
    DisplayStatus, ExecutionStage, RunEvent, RunState, TerminalOutcome, STALE_THRESHOLD_MS,
    TERMINATED_REASON,
};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
