//! `agent-tools`：参考客户端 `file:*` 底层能力层的 Rust 实现。
//!
//! 工具（协议层）之下的能力层，把安全闸、回收站删除、组回滚与原子落盘
//! 收在一处：所有路径先过 `FsToolbox::resolve`（词法闸 → 符号链接解析 →
//! 工作区包含判定 → 受保护分量 deny），任一步不通过即 `Denied`。
//! 同步 IO，不依赖 tokio；不使用 `unsafe`。

#![forbid(unsafe_code)]

mod edit;
mod error;
mod fs_ops;
mod pathguard;
mod read;
mod sandbox_integrity;
mod search;
mod types;

#[cfg(all(test, unix))]
mod testutil;

pub use error::ToolError;
pub use fs_ops::FsToolbox;
pub use types::{EntryInfo, FileInfo, SearchHit, WriteOp};

/// 单文件读取硬上限：8 MiB。`read` 的有效上限是 `max_bytes` 与本值取小。
pub const MAX_SINGLE_READ_BYTES: u64 = 8 * 1024 * 1024;
