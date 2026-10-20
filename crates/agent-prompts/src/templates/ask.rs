//! ask 模式段集（07 册 §3.3 ask 六片段 + 21 册 §1「ask | agent-loop(空)」）。
//!
//! ask **不挂 agent-loop**（21 册 §1「ask | agent-loop(空)」，只读问答没有执行回合），
//! 挂 mode-behavior（只读白名单能力面）与只读版结果呈现。ask 的压倒性语义（A4）
//! 与三处只读冗余**不在本卡**做。
//!
//! ask 与 craft 同为「行为层」模式（07 册 §3.3 片段清单），行为段**按段**取用
//! craft 的同一份常量，只做两处替换（逐条见 [`ask_behavior_sections`]）；
//! **不挂** task_management（ask 不建任务），result_presentation 走 ask 版
//! （ask 只读，不产出新文件）。

use crate::model::{PromptMode, Segment};

use super::common::text;

/// ask 的段集（公共头 + 只读能力面 + 公共层 + 行为段 + 公共尾）。
pub(crate) fn ask_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    segments.push(text(super::common::ASK_CAPABILITY));
    segments.extend(super::common::public_layer());
    segments.extend(ask_behavior_sections());
    segments.extend(super::common::footer());
    segments
}

/// ask 的**行为层**：与 craft 同序，只去掉执行回合独有的一段。
///
/// 唯二差异（07 §3.3 ask 片段清单）：
/// - 不挂 `agent_loop`（21 §1「ask | agent-loop(空)」：只读问答没有执行回合）；
/// - `task_management` 跟踪的是执行进度，ask 只读不建任务 ⇒ 不挂；
///   `result_presentation` 换 ask 只读版。`sharing_files` 仍在（第 9 项必做，
///   ask 同样可能交付读到的已有文件）。
///
/// 其余行为段（工具调用 / 策略九条 / 追问 / 系统提醒 / 技能 / MCP / 回复语言）
/// 与 craft 共用同一份常量（调用方的「同一份段正文」断言据此成立）。
fn ask_behavior_sections() -> Vec<Segment> {
    let mut segments: Vec<Segment> = super::craft::craft_behavior_sections()
        .into_iter()
        .filter_map(|segment| match segment {
            // 只在**段正文**这一层做替换：条件段（`If`）原样保留，其内部字段
            // 不经本函数改写（条件段里没有 agent_loop / 结果呈现，与 craft 共用）。
            Segment::Text(body) if super::craft::craft_absent(&body) => None,
            Segment::Text(body) if body == super::common::RESULT_PRESENTATION => {
                Some(text(super::common::RESULT_PRESENTATION_ASK))
            }
            Segment::Text(body) if body == super::common::EXAMPLE => {
                Some(text(super::common::EXAMPLE_ASK))
            }
            Segment::Text(body) if body == super::common::TOOL_USE => {
                Some(text(super::common::TOOL_USE_ASK))
            }
            Segment::If(cond, inner) => Some(Segment::If(super::craft::ask_condition(cond), inner)),
            other => Some(other),
        })
        .collect();
    segments.push(text(super::common::ASK_CAPABILITY));
    segments
}
