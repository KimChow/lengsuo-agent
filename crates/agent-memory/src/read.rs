//! `memory.read` 后端：按相对路径读取记忆文件（1-indexed 行窗口 + 预算截断）。
//!
//! 转写自 codex `codex-rs/ext/memories/src/local/read.rs`：同样的
//! `line_offset` / `max_lines` 校验、同样的行首/行尾字节定位、同样的
//! `{path, start_line_number, content, truncated}` 响应。
//!
//! 唯一偏离（调研 D9）：codex 按 **token** 截断，本仓统一按**字符**计数
//! （`MEMORY-SPEC.md:57`），因此 `max_tokens` 预算按字符数执行。

use std::fs;

use serde::Serialize;

use crate::error::MemoryError;
use crate::paths::MemoryPaths;

use super::list::{display_path, metadata_or_none, reject_symlink, resolve_memory_path};

/// `memory.read` 固定预算（照抄 codex `lib.rs:15`；不由模型控制）。
pub const DEFAULT_READ_MAX_TOKENS: usize = 20_000;

/// `memory.read` 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadMemoryRequest {
    pub path: String,
    pub line_offset: usize,
    pub max_lines: Option<usize>,
    pub max_tokens: usize,
}

/// `memory.read` 响应（字段名照抄 codex）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadMemoryResponse {
    pub path: String,
    pub start_line_number: usize,
    pub content: String,
    pub truncated: bool,
}

/// `memory.read`：返回从 `line_offset` 起至多 `max_lines` 行的内容，受预算截断。
pub fn read(
    paths: &MemoryPaths,
    request: ReadMemoryRequest,
) -> Result<ReadMemoryResponse, MemoryError> {
    if request.line_offset == 0 {
        return Err(MemoryError::InvalidLineOffset);
    }
    if request.max_lines == Some(0) {
        return Err(MemoryError::InvalidMaxLines);
    }

    let resolved = resolve_memory_path(paths, Some(request.path.as_str()))?;
    let Some(metadata) = metadata_or_none(&resolved.full)? else {
        return Err(MemoryError::NotFound { path: request.path });
    };
    reject_symlink(&resolved.full, &metadata)?;
    if !metadata.is_file() {
        return Err(MemoryError::NotFile { path: request.path });
    }

    let original_content = fs::read_to_string(&resolved.full)
        .map_err(|err| super::list::io_err(&resolved.full, err))?;
    let start_byte = line_start_byte_offset(&original_content, request.line_offset)?;
    let end_byte = line_end_byte_offset(&original_content, start_byte, request.max_lines);
    let content_from_offset = &original_content[start_byte..end_byte];
    let max_tokens = if request.max_tokens == 0 {
        DEFAULT_READ_MAX_TOKENS
    } else {
        request.max_tokens
    };
    let content = truncate_chars(content_from_offset, max_tokens);
    let truncated = end_byte < original_content.len() || content != content_from_offset;
    Ok(ReadMemoryResponse {
        path: display_path(&resolved, &resolved.full),
        start_line_number: request.line_offset,
        content,
        truncated,
    })
}

/// 1-indexed 行号 → 行首字节偏移；超出文件长度 ⇒ `LineOffsetExceedsFileLength`。
fn line_start_byte_offset(content: &str, line_offset: usize) -> Result<usize, MemoryError> {
    if line_offset == 1 {
        return Ok(0);
    }

    let mut current_line = 1;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            current_line += 1;
            if current_line == line_offset {
                return Ok(index + 1);
            }
        }
    }

    Err(MemoryError::LineOffsetExceedsFileLength)
}

/// 行窗口的结束字节偏移（`max_lines = None` ⇒ 文件尾）。
fn line_end_byte_offset(content: &str, start_byte: usize, max_lines: Option<usize>) -> usize {
    let Some(max_lines) = max_lines else {
        return content.len();
    };

    let mut lines_seen = 1;
    for (relative_index, ch) in content[start_byte..].char_indices() {
        if ch == '\n' {
            if lines_seen == max_lines {
                return start_byte + relative_index + 1;
            }
            lines_seen += 1;
        }
    }

    content.len()
}

/// 字符预算的中段截断（保留首尾，插入 `…N chars truncated…` 标记）。
///
/// 与 codex `truncate_middle_with_token_budget` 同形，仅把计量单位从 token
/// 换成字符（调研 D9）。
fn truncate_chars(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }

    let marker = format!("…{} chars truncated…", total.saturating_sub(max_chars));
    let marker_len = marker.chars().count();
    if max_chars <= marker_len {
        return text.chars().take(max_chars).collect();
    }

    let content_budget = max_chars - marker_len;
    let left_budget = content_budget / 2;
    let right_budget = content_budget - left_budget;
    let left = text.chars().take(left_budget).collect::<String>();
    let right = text
        .chars()
        .skip(total.saturating_sub(right_budget))
        .collect::<String>();
    format!("{left}{marker}{right}")
}
