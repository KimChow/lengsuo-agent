//! 工具结果落库：大输出进 blob，消息只留摘要（防分页打爆）。
//!
//! 事件流里的 `tool_result` 由三类真实结算产生：宿主经
//! [`Runtime::record_tool_result`] 回填的真实结果、缴械步的拒绝结算
//! （`Runtime::reject_disabled_tool`，`turns.rs` 私有），与注入执行器后
//! `Runtime::execute_tool` 的真实/失败/deny 结算；pending 占位不冒充真实结果、不落事件。
//!
//! 消息时间与事件 `payload.tsMs` 用**同一次时钟读数**（`Runtime::turn_ts`），
//! 使「重启重建逐条相等」成立。

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::schema::{Message, Role, ToolInfo};

use super::{narrow, Runtime, CONTENT_PREVIEW_CHARS};

impl Runtime {
    /// 把工具结果写进会话历史（大输出进 blob，消息只留摘要），不落事件。
    ///
    /// 事件流里的 `tool_result` 由三类真实结算产生：宿主经
    /// [`Runtime::record_tool_result`] 回填的真实结果、缴械步的拒绝结算
    /// （[`Runtime::reject_disabled_tool`]，本文件私有），与注入执行器后
    /// [`Runtime::execute_tool`] 的真实/失败/deny 结算；pending 占位
    /// 不冒充真实结果、不落事件。
    pub(super) fn push_tool_result(
        &self,
        id: Uuid,
        tool: &str,
        args: Value,
        result: &str,
    ) -> Result<(Message, Value), CoreError> {
        let args = self.redactor.redact_json(&args);
        let args_ref = self
            .blobs
            .put(args.to_string().as_bytes())
            .map(|r| r.0)
            .ok();
        let raw = self.redactor.redact_str(result);
        let (preview, result_ref) = if raw.chars().count() > CONTENT_PREVIEW_CHARS {
            let blob = self.blobs.put(raw.as_bytes())?;
            (narrow(&raw, CONTENT_PREVIEW_CHARS), Some(blob.0))
        } else {
            (raw.clone(), None)
        };
        let tool_info = ToolInfo {
            name: tool.to_string(),
            args_ref,
            result_ref: result_ref.clone(),
            ms: None,
        };
        // 一次时钟读数同时用于消息与事件 payload：消息时间随事件落盘。
        let ts = self.turn_ts();
        let message = Message {
            role: Role::ToolResult,
            content: preview.clone(),
            tool: Some(tool_info.clone()),
            ts_ms: ts,
        };
        self.push_message(id, Role::ToolResult, preview, Some(tool_info), ts)?;
        let payload = json!({
            "name": tool,
            "chars": raw.chars().count(),
            "resultRef": result_ref,
            "preview": message.content,
            "tsMs": ts,
        });
        Ok((message, payload))
    }
}
