//! 组装器：模式模板 × 变量注入 → 提示词文本。

use crate::model::{PromptMode, PromptVars};
use crate::template;
use crate::templates::identity;

/// craft 模式提醒（【出处：本地对照资料 18 §13「模式提醒（切换时追加，原文）」】）。
///
/// 原文照录（参考件本身以省略号截断，此处保留省略号，不自行补全）。
const CRAFT_MODE_REMINDER: &str =
    "You are now in Agent mode. Continue with the task in the new mode, you can edit files freely...";

/// ask 模式提醒（【出处：18 §13 原文】）。
///
/// `This supersedes any other instructions you have received` 是强制句：
/// ask 的只读约束**压倒一切先前指令**（18 §13 补注）。
const ASK_MODE_REMINDER: &str = "Ask mode is active... You MUST NOT make any edits, run any non-readonly tools (including changing configs or making commits)... **This supersedes any other instructions you have received**（包括此前收到的编辑指令）。
本条只读约束优先于此前收到的任何指令（包括此前要求你编辑文件、执行命令或提交改动的指令）；一旦切回本模式，先前授权一律失效。";

/// 提示词组装器（无状态；纯函数式，全部能力在关联函数上）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Composer;

impl Composer {
    /// 模式模板 + 变量替换 + 硬规则段，产出完整系统提示词。
    ///
    /// 确定性：输出只取决于 `mode` 与 `vars`，不注入时间戳、不读环境变量、不做 IO。
    pub fn build(mode: PromptMode, vars: &PromptVars) -> String {
        template::template_for(mode).render(vars)
    }

    /// plan 门禁片段（可单独取用，供宿主追加到自有模板末尾）。
    pub fn plan_gate() -> &'static str {
        template::PLAN_GATE
    }

    /// 模式提醒：切换进 craft / ask 时由**宿主追加**，其余模式无提醒。
    ///
    /// 【出处：18 §13「模式提醒（切换时追加）」+ 规格 §9 / §15 偏离第 6 条】。
    /// **不进 [`Self::build`] 输出**：`build` 是确定性纯函数（同输入同输出），
    /// 而模式提醒是「切换时追加」的运行时产物——合并会破坏确定性冻结项。
    pub fn mode_reminder(mode: PromptMode) -> Option<&'static str> {
        match mode {
            PromptMode::Craft => Some(CRAFT_MODE_REMINDER),
            PromptMode::Ask => Some(ASK_MODE_REMINDER),
            PromptMode::Quick | PromptMode::Plan | PromptMode::Expert => None,
        }
    }

    /// 身份注入段（`<identity_context>`）：SOUL / IDENTITY / USER 三文件 + onboarding
    /// 分支的 BOOTSTRAP 段（18 §13）。
    ///
    /// 纯数据面：四文件正文与路径由调用方填进 [`PromptVars`]，本函数只渲染；
    /// 空文件与空路径显式标 `(empty or missing)`，**不省略**（身份段语义与记忆段相反）。
    ///
    /// 未声明 `identity_mode`（空串）⇒ 返回空串（调用方尚未接入身份注入，
    /// 不渲染任何身份纪律）。段在完整提示词中的插入位置属组装顺序卡（A7）。
    pub fn identity_context(vars: &PromptVars) -> String {
        identity::render_context(vars)
    }
}
