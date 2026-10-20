//! 专家团结构视图与团队契约 schema 的测试入口（卡 9）。
//!
//! 覆盖矩阵：
//! - **一致性**（`consistency.rs`）：`TeamView` 的主理人 / 团员两分法、
//!   三处数量关系；
//! - **契约**（`packet.rs`）：`task_packet.json` / `member_handoff.json`
//!   的线格式、往返与关联判定；
//! - **负例**（`negative.rs`）：三个反例各自命中**对应的**错误码。
//!
//! 夹具在 `tests/fixtures/team/`（两份正例 + 三份反例），共享工具在
//! `support.rs`。
mod consistency;
mod negative;
mod packet;
mod support;
