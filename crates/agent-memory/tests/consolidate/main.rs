//! h5-memory-distill 集成测试入口：机械聚合 / 幂等水位 / 30 天窗口删除 / 失败面。
//!
//! 本文件是 Cargo 自动发现的测试目标（`tests/consolidate.rs`），主题用例在
//! `tests/consolidate/` 子目录中；无需改 `Cargo.toml`。

mod aggregate;
mod failures;
mod idempotent;
mod prune;
mod support;
