use std::path::PathBuf;

/// 记忆系统错误（fail-closed：所有失败显式上抛，不静默降级）。
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// 跨会话记忆未开启（`memory.enabled = false`）。
    #[error("跨会话记忆未开启")]
    Disabled,
    /// 超出层容量上限。
    #[error("记忆超出容量上限: cap={cap} len={len}")]
    CapExceeded { cap: usize, len: usize },
    /// 文件系统错误（路径随错误上报，内容不上报）。
    #[error("记忆文件 IO 错误: {path}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// 路径逃逸（越出授权目录）。
    #[error("路径越出授权目录")]
    PathEscape,
    /// 记忆根内路径非法（如经 symlink 访问；判定之外的额外拒绝）。
    #[error("记忆路径非法: {path} ({reason})")]
    InvalidPath { path: String, reason: String },
    /// 检索游标非法（必须是结果序号）。
    #[error("记忆游标非法: {cursor}")]
    InvalidCursor { cursor: String },
    /// 目标路径不存在。
    #[error("记忆路径不存在: {path}")]
    NotFound { path: String },
    /// 目标不是普通文件。
    #[error("不是文件: {path}")]
    NotFile { path: String },
    /// `line_offset` 必须从 1 开始（1-indexed）。
    #[error("line_offset 必须从 1 开始")]
    InvalidLineOffset,
    /// `max_lines` 必须为正整数。
    #[error("max_lines 必须为正整数")]
    InvalidMaxLines,
    /// 起始行超出文件长度。
    #[error("line_offset 超出文件长度")]
    LineOffsetExceedsFileLength,
    /// 查询为空或包含空字符串。
    #[error("queries 不能为空或包含空字符串")]
    EmptyQuery,
    /// `all_within_lines.line_count` 必须为正整数。
    #[error("all_within_lines.line_count 必须为正整数")]
    InvalidMatchWindow,
    /// 名称为空。
    #[error("名称不能为空")]
    EmptyName,
    /// JSON 序列化错误（bundle 面向网关时使用）。
    #[error("记忆序列化失败: {0}")]
    Json(String),
}
