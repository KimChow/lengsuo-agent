//! plan 模式段集（07 册 §3「plan 无专属模板，复用 craft + plan fragments」）。
//!
//! plan **不是另一种人格**：它复用 craft 的全部段（21 册 §3「craft/plan/expert
//! 共用同一份」），差异只由「craft 工具集 + 规划门禁片段 + plan 专属
//! task-management」承载（21 册 §5：plan 的 task-management 比 craft 版多两条）。
//! 门禁段的工具名用本仓真实名（`plan.enter` / `plan.exit`）。

use crate::model::{PromptMode, Segment};

use super::common::text;

/// plan 的段集（craft 段 + 门禁段 + plan 专属任务管理段）。
///
/// 段序（07 §2「模板 + 片段」的分层）：模式身份 → 门禁片段与规划态任务管理
/// （plan 的 `interaction` 片段）→ craft 公共层与行为段 → 硬规则。
pub(crate) fn plan_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::craft::craft_segments(mode);
    // 能力段占 header 之后一格（与 expert 人格槽的定位方式一致）。
    let at = super::common::header(mode).len() + 1;
    // 门禁与规划态任务管理：05 §5 plan 门禁状态机 + 21 §5 plan 专属 task-management。
    let extra = [Segment::Gate, text(super::common::TASK_MANAGEMENT_PLAN)];
    segments.splice(at..at, extra);
    segments
}
