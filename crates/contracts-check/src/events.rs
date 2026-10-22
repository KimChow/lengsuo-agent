/// v0 全部事件 kind，共 11 项——与 `agent-core::schema::EventKind` **逐项对齐**。
///
/// 真相来源：`agent-core` `schema.rs` 的 `enum EventKind`（10 个变体：
/// `thinking / text / tool_use / tool_result / approval_request / approval_resolved /
/// files / done / error / session_update`），加上 v0 实现里**持久化在事件日志中**的
/// `user` 一类，共 11 项。
///
/// `AGENT-KICKOFF.md` 第 23 行只按「参考客户端 基线 §5」抄了七类再加 `session_update`，
/// 实现里稳定存在的三类**未写进该行枚举句**，按「文档缺口（实现为准）」登记：
/// - `user`：`runtime/turns.rs` 落盘 `"user"`，`lengsuo-shell`
///   `state/stream/events.rs` 实现了该分支；
/// - `approval_resolved` / `error`：`agent-core` `EventKind` 有对应变体、`as_str()`/`parse()`
///   都给名字，网关侧 `agent-gateway` `server.rs` 把 `event.kind` 当**任意字符串**原样透传。
///
/// 不要在这里删掉任何一项：本常量是事件面的单一冻结来源，删项会让
/// `tests/event_surface.rs` 的穷举断言静默少跑一圈。
pub const EVENT_KINDS: &[&str] = &[
    "thinking",          // agent-core EventKind::Thinking
    "text",              // agent-core EventKind::Text
    "tool_use",          // agent-core EventKind::ToolUse
    "tool_result",       // agent-core EventKind::ToolResult
    "approval_request",  // agent-core EventKind::ApprovalRequest
    "approval_resolved", // agent-core EventKind::ApprovalResolved（第 23 行缺席，见常量文档注释）
    "files",             // agent-core EventKind::Files
    "done",              // agent-core EventKind::Done
    "error",             // agent-core EventKind::Error（第 23 行缺席，见常量文档注释）
    "session_update",    // agent-core EventKind::SessionUpdate
    "user", // 持久化事件 `user`（非 `agent-core::schema::EventKind` 变体，见常量文档注释）
];
