//! 模型调用 token 用量的记账入口（x-token-usage 批 2）。
//!
//! 一次 provider 调用成功拿到步骤序列后，回合层经 [`record_usage`] 把
//! [`TokenUsage`] 落成一条 `model.usage` 事件；**不新建账本文件**，查询与回放
//! 沿用既有的 [`EventLog`]（`session:replay` 即可读到）。
//!
//! 本模块只做记账入口：不做预算上限、超额阻断、金额换算与跨会话累计。

use serde_json::json;
use uuid::Uuid;

use crate::error::CoreError;
use crate::event_log::EventLog;
use crate::provider::TokenUsage;
use crate::schema::Event;

use super::Runtime;

/// `model.usage` 事件的 kind（定死；沿用仓内点分命名）。
pub const MODEL_USAGE_EVENT: &str = "model.usage";

/// 把一次模型调用的 usage 落进事件日志。
///
/// payload 的 5 个键全部必填：`prompt_tokens` / `completion_tokens` /
/// `total_tokens` / `cached_tokens` 直接取自 [`TokenUsage`]，
/// `billed_tokens` **复用** [`TokenUsage::billed_tokens`]，不在别处重算。
pub fn record_usage(
    log: &dyn EventLog,
    session_id: Uuid,
    usage: TokenUsage,
) -> Result<Event, CoreError> {
    log.append(
        session_id,
        MODEL_USAGE_EVENT,
        json!({
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "total_tokens": usage.total_tokens,
            "cached_tokens": usage.cached_tokens,
            "billed_tokens": usage.billed_tokens(),
        }),
    )
}

impl Runtime {
    /// 取 provider 最近一次调用的用量并落一条 `model.usage`；`None` ⇒ 跳过
    /// （usage 缺失时不写全 0 假记录）。
    ///
    /// 调用点：回合原语在 provider 返回步骤序列之后、消费/结算之前。
    pub(super) fn record_provider_usage(
        &self,
        session_id: Uuid,
        events: &mut Vec<Event>,
    ) -> Result<(), CoreError> {
        if let Some(usage) = self.provider.last_usage() {
            events.push(record_usage(self.log.as_ref(), session_id, usage)?);
        }
        Ok(())
    }
}
