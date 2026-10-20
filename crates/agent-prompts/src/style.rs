//! 风格层：七种语言风格与 `<tone_and_style>` 包裹。
//!
//! 风格与模式**正交**：风格只影响「怎么说」，不影响「说什么」；准确性的优先级
//! 高于风格。模式模板与变量注入见 [`crate::model`] / [`crate::compose`]。
//!
//! 数据流（本地对照资料 07 §4 与 18 §13 的组装位）：
//! - 宿主取 [`ToneStyle::content`] 填入 `PromptVars.tone_style`；
//! - [`crate::Composer::build`] 在公共层末尾以 `<tone_and_style>` 包裹成段，
//!   空值整段省略（见 `templates/common/mod.rs::tone_style_segment`）；
//! - 需要独立成块的调用方可直接用 [`render`]（含枚举名归一化与自由文本分支）。
//!
//! 七段正文由本仓自撰（参考件的 `style/*.md` 原文未收编，见
//! `docs/style-layer-status.md` §5），每段 ≤200 字符。

/// 七种语言风格；与模式（quick/ask/plan/craft/expert）正交。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToneStyle {
    /// 直白：结论先行。
    Straightforward,
    /// 友好：温和、有耐心。
    Friendly,
    /// 专业：准确、结构化。
    Professional,
    /// 高效：要点列表、直入主题。
    Efficient,
    /// 轻微调侃：只针对事情本身，不冒犯用户。
    Sarcastic,
    /// 启发式：以提问推进。
    Socratic,
    /// 创意：有想象力，但结论仍须落地。
    Creative,
}

impl ToneStyle {
    /// 固定顺序，便于遍历与测试。
    pub const ALL: [ToneStyle; 7] = [
        ToneStyle::Straightforward,
        ToneStyle::Friendly,
        ToneStyle::Professional,
        ToneStyle::Efficient,
        ToneStyle::Sarcastic,
        ToneStyle::Socratic,
        ToneStyle::Creative,
    ];

    /// 该风格的正文（全中文，≤200 字符）。
    ///
    /// 正文只描述表达方式，不涉及工具、权限或模式分工（正交性约束）。
    pub fn content(self) -> &'static str {
        match self {
            ToneStyle::Straightforward => {
                "先给结论，再给理由。用最少的铺垫把事实、判断和建议说清楚；不确定的地方直接标注，不绕弯子，也不为了语气而稀释信息。"
            }
            ToneStyle::Friendly => {
                "语气温和、有耐心，像同事之间的交流。先接住对方的问题与情绪，再给出可执行的帮助；不用客套堆砌，也不居高临下，让对方感到被理解。"
            }
            ToneStyle::Professional => {
                "用语准确、结构清晰，先界定问题与范围，再给出依据、方案与风险。区分事实与推断，保留必要的限定条件，避免夸张与口语化的随意表达。"
            }
            ToneStyle::Efficient => {
                "直入主题，用要点列表组织信息，一条只讲一件事。能一句说完就不写三句，省去重复背景与多余修饰；先给可用结果，细节按需展开。"
            }
            ToneStyle::Sarcastic => {
                "偶尔用轻微的反讽或自嘲点出事情的荒诞之处，但不针对用户、不贬低任何人。调侃只落在事情本身，事实、数据与结论必须保持准确，宁可不幽默也不冒犯。"
            }
            ToneStyle::Socratic => {
                "不急着给答案，先用问题帮对方澄清目标、假设与取舍。每次只问最关键的几点，得到回答后再推进；必要时给出候选方向，但把判断权留给对方。"
            }
            ToneStyle::Creative => {
                "敢于提出不同寻常的视角与类比，把零散线索连成新方案。想象可以放开，但最终必须落到可执行的步骤、代价与验证方式上，不做无法落地的空想。"
            }
        }
    }
}

/// 风格段头：标签 + 正交性声明（MUST adopt）。
pub const TONE_AND_STYLE_HEADER: &str =
    "<tone_and_style>\n风格只影响怎么说不影响说什么，准确性优先级高于风格。\n";

/// 风格段尾标签。
pub const TONE_AND_STYLE_FOOTER: &str = "</tone_and_style>";

/// 把风格输入渲染为完整 `<tone_and_style>` 区块。
///
/// 三个分支：
/// 1. 空串或纯空白 ⇒ 空串（调用方据此整段省略）；
/// 2. 七个枚举名之一（大小写不敏感，`-` / `_` 与无分隔等价）⇒ 用
///    [`ToneStyle::content`] 作正文；
/// 3. 其余自由文本 ⇒ 按原样作正文（宿主可传自定义风格）。
pub fn render(tone_style: &str) -> String {
    let trimmed = tone_style.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let body = match ToneStyle::from_name(trimmed) {
        Some(style) => style.content(),
        None => trimmed,
    };
    format!("{TONE_AND_STYLE_HEADER}{body}\n{TONE_AND_STYLE_FOOTER}")
}

impl ToneStyle {
    /// 枚举名识别：大小写不敏感，`-` / `_` 与无分隔等价。
    fn from_name(name: &str) -> Option<Self> {
        let normalized = name.trim().to_ascii_lowercase().replace(['-', '_'], "");
        match normalized.as_str() {
            "straightforward" => Some(ToneStyle::Straightforward),
            "friendly" => Some(ToneStyle::Friendly),
            "professional" => Some(ToneStyle::Professional),
            "efficient" => Some(ToneStyle::Efficient),
            "sarcastic" => Some(ToneStyle::Sarcastic),
            "socratic" => Some(ToneStyle::Socratic),
            "creative" => Some(ToneStyle::Creative),
            _ => None,
        }
    }
}
