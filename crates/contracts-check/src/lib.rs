//! 契约验证 crate：把 `docs/contracts/` 与 `AGENT-KICKOFF.md` 的冻结条款
//! 变成可执行断言。本 crate 不参与生产，只被 `tests/` 使用。
#![forbid(unsafe_code)]

// 契约常量由本卡实现者按 AGENT-KICKOFF.md §2 填充：
// - ACTIONS: &[&str]         v0 全部动作名
// - EVENT_KINDS: &[&str]     v0 全部事件 kind
// - ENVELOPE_CODES: &[u16]   已冻结的响应码
