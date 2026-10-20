//! 共享夹具：30 段的行类型（`Row` / `Verdict` / `Form`）、条件成立判定、
//! 「不做的段」标记与模式夹具。
//!
//! 30 行表本体在 `table_rows.rs`，本文件再导出（调用方只认 `support`）。

#![allow(dead_code)]

use agent_prompts::{Composer, PromptMode, PromptVars};

use crate::common::{bare_vars, full_vars};

pub(super) use super::table_rows::table;

/// 五个模式（参数化用例的遍历顺序）。
pub(super) const ALL: &[PromptMode] = &[
    PromptMode::Quick,
    PromptMode::Ask,
    PromptMode::Plan,
    PromptMode::Craft,
    PromptMode::Expert,
];
/// 不出现任何段的模式集合（不做段用）。
pub(super) const NONE: &[PromptMode] = &[];
/// 不挂 agent loop 的两模式（21 §1 ask agent-loop 为空）。
pub(super) const NO_LOOP: &[PromptMode] = &[PromptMode::Quick, PromptMode::Ask];
/// craft 系（craft 与复用它两段的 plan / expert）。
pub(super) const CRAFT_FAMILY: &[PromptMode] =
    &[PromptMode::Craft, PromptMode::Plan, PromptMode::Expert];
/// craft 系去掉 expert（专家管理段只服务具体专家实例）。
pub(super) const CRAFT_AND_PLAN: &[PromptMode] = &[PromptMode::Craft, PromptMode::Plan];

/// 本仓判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 已在 07 册 §3.1 的 30 段内落地正文（必做 15 段）。
    Done,
    /// 规格标「可后置」，本仓有能力支撑，本卡一并落地正文。
    DeferredDone,
    /// 本仓无对应运行时，本卡不做（不造占位段）。
    NotDone,
}

/// 本仓落地形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// 各模式模板里的独立段（按 `title` 在渲染结果中定位）。
    Section(&'static str),
    /// 条件段（`Cond` 形态）。
    Conditional(&'static str),
    /// 并入公共头 `## 身份`（`user_name` 插值，不另起标题）。
    MergedIntoHeader,
    /// 不落地：不出现任何标题与占位正文。
    Omitted,
}

/// 30 段中的一行。
#[derive(Debug, Clone, Copy)]
pub struct Row {
    /// 07 §3.1 原文顺序编号（1..=30）。
    pub number: u8,
    /// 标签名（去掉尖括号）。
    pub tag: &'static str,
    /// 一句话。
    pub summary: &'static str,
    /// 本仓判定。
    pub verdict: Verdict,
    /// 落地形态。
    pub form: Form,
    /// 出处（参考端代号 + 册号 + 节号，可 grep 回）。
    pub source: &'static str,
    /// 出现该段（独立段 / 条件段成立）的模式。
    pub modes: &'static [PromptMode],
    /// 不出现该段的模式（本仓判定为「做了」的段必须显式列出）。
    pub absent_in: &'static [PromptMode],
    /// 不做时的理由与重新考虑条件；做了为空。
    pub not_done: &'static str,
}

impl Row {
    /// 该模式在给定变量下是否应出现本段（条件段按条件成立判定）。
    pub fn renders_in(self, mode: PromptMode, vars: &PromptVars) -> bool {
        match self.form {
            Form::Section(_) | Form::MergedIntoHeader => self.modes.contains(&mode),
            Form::Conditional(title) => self.modes.contains(&mode) && condition_holds(title, vars),
            Form::Omitted => false,
        }
    }

    /// 该模式渲染结果里定位本段的**可搜索标记**（不出现时为 `None`）。
    pub fn marker(self) -> Option<&'static str> {
        match self.form {
            Form::Section(title) | Form::Conditional(title) => Some(title),
            Form::MergedIntoHeader => Some("本次服务对象："),
            Form::Omitted => None,
        }
    }
}

/// 条件段成立与否与本仓 `Cond` 一致：`is_windows` 为真、`response_language` 含中文、
/// 专家管理开关为真。
pub(super) fn condition_holds(title: &str, vars: &PromptVars) -> bool {
    match title {
        "## Windows 命令安全" => vars.is_windows,
        "## 回复语言" => vars.response_language.contains("中文"),
        "## 专家管理" => vars.feature_expert_management,
        _ => false,
    }
}

pub const NOT_DONE_MARKERS: [(&str, [&str; 1]); 9] = [
    // 1 <user_query>
    ("user_query", ["本轮用户输入："]),
    // 11 <automations>
    ("automations", ["automation_update"]),
    // 13 <file_id>
    ("file_id", ["file_id"]),
    // 14 <doc_url>
    ("doc_url", ["?_fid="]),
    // 15 <instructions_for_visualizer>
    ("instructions_for_visualizer", ["show_widget"]),
    // 17 <visualizer_examples>
    ("visualizer_examples", ["widget_guidelines"]),
    // 21 <user-prompt-submit-hook>
    ("user-prompt-submit-hook", ["user-prompt-submit-hook"]),
    // 24 <agent_mail>
    ("agent_mail", ["agent-mail"]),
    // 30 <binary_context>
    ("binary_context", ["二进制上下文"]),
];

/// 全条件成立的变量（is_windows / 中文 / 专家管理开关都为真），供 plan 复用。
pub fn full_craft_vars() -> PromptVars {
    let vars = full_vars();
    assert!(
        vars.is_windows
            && vars.response_language.contains("中文")
            && vars.feature_expert_management
    );
    vars
}

/// craft 全条件成立的输出。
pub fn full_craft() -> String {
    Composer::build(PromptMode::Craft, &full_craft_vars())
}

/// 条件全关的输出（三个条件段都应整段省略）。
pub fn bare_craft() -> String {
    let vars = bare_vars();
    assert!(!vars.is_windows && !vars.response_language.contains("中文"));
    Composer::build(PromptMode::Craft, &vars)
}
