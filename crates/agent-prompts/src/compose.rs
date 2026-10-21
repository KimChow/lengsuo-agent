//! 组装器：模式模板 × 变量注入 → 提示词文本。
//!
//! 渲染管线（段序列 → 文本）在本文件：域模型（[`crate::model`]）只描述
//! 「段是什么」，「怎么拼成文本」属组装职责。

use crate::model::{PromptMode, PromptTemplate, PromptVariant, PromptVars, Segment};
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
    ///
    /// **兼容入口**：等价于 [`Self::build_with`] 传 [`PromptVariant::Default`]——
    /// 既有调用方与既有快照**零改动**（变体层是叠加维度，不改默认输出）。
    pub fn build(mode: PromptMode, vars: &PromptVars) -> String {
        Self::build_with(mode, PromptVariant::Default, vars)
    }

    /// 模式 × 变体模板 + 变量替换 + 硬规则段，产出完整系统提示词。
    ///
    /// 变体只追加一段「变体侧重」（不碰能力面、公共层与硬规则）；
    /// 变体不被该模式支持时按回落表回落到默认变体（回落在输出上可观测）。
    ///
    /// 确定性：输出只取决于 `mode`、`variant` 与 `vars`，不做 IO、不取时钟。
    pub fn build_with(mode: PromptMode, variant: PromptVariant, vars: &PromptVars) -> String {
        template::template_for_variant(mode, variant).render(vars)
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

/// 渲染一个模板：段间以空行分隔，空槽位整段省略，末尾保留单个换行。
///
/// 条件段（[`Segment::If`]）：条件成立才渲染其内层段；不成立则整段省略，
/// 不留空洞标题、不留多余空行。
pub(crate) fn render_template(tpl: &PromptTemplate, vars: &PromptVars) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(tpl.segments().len());
    render_segments(tpl.segments(), vars, tpl.mode(), &mut parts);
    let mut out = parts.join("\n\n");
    out.push('\n');
    out
}

/// 渲染一段段序列（含条件段的内层递归），把非空段追加到 `parts`。
fn render_segments(
    segments: &[Segment],
    vars: &PromptVars,
    mode: PromptMode,
    parts: &mut Vec<String>,
) {
    for segment in segments {
        match segment {
            Segment::Text(text) => push_part(parts, Some(substitute(text, vars))),
            Segment::Var(key) => push_part(parts, slot(key, vars)),
            Segment::Gate => push_part(parts, trim(template::PLAN_GATE)),
            // 条件不成立 ⇒ 整段省略（与空值省略语义一致）。
            Segment::If(cond, inner) => {
                if cond.holds(vars, mode) {
                    render_segments(inner, vars, mode, parts);
                }
            }
        }
    }
}

/// 追加一段：空段（纯空白）不进列表，避免留下空洞标题。
fn push_part(parts: &mut Vec<String>, part: Option<String>) {
    if let Some(part) = part {
        if !part.trim().is_empty() {
            parts.push(part);
        }
    }
}

/// 单遍替换 `{{key}}`：未知键与值中的花括号原样保留，不递归展开。
fn substitute(text: &str, vars: &PromptVars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => out.push_str(value),
                    // 未知键：原样保留花括号（内置模板不含未知键，注入值不再被替换）。
                    None => {
                        out.push_str("{{");
                        out.push_str(key);
                        out.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 槽位渲染：值非空（且非纯空白）时成段，否则整段省略，不留空洞标记。
/// 六段可独立成节（人格设定 / 记忆四段 / 用户自定义规则）；
/// 其余已知变量作为独立段时输出裸值；未知键省略。
fn slot(key: &'static str, vars: &PromptVars) -> Option<String> {
    let value = vars.get(key).and_then(trim)?;
    match key {
        "soul" => Some(format!("人格设定：\n{value}")),
        "memory_identity" => Some(format!("## 身份\n{value}")),
        "memory_user" => Some(format!("## 用户记忆\n{value}")),
        "memory_project" => Some(format!("## 项目记忆\n{value}")),
        "memory_daily" => Some(format!("## 当日日志\n{value}")),
        "user_rules" => Some(format!("## 用户自定义规则\n{value}")),
        _ => Some(value),
    }
}

/// 去掉首尾空白；全空白视为空（返回 `None`）。
fn trim(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
