//! craft 模式段集（07 册 §3.1 的 30 section 骨架中 craft 承载的部分）。
//!
//! 段序逐条对齐 07 册 §3.1 的 section 顺序（编号即原文顺序）；plan 与 expert
//! 复用本文件的段（21 册 §3「craft/plan/expert 共用同一份」），差异由各自文件追加。
//!
//! 内容来源：本地对照资料 18 册（§4 agent loop、§5 工具规则、§6 交付协议、
//! §7 任务管理、§9 技能/MCP、§10 安全规则、§11.2 专家）、05 册（§2 模式矩阵、
//! §3 loop、§4 任务启用条件）、07 册（§3.1 section 顺序、§3.3 行为片段、§5 条件段）。
//! 每段正文在 `common/` 下按职责分包，出处标注写在常量的文档注释里。
//!
//! **段正文一律在 `common/`**（跨模式复用时只此一份来源）；本文件只决定
//! **哪些段、按什么顺序**出现在 craft 模式里。

use crate::model::{Cond, PromptMode, Segment, WORK_MODE_KEY};

use super::common::{contains, flag, text, tooling::CRAFT_CAPABILITY, when};

/// craft 的完整段集（公共头 + 能力面 + 公共层 + 行为层 + 公共尾）。
///
/// `plan` 与 `expert` 以本函数为基座追加自己的差异段（21 册 §1 片段复用语义）。
pub(crate) fn craft_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    // 07 §3.1 的 section 骨架不含能力面段；能力面是各模式自己的「模式身份」段
    // （05 §2「模式 = 工具白名单 + 提示词模板 + 硬规则」三层同切）。
    segments.push(text(CRAFT_CAPABILITY));
    segments.extend(super::common::public_layer());
    segments.extend(craft_behavior_sections());
    segments.extend(super::common::footer());
    segments
}

/// craft 行为面段，按 07 §3.1 的 section 顺序排列。
///
/// 规格 §2 表中「不做」的 9 段（1 用户输入槽位 / 11 automations / 13 file_id /
/// 14 doc_url / 15 与 17 可视化器 / 21 提交钩子 / 24 agent_mail / 30 二进制上下文）
/// **不在本序列出现，也不留占位段**；理由与重新考虑条件见
/// `docs/prompt-sections-full-status.md` §4。
///
/// 公共层（第 2–6 项）由 [`super::common::public_layer`] 提供；本函数只排
/// 行为层（第 7–29 项），顺序即 07 §3.1 的编号顺序。
pub(super) fn craft_behavior_sections() -> Vec<Segment> {
    vec![
        // 07 §3.1 第 7 项 <agent_loop>：18 §4 九步去掉第 8 步 File transfer，
        // 以 21 §3 复刻建议的 fragment 8 步为基座。
        text(super::common::AGENT_LOOP),
        // 07 §3.1 第 8 项 <result_presentation>：18 §6 六条。
        text(super::common::RESULT_PRESENTATION),
        // 07 §3.1 第 9 项 <sharing_files>：18 §6 风格规则。
        text(super::common::SHARING_FILES),
        // 07 §3.1 第 10 项 <final_answer_instructions>：18 §6 独立成立 + ≤50–70 行。
        text(super::common::FINAL_ANSWER_INSTRUCTIONS),
        // 07 §3.1 第 12 项 <tool_use>：18 §5 五规则。
        text(super::common::TOOL_USE),
        // 07 §3.1 第 18 项 <task_management>：18 §7 + 05 §4 R05-3 启用条件。
        text(super::common::TASK_MANAGEMENT),
        // 07 §3.1 第 19 项 <example>：18 §7「示例即行为锚点」两个完整示例。
        text(super::common::EXAMPLE),
        // 07 §3.1 第 20 项 <asking_questions>：18 §5 绝不猜参数 + 05 §4 提问纪律。
        text(super::common::ASKING_QUESTIONS),
        // 07 §3.1 第 22 项 <tool_usage_policy>：18 §5 九条。
        text(super::common::TOOL_USAGE_POLICY),
        // 07 §3.1 第 23 项 <system-reminder>：18 §5 容器机制说明（九条的邻居段）。
        text(super::common::SYSTEM_REMINDER),
        // 07 §3.1 第 25 项 <agent_skills>：18 §9.1 技能加载纪律。
        text(super::common::AGENT_SKILLS),
        // 07 §3.1 第 26 项 <available_skills>：技能清单注入点（18 §9.1）。
        text(super::common::AVAILABLE_SKILLS),
        // 07 §3.1 第 27 项 <expert_management>：18 §11.2；
        // 条件段 ExpertManagementEnabled（07 §5）：开关为真才渲染该段。
        // 开关是「专家管理功能是否启用」的产品开关（不是「当前处于专家模式」：
        // 05 §2 的模式切换靠模板族本身，craft/plan 也挂本段）。
        // 人格注入槽（21 §5）由 expert.rs 追加，不在 craft 序列内。
        when(
            flag("feature_expert_management"),
            vec![text(super::common::EXPERT_MANAGEMENT)],
        ),
        // 07 §3.1 第 28 项 <mcp_configuration>：18 §9.2 MCP 配置协议。
        text(super::common::MCP_CONFIGURATION),
        // 07 §3.1 第 29 项 <response_language>：条件段 '中文' in ResponseLanguage（07 §5）。
        when(
            contains("response_language", "中文"),
            vec![text(super::common::RESPONSE_LANGUAGE)],
        ),
    ]
}

/// ask 从 craft 行为层拿掉的一段（按段正文判定，不靠下标）。
///
/// `agent_loop`：21 §1「ask | agent-loop(空)」——只读问答没有执行回合；
/// `task_management` 跟踪的正是执行回合的进度，同样不挂。
pub(super) fn craft_absent(body: &str) -> bool {
    body == super::common::AGENT_LOOP || body == super::common::TASK_MANAGEMENT
}

/// ask 版本的条件段改写：把 `<expert_management>` 收紧为「模式 ∈ 可写模式」。
///
/// 只有这一条需要：它是**写类**段（创建 / 修改专家），ask 只读 ⇒ 整段不挂。
/// 用既有 `Cond::Any` + `Cond::Eq(WORK_MODE_KEY, …)` 组合，不新造条件机制。
/// `Cond` 没有「与」变体；ask 恒为只读模式，收紧模式面后该段对 ask 永不成立，
/// 开关是否打开不影响结论，故这里不重复判开关。
pub(super) fn ask_condition(cond: Cond) -> Cond {
    if cond != super::common::flag("feature_expert_management") {
        return cond;
    }
    Cond::Any(&[
        Cond::Eq(WORK_MODE_KEY, "plan"),
        Cond::Eq(WORK_MODE_KEY, "craft"),
        Cond::Eq(WORK_MODE_KEY, "expert"),
    ])
}
