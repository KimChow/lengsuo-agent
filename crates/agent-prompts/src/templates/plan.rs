//! plan 模式段集（07 册 §3「plan 无专属模板，复用 craft + plan fragments」）。
//!
//! plan **不是另一种人格**：它复用 craft 的全部段（21 册 §3「craft/plan/expert
//! 共用同一份」），差异只由「craft 工具集 + EnterPlanMode/ExitPlanMode 门禁 +
//! plan 专属 task-management」承载（21 册 §5）。本卡做门禁段与 plan 专属的
//! 任务管理段；门禁段的工具名用本仓真实名（`plan.enter` / `plan.exit`）。

use crate::model::{PromptMode, Segment};

use super::common::text;

/// plan 的段集（craft 段 + 门禁段 + plan 专属任务管理段）。
///
/// 门禁段排在能力段之后、硬规则之前（与既有 v0 段序一致）。
pub(crate) fn plan_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::craft::craft_segments(mode);
    // 门禁插在 craft 段之后、硬规则之前：重建顺序 = 头部 + 能力 + 门禁 + 尾。
    let tail = segments.split_off(segments.len() - 2);
    segments.push(Segment::Gate);
    segments.push(text(super::common::TASK_MANAGEMENT_PLAN));
    segments.extend(tail);
    segments
}
