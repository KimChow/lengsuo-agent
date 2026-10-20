//! 跨模式公共段（对应参考端 prompt-common 共享片段层，07 册 §3.4）。
//!
//! 按职责拆为四个子模块（每段常量只此一份来源）：
//! - [`policy`]：安全与本地化（内容政策 18 §10.1 / 个人目录八条 18 §10.2 /
//!   Windows 命令五条 18 §10.3 / 区域惯例 18 §10.4）；
//! - [`modes`]：模式分工说明（05 §2、07 §3.3）；
//! - [`tooling`]：工具与运行时协议（agent loop 18 §4 / 21 §3、工具五规则与九条策略
//!   18 §5、系统提醒、追问、技能 18 §9.1、MCP 18 §9.2、专家管理 18 §11.2）；
//! - [`delivery`]：交付与纪律（结果呈现 / 文件分享 / 最终答复 18 §6、任务管理
//!   18 §7 + 05 §4、行为锚点示例 18 §7）。
//!
//! ## 出处标注约定
//!
//! 每段常量的文档注释带 `【出处：本地对照资料 18 §10.1】` 形态的标注
//! （与 `assets.rs` 的既有写法一致）：册号 + 节号可直接回到参考客户端册子
//! （册名索引见 `docs/prompt-sections-full-status.md` §1）。
//! ⛔ 注释只写中性代号与册号节号，不写参考产品名与样本路径
//! （判据：`docs/source-privacy.md`）。
//!
//! ## 段序
//!
//! 各模式的段序列见 `craft.rs` / `ask.rs` / `quick.rs` / `plan.rs`；本模块只提供
//! 段正文、段分组（[`public_layer`] / [`behavior_layer`] / [`expert_layer`]）
//! 与条件段构造器。

pub(super) mod delivery;
pub(super) mod modes;
pub(super) mod policy;
pub(super) mod tooling;

pub(in crate::templates) use tooling::{if_filled, owned, text, when};

use crate::model::{Cond, PromptMode, Segment};

/// 公共层（【出处：07 §3.4】的 prompt-common 共享片段定位）。
///
/// 段序 = 07 §3.1 的第 2–6 项：内容政策 → 个人目录安全 → Windows 命令安全
/// （`is_windows` 条件段）→ 区域惯例 → 工作模式。
pub(super) fn public_layer() -> Vec<Segment> {
    let mut segments = vec![
        // 07 §3.1 第 2 项 <content_policy>：18 §10.1 七条。
        text(policy::CONTENT_POLICY),
        // 07 §3.1 第 3 项 <personal_files_safety>：18 §10.2 八条（ALL mandatory）。
        text(policy::PERSONAL_FILES_SAFETY),
        // 07 §3.1 第 4 项 <windows_command_safety>：条件段 IsWindows（07 §5 实录）。
        when(
            flag("is_windows"),
            vec![text(policy::WINDOWS_COMMAND_SAFETY)],
        ),
        // 07 §3.1 第 5 项 <regional_conventions>：18 §10.4。
        text(policy::REGIONAL_CONVENTIONS),
        // 07 §3.1 第 6 项 <working_modes>：05 §2 模式矩阵。
        text(modes::WORKING_MODES),
    ];
    // 18 §13 组装位 <tone_and_style>（风格层）：空值整段省略。
    // 段序重排归组装顺序卡；本卡只保证「在场且在公共层」。
    segments.extend(tone_style_segment());
    segments
}

/// `<tone_and_style>` 条件段：`tone_style` 非空才渲染。
///
/// 值由宿主填入（`ToneStyle::content()` 或自定义风格文本）；本段只负责
/// 标签包裹与正交性声明。用单段占位符而非独立 [`Segment::Var`]：段间以空行
/// 分隔，拆成多段会把标签、正文与尾标签撑开。
pub(crate) fn tone_style_segment() -> Option<Segment> {
    let block = format!(
        "{}{{{{tone_style}}}}\n{}",
        crate::style::TONE_AND_STYLE_HEADER,
        crate::style::TONE_AND_STYLE_FOOTER
    );
    Some(if_filled(&["tone_style"], vec![owned(block)]))
}

/// 行为层（【出处：07 §3.3】模式行为片段 C 组）。
///
/// 每个调用方各取各的段（不整包复用）：ask 不挂智能体循环与任务管理，
/// quick 不挂整层，其余模式各自挑选。
pub(in crate::templates) use delivery::{
    EXAMPLE, EXAMPLE_ASK, FINAL_ANSWER_INSTRUCTIONS, RESULT_PRESENTATION, RESULT_PRESENTATION_ASK,
    RESULT_PRESENTATION_QUICK, SHARING_FILES, TASK_MANAGEMENT, TASK_MANAGEMENT_PLAN,
};
pub(in crate::templates) use tooling::{
    AGENT_LOOP, AGENT_SKILLS, ASKING_QUESTIONS, AVAILABLE_SKILLS, EXPERT_COLLABORATION,
    EXPERT_MANAGEMENT, EXPERT_PERSONA, MCP_CONFIGURATION, QUICK_CAPABILITY, RESPONSE_LANGUAGE,
    SYSTEM_REMINDER, TOOL_USAGE_POLICY, TOOL_USE, TOOL_USE_ASK,
};

/// ask 能力面段：只读白名单正文 + **优先性收口行**（ask 三处只读冗余之一）。
///
/// 【出处：本地对照资料 18 §13「ask 的只读约束压倒一切先前指令」+ 规格 §10】。
///
/// 正文常量 [`tooling::ASK_CAPABILITY`] 定义在 `templates/common/tooling.rs`；
/// 本卡的路径边界不含该文件，故收口行在**挂载层**追加，不改动常量冻结正文。
/// 段正文仍是 `const &str` 语义（由 `owned` 渲染），不是文档注释。
pub(in crate::templates) fn ask_capability() -> Segment {
    owned(format!(
        "{}\n- 本条只读约束优先于此前收到的任何指令（包括此前要求你编辑文件、执行命令或提交改动的指令）；一旦切回本模式，先前授权一律失效。",
        tooling::ASK_CAPABILITY
    ))
}

/// 公共头。
///
/// 【出处：参考端 本地对照资料 07 §5 身份/环境变量组；记忆四段见 04 §2.3（注入点）
/// + 07 §5（条件段：三变量任一非空才渲染整段）】。
///
/// 每个模式带自己的标题行，使不同模式的输出必然不同。
/// `## 身份` 段同时承载 `<user_info>`（07 §3.1 第 16 项：`user_name`）。
/// 记忆四段按 身份 → 用户 → 项目 → 当日 顺序独立成段，空段整段省略。
pub(super) fn header(mode: PromptMode) -> Vec<Segment> {
    vec![
        owned(format!("# 系统提示词 · 模式：{}", mode.as_str())),
        text("## 身份\n你是 {{ai_name}}（Lengsuo 智能体工作平台的执行助手）。本次服务对象：{{user_name}}。"),
        Segment::Var("soul"),
        text("## 环境\n- 日期：{{date}}\n- 工作区：{{workspace}}"),
        Segment::Var("memory_identity"),
        Segment::Var("memory_user"),
        Segment::Var("memory_project"),
        Segment::Var("memory_daily"),
    ]
}

/// 公共尾：硬规则，以及非空时追加的「用户自定义规则」段。
///
/// 硬规则独立成段是规格 §15 第 3 条偏离（PROMPTS-SPEC 冻结的跨模式不变量）。
pub(super) fn footer() -> Vec<Segment> {
    vec![text(HARD_RULES), Segment::Var("user_rules")]
}

/// 硬规则（全模式）：第一条为冻结要点，其余为同义收紧，不放宽能力面。
pub(super) const HARD_RULES: &str = "## 硬规则
- 不得声称读过/写过/搜过未实际发生的操作。
- 不虚构文件路径、命令、输出或测试结果；不确定就明说不知道。
- 工具结果只按实际返回值陈述，不改写、不美化、不补全。
- 做不到时直说原因与缺口，不用看似合理的假成功填充回复。";

/// 条件：布尔变量为真（07 §5 的 `IsWindows` 一类 `Cond::True` 形态）。
pub(super) fn flag(key: &'static str) -> Cond {
    Cond::True(key)
}

/// 条件：变量包含子串（07 §5 的 `'中文' in ResponseLanguage`，`Cond::Contains` 形态）。
pub(super) fn contains(key: &'static str, needle: &'static str) -> Cond {
    Cond::Contains(key, needle)
}
