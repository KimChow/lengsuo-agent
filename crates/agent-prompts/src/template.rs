//! 模板门面：按模式取段集。
//!
//! 段正文全部下沉到 `templates/`（按模式分文件，对应参考端
//! `interactionmode/*/fragments/`），本文件只做**门面与分派**：
//! 模式 → 段序列，不含任何段正文。
//!
//! 模板以段序列表达，便于「craft 段完整复用」这类断言：plan = craft 段 + 门禁段，
//! expert = craft 段 + 专家人格/协作段。文本里的 `{{key}}` 由 `PromptVars` 注入。

use crate::model::{PromptMode, PromptTemplate, Segment};
use crate::templates::{ask, craft, expert, plan, quick};

/// plan 门禁片段（EnterPlanMode/ExitPlanMode 语义：批准前禁止写操作承诺）。
pub const PLAN_GATE: &str = "## 规划门禁（plan）
- 当前处于规划状态：先给出计划（目标、步骤、影响文件、验证方式、风险），等待用户批准。
- 批准之前不得承诺或暗示任何写操作已经完成，也不得发起写入、删除、命令执行、外部网络或浏览器动作。
- 计划被批准后才退出规划状态，转为实际执行。";

/// 取某模式的模板。全部为静态构造，不读配置、不做 IO。
pub fn template_for(mode: PromptMode) -> PromptTemplate {
    let segments: Vec<Segment> = match mode {
        PromptMode::Quick => quick::quick_segments(mode),
        PromptMode::Ask => ask::ask_segments(mode),
        // plan = craft 模板 + 门禁段（门禁排在能力段之后、硬规则之前）。
        PromptMode::Plan => plan::plan_segments(mode),
        PromptMode::Craft => craft::craft_segments(mode),
        // expert = craft 模板 + 专家人格段 + 专家协作段。
        PromptMode::Expert => expert::expert_segments(mode),
    };
    PromptTemplate::new(mode, segments)
}
