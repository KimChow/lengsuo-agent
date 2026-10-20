//! ask 模式段集（07 册 §3.3 ask 六片段 + 21 册 §5 ask 双层只读）。
//!
//! ask **不挂 agent-loop**（21 册 §1「ask | agent-loop(空)」），挂 mode-behavior
//! 与只读白名单能力面。ask 的压倒性语义（A4）与三处只读冗余**不在本卡**做。
//!
//! ask 与 craft 共用 tool-use / tool_usage_policy / final_answer_instructions /
//! system-reminder / asking_questions 五件行为片段（07 册 §3.3 片段清单），
//! 但**不挂** task_management（ask 不建任务），result_presentation 走 ask 版
//! （ask 只读，不产出新文件）。

use crate::model::{PromptMode, Segment};

use super::common::{contains, text, when};

/// ask 的段集（公共头 + 只读能力面 + 公共段 + 行为段 + 公共尾）。
pub(crate) fn ask_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    segments.push(text(ASK_CAPABILITY));
    segments.extend(super::common::common_sections());
    segments.extend(ask_behavior_sections());
    segments.extend(super::common::footer());
    segments
}

/// ask 自己的行为片段（ask 版结果呈现 + 与 craft 共用的五件）。
fn ask_behavior_sections() -> Vec<Segment> {
    vec![
        // 07 §3.1 第 8 项 <result_presentation>（ask 版）。
        text(super::common::RESULT_PRESENTATION_ASK),
        // 07 §3.1 第 10 项 <final_answer_instructions>：ask 与 craft 共用。
        text(super::common::FINAL_ANSWER_INSTRUCTIONS),
        // 07 §3.1 第 12 项 <tool_use>：ask 与 craft 共用同一份 tool-use 片段。
        text(super::common::TOOL_USE),
        // 07 §3.1 第 20 项 <asking_questions>：ask 与 craft 共用。
        text(super::common::ASKING_QUESTIONS),
        // 07 §3.1 第 22 项 <tool_usage_policy>：ask 与 craft 共用。
        text(super::common::TOOL_USAGE_POLICY),
        // 07 §3.1 第 23 项 <system-reminder>：ask 与 craft 共用。
        text(super::common::SYSTEM_REMINDER),
        // 07 §3.1 第 29 项 <response_language>：条件段（'中文' in ResponseLanguage）。
        when(
            contains("response_language", "中文"),
            vec![text(super::common::RESPONSE_LANGUAGE)],
        ),
    ]
}

/// ask 能力面：只读白名单。
const ASK_CAPABILITY: &str = "## 工具能力（ask · 只读白名单）
- 允许的工具仅限只读类别：fs.list、fs.read、fs.search。
- 只读白名单之外的动作一律不发起：不写入、不删除、不执行命令、不访问外部网络、不操作浏览器。
- 需要改动时先说明缺口与所需授权，请用户切换到 plan 或 craft。";
