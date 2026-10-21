//! 模板门面：按模式取段集；变体层在此追加「本轮关注什么」一段。
//!
//! 段正文全部下沉到 `templates/`（按模式分文件，对应参考端
//! `interactionmode/*/fragments/`），本文件只做**门面与分派**：
//! 模式 → 段序列，不含任何既有段正文。
//!
//! 模板以段序列表达，便于「craft 段完整复用」这类断言：plan = craft 段 + 门禁段，
//! expert = craft 段 + 专家人格/协作段。文本里的 `{{key}}` 由 `PromptVars` 注入。
//!
//! ## 变体层（v1）
//!
//! 模式 × 变体 = 模板矩阵（【出处：本地对照资料 07 §3「模式 × 变体选择规则
//! R07-1」+ §8 模板缺失回落表】）。本仓 v1 的变体只做**一段追加**：
//! 在模式身份区（能力面段之后）插入一段「变体侧重」，不改能力面、公共层与
//! 硬规则；回落表见 [`template_for_variant`]。
//!
//! 变体正文（[`VARIANT_CODE`] / [`VARIANT_CODING`] / [`VARIANT_DESIGN`]）由本仓
//! 自撰（参考端的变体模板正文未收编），每段 ≤12 行，只讲「本变体关注什么」，
//! **不写工具名、不写权限叙述、不写模式名**（与模式、风格层正交）。

use crate::model::{PromptMode, PromptTemplate, PromptVariant, Segment};
use crate::templates::{ask, craft, expert, plan, quick};

/// plan 门禁片段（EnterPlanMode/ExitPlanMode 语义：批准前禁止写操作承诺）。
pub const PLAN_GATE: &str = "## 规划门禁（plan）
- 当前处于规划状态：先给出计划（目标、步骤、影响文件、验证方式、风险），等待用户批准。
- 批准之前不得承诺或暗示任何写操作已经完成，也不得发起写入、删除、命令执行、外部网络或浏览器动作。
- 计划被批准后才退出规划状态，转为实际执行。";

/// 变体段正文 · 代码类：实现细节、边界与错误处理优先。
///
/// 只讲关注点，不叙述能力面与授权语义（正交性判据见 `tests/variant.rs`）。
pub(crate) const VARIANT_CODE: &str = "## 变体侧重（代码类）
- 关注实现本身：数据结构、控制流与接口边界怎么写。
- 关注边界条件：空值、越界、并发与异常路径是否被显式处理。
- 关注失败语义：错误在哪里被捕获、如何暴露、调用方能否据此恢复。
- 关注改动面：新逻辑与既有逻辑的耦合点是否足够小，替换是否可回退。";

/// 变体段正文 · 编码类：从零到一的完整实现优先。
pub(crate) const VARIANT_CODING: &str = "## 变体侧重（编码类）
- 关注完整实现：从零到一每一步都落地，不停在方案描述上。
- 关注步骤顺序：先搭骨架再填细节；顺序有依赖时先讲清依赖关系。
- 关注完成状态：半成品与占位实现要显式标出，不当作完成交付。
- 关注可继续加工：产出能接着往下写，而不是只作说明的片段。";

/// 变体段正文 · 设计类：结构、层次与可读性优先。
pub(crate) const VARIANT_DESIGN: &str = "## 变体侧重（设计类）
- 关注结构：模块怎么分、层次怎么排、职责边界画在哪里。
- 关注可读性：命名、注释与抽象层次是否让后来者能顺着读下去。
- 关注可演进：改动一处时牵连面有多大，有没有预留替换余地。
- 关注取舍：为什么选这个结构、代价是什么、与备选方案的差别在哪。";

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

/// 取「模式 × 变体」的模板；变体只追加一段「变体侧重」。
///
/// ## 回落表（【出处：本地对照资料 07 §8 模板缺失回落表】）
///
/// | 模式 | Default | Code | Coding | Design |
/// |---|---|---|---|---|
/// | Craft | craft 段集 | craft + 代码类 | craft + 编码类 | craft + 设计类 |
/// | Plan | plan 段集 | plan + 代码类 | 回落 Code | 回落 Default |
/// | Expert | expert 段集 | expert + 代码类 | 回落 Code | 回落 Default |
/// | Ask / Quick | 各自段集 | 回落 Default | 回落 Default | 回落 Default |
///
/// **回落在返回值上可观测**：回落后的模板与回落目标的模板**段序列完全相等**
/// （不是「多一段说明」），且 [`PromptTemplate::variant`] 返回**实际生效**的变体。
/// 未知变体名在 [`PromptVariant::from_name`] 处即已归一为 `Default`。
pub fn template_for_variant(mode: PromptMode, variant: PromptVariant) -> PromptTemplate {
    let effective = effective_variant(mode, variant);
    let base = template_for(mode);
    if effective == PromptVariant::Default {
        // 回落 Default ⇒ 与 `template_for(mode)` 逐段相同（不做任何追加）。
        return base;
    }
    let Some(body) = variant_body(effective) else {
        return base;
    };
    let mut segments = base.segments().to_vec();
    let at = variant_insert_at(&segments);
    segments.insert(at, Segment::Text(body.to_string()));
    PromptTemplate::new_with_variant(mode, effective, segments)
}

/// 模式 × 变体 → 实际生效的变体（回落表的可执行形态）。
fn effective_variant(mode: PromptMode, variant: PromptVariant) -> PromptVariant {
    match (mode, variant) {
        (_, PromptVariant::Default) => PromptVariant::Default,
        // craft 三个变体各有一段独立正文，互不回落。
        (PromptMode::Craft, other) => other,
        // plan / expert 复用 craft 模板族：编码类回落代码类，设计类回落默认。
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Code) => PromptVariant::Code,
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Coding) => PromptVariant::Code,
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Design) => PromptVariant::Default,
        // ask / quick 无变体模板族：一律回落默认。
        (PromptMode::Ask | PromptMode::Quick, _) => PromptVariant::Default,
    }
}

/// 变体对应的段正文（`Default` 无正文 ⇒ `None`）。
fn variant_body(variant: PromptVariant) -> Option<&'static str> {
    match variant {
        PromptVariant::Code => Some(VARIANT_CODE),
        PromptVariant::Coding => Some(VARIANT_CODING),
        PromptVariant::Design => Some(VARIANT_DESIGN),
        PromptVariant::Default => None,
    }
}

/// 变体段的插入位：紧跟「能力面」段之后（模式身份区的末尾）。
///
/// 找能力面段而不是写死下标：header 长度会随记忆四段增减，硬写下标会在
/// header 变更时错位。找不到能力面段时退化为追加到末尾（保底不丢内容）。
fn variant_insert_at(segments: &[Segment]) -> usize {
    segments
        .iter()
        .position(|segment| {
            matches!(segment, Segment::Text(body)
                if body.starts_with("## 工具能力") || body.starts_with("## 能力边界"))
        })
        .map_or(segments.len(), |index| index + 1)
}
