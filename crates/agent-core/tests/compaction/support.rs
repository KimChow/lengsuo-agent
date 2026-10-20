//! 摘要式上下文压缩测试的共享 helper：常量镜像、估算口与消息构造器。
//!
//! 常量与模板是 src 私有常量的**契约镜像**：源常量一旦变动，用例会失配，
//! 迫使其同步更新。
#![allow(dead_code)]
use agent_core::runtime::compaction::{
    render_checkpoint, Compaction, CompactionSettings, TokenEstimator,
};
use agent_core::schema::{Message, Role};

/// 保留区默认 token 预算，镜像 src 的 `DEFAULT_KEEP`（照抄 opencode `keep: 15_000`）。
pub const DEFAULT_KEEP: usize = 15_000;
/// 窗口至少留出的 token，镜像 src 的 `RESERVE_MIN`（照抄 opencode `compaction.ts:97`）。
pub const RESERVE_MIN: i64 = 16_000;
/// 窗口未知时的假定值，镜像 src 的 `UNKNOWN_WINDOW`（照抄 `compaction.ts:99`）。
pub const UNKNOWN_WINDOW: i64 = 200_000;
/// 工具输出截断上限，镜像 src 的 `TOOL_OUTPUT_MAX_CHARS`（照抄 `compaction.ts:100`）。
pub const TOOL_OUTPUT_MAX_CHARS: usize = 1_250;

/// 固定估算口：按字符数计（1 字符 = 1 token），便于手算切点。
pub struct CharEstimator;

impl TokenEstimator for CharEstimator {
    fn estimate(&self, text: &str) -> usize {
        text.chars().count()
    }
}

/// user 消息。
pub fn user(content: &str) -> Message {
    Message {
        role: Role::User,
        content: content.to_string(),
        tool: None,
        ts_ms: 0,
    }
}

/// assistant 消息。
pub fn assistant(content: &str) -> Message {
    Message {
        role: Role::Assistant,
        content: content.to_string(),
        tool: None,
        ts_ms: 0,
    }
}

/// 工具调用消息（带 args 引用，形态与运行时一致）。
pub fn tool_call(name: &str, content: &str) -> Message {
    Message {
        role: Role::ToolCall,
        content: content.to_string(),
        tool: Some(agent_core::schema::ToolInfo {
            name: name.to_string(),
            args_ref: Some("blob-1".to_string()),
            result_ref: None,
            ms: None,
        }),
        ts_ms: 0,
    }
}

/// 工具结果消息。
pub fn tool_result(content: &str) -> Message {
    Message {
        role: Role::ToolResult,
        content: content.to_string(),
        tool: Some(agent_core::schema::ToolInfo {
            name: "read".to_string(),
            args_ref: None,
            result_ref: None,
            ms: None,
        }),
        ts_ms: 0,
    }
}

/// 压缩产物（checkpoint 消息）。
pub fn checkpoint(summary: &str, recent: &str) -> Message {
    Message {
        role: Role::User,
        content: render_checkpoint(summary, recent),
        tool: None,
        ts_ms: 0,
    }
}

/// 组一个压缩器：窗口固定、保留区固定、估算口可注入。
pub fn compaction(window: Option<i64>, keep: usize) -> Compaction {
    Compaction::new(
        CompactionSettings {
            auto: true,
            buffer: None,
            keep,
            window,
        },
        Box::new(CharEstimator),
    )
}
