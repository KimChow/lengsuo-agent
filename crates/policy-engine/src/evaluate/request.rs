//! 判定请求 `EvaluateRequest`（规格 §2.1、任务卡 D2）。

use crate::model::{Action, TargetContext};

/// 一次判定请求。
///
/// 不是线格式类型（§1 的线格式冻结在 [`crate::model`]），因此不实现 serde。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluateRequest {
    /// 待判定 URL（原样传入，由判定层归一化）。
    pub url: String,
    /// 被请求的操作。
    pub action: Action,
    /// 判定时刻（毫秒时间戳）：时间只从这里来，判定层不取系统时间。
    pub now_ms: u64,
    /// 访问者上下文（谁在访问）。
    pub targets_ctx: TargetContext,
    /// `click_dom` 的选择器粒度；非 `ClickDom` 操作可忽略（规格 §2.6）。
    /// SPEC §2.1 的字段枚举未列全此字段，但 §2.6/§3 要求选择器命中判定。
    pub selector: Option<String>,
}
