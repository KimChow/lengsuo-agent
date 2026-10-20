//! craft 模式段集（07 册 §3.1「28 section 骨架」中 craft 必做的 17 段为主）。
//!
//! 段序对齐 07 册 §3.1 的 craft 模板 section 顺序；plan 与 expert 复用本文件的
//! 段（21 册 §3「craft/plan/expert 共用同一份」），差异由各自文件追加。
//!
//! 内容来源：参考端 pc-devguide 18 册（§4 agent loop、§5 工具规则、§6 交付协议、
//! §7 任务管理、§10 安全规则、§11.2 专家）、05 册（§3 loop、§4 任务启用条件）、
//! 07 册（§3.3 模式行为片段、§5 条件段实录）、21 册（§5 ask 双层只读）。
//! 每段旁标注册号节号，便于逐字回查。
//!
//! **段正文一律在 `common.rs`**（跨模式复用时只此一份来源）；本文件只决定
//! **哪些段、按什么顺序**出现在 craft 模式里。

use crate::model::{PromptMode, Segment};

use super::common::{contains, flag, text, when};

/// craft 的完整段集（公共头 + 能力面 + craft 专属段 + 公共尾）。
///
/// `plan` 与 `expert` 以本函数为基座追加自己的差异段（21 册 §1 片段复用语义）。
pub(crate) fn craft_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    segments.push(text(CRAFT_CAPABILITY));
    segments.extend(super::common::common_sections());
    segments.extend(craft_behavior_sections());
    segments.extend(super::common::footer());
    segments
}

/// craft 行为面段（§2 骨架里除公共头/尾之外的部分）。
///
/// 段序按 07 册 §3.1 的 craft 模板 section 顺序排列；条件段用 `when(...)` 包裹。
fn craft_behavior_sections() -> Vec<Segment> {
    vec![
        // 07 §3.1 第 7 项 <agent_loop>：18 §4 的 9 步，按 21 §3 复刻建议取
        // fragment 8 步为基座（第 8 步 File transfer 是 craft-code 变体特化）。
        text(super::common::AGENT_LOOP),
        // 07 §3.1 第 8 项 <result_presentation>：18 §6 六条。
        text(super::common::RESULT_PRESENTATION),
        // 07 §3.1 第 9 项 <sharing_files>：18 §6 风格规则。
        text(super::common::SHARING_FILES),
        // 07 §3.1 第 10 项 <final_answer_instructions>：18 §6 独立成立 + 行数上限。
        text(super::common::FINAL_ANSWER_INSTRUCTIONS),
        // 07 §3.1 第 12 项 <tool_use>：18 §5 五规则。
        text(super::common::TOOL_USE),
        // 07 §3.1 第 18 项 <task_management>：18 §7 + 05 §4 R05-3 启用条件。
        text(super::common::TASK_MANAGEMENT),
        // 07 §3.1 第 20 项 <asking_questions>：18 §5 绝不猜参数 + 05 §4 提问纪律。
        text(super::common::ASKING_QUESTIONS),
        // 07 §3.1 第 19 项 <example>：18 §7「示例即行为锚点」。
        text(super::common::EXAMPLE),
        // 07 §3.1 第 22 项 <tool_usage_policy>：18 §5 九条。
        text(super::common::TOOL_USAGE_POLICY),
        // 07 §3.1 第 23 项 <system-reminder>：18 §5 容器机制说明。
        text(super::common::SYSTEM_REMINDER),
        // 07 §3.1 第 25 项 <agent_skills>：18 §9.1 技能加载纪律。
        text(super::common::AGENT_SKILLS),
        // 07 §3.1 第 26 项 <available_skills>：技能清单注入点。
        text(super::common::AVAILABLE_SKILLS),
        // 07 §3.1 第 27 项 <expert_management>：条件段（ExpertManagementEnabled）。
        when(
            flag("feature_expert_management"),
            vec![text(super::common::EXPERT_MANAGEMENT)],
        ),
        // 07 §3.1 第 28 项 <mcp_configuration>：18 §9.2 MCP 配置协议。
        text(super::common::MCP_CONFIGURATION),
        // 07 §3.1 第 29 项 <response_language>：条件段（'中文' in ResponseLanguage）。
        when(
            contains("response_language", "中文"),
            vec![text(super::common::RESPONSE_LANGUAGE)],
        ),
    ]
}

/// craft 能力面（全工具）：本仓真实工具面（`agent-core` 的 `all_tools()`）。
const CRAFT_CAPABILITY: &str = "## 工具能力（craft · 全工具）
- 可用工具覆盖读、写、执行、网络与浏览器五类：fs.list、fs.read、fs.write、fs.delete、shell.exec、net.fetch、browser.open。
- 每个工具调用都必须真实发起，由运行时权限门裁决；被拒绝或待确认时不得假装已执行。
- 涉及写入、删除、命令执行、外部网络与浏览器操作的敏感动作需要用户确认。
- 改动工作区文件前先读取现状，收尾时列出实际产生的变更。";
