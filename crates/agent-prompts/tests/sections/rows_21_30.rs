//! 30 段表第 21–30 项（07 §3.1 原文顺序）。

#![allow(dead_code)]

use agent_prompts::PromptMode;

use super::support::{Form, Row, Verdict, ALL, CRAFT_FAMILY};

pub(super) const ROWS_21_TO_30: [Row; 10] = [
    Row {
        number: 21,
        tag: "user-prompt-submit-hook",
        summary: "用户提交钩子",
        verdict: Verdict::NotDone,
        form: Form::Omitted,
        source: "07 §3.1 第 21 项",
        modes: &[],
        absent_in: ALL,
        not_done: "属宿主层钩子机制，不在提示词模板层。重新考虑：宿主定义提交钩子注入通道时。",
    },
    Row {
        number: 22,
        tag: "tool_usage_policy",
        summary: "工具使用策略九条",
        verdict: Verdict::Done,
        form: Form::Section("## 工具使用策略"),
        source: "18 §5",
        modes: &[
            PromptMode::Ask,
            PromptMode::Plan,
            PromptMode::Craft,
            PromptMode::Expert,
        ],
        absent_in: &[PromptMode::Quick],
        not_done: "",
    },
    Row {
        number: 23,
        tag: "system-reminder",
        summary: "系统提醒容器说明",
        verdict: Verdict::Done,
        form: Form::Section("## 系统提醒"),
        source: "18 §5",
        modes: &[
            PromptMode::Ask,
            PromptMode::Plan,
            PromptMode::Craft,
            PromptMode::Expert,
        ],
        absent_in: &[PromptMode::Quick],
        not_done: "",
    },
    Row {
        number: 24,
        tag: "agent_mail",
        summary: "智能体邮件",
        verdict: Verdict::NotDone,
        form: Form::Omitted,
        source: "07 §3.1 第 24 项",
        modes: &[],
        absent_in: ALL,
        not_done: "依赖内置邮件 MCP 服务器，本仓不内置该服务器。重新考虑：内置邮件 MCP 服务器时。",
    },
    Row {
        number: 25,
        tag: "agent_skills",
        summary: "技能加载纪律",
        verdict: Verdict::DeferredDone,
        form: Form::Section("## 技能加载"),
        source: "18 §9.1",
        modes: &[
            PromptMode::Ask,
            PromptMode::Plan,
            PromptMode::Craft,
            PromptMode::Expert,
        ],
        absent_in: &[PromptMode::Quick],
        not_done: "",
    },
    Row {
        number: 26,
        tag: "available_skills",
        summary: "可用技能清单",
        verdict: Verdict::DeferredDone,
        form: Form::Section("## 可用技能"),
        source: "18 §9.1；07 §3.1 第 26 项；07 §8",
        modes: &[
            PromptMode::Ask,
            PromptMode::Plan,
            PromptMode::Craft,
            PromptMode::Expert,
        ],
        absent_in: &[PromptMode::Quick],
        not_done: "",
    },
    Row {
        number: 27,
        tag: "expert_management",
        summary: "专家管理纪律",
        verdict: Verdict::DeferredDone,
        form: Form::Conditional("## 专家管理"),
        source: "18 §11.2；07 §5 条件段",
        modes: CRAFT_FAMILY,
        // 条件段只单向列出「可出现」模式：不出现的模式由 `renders_in` 按
        // 「不在 modes 或条件不成立」判定。专家管理是**写类**功能，只服务能
        // 执行写操作的模式（quick 只答、ask 只读，都不能创建/修改专家）。
        absent_in: &[PromptMode::Quick, PromptMode::Ask],
        not_done: "",
    },
    Row {
        number: 28,
        tag: "mcp_configuration",
        summary: "MCP 配置协议",
        verdict: Verdict::DeferredDone,
        form: Form::Section("## MCP 配置"),
        source: "18 §9.2",
        modes: &[
            PromptMode::Ask,
            PromptMode::Plan,
            PromptMode::Craft,
            PromptMode::Expert,
        ],
        absent_in: &[PromptMode::Quick],
        not_done: "",
    },
    Row {
        number: 29,
        tag: "response_language",
        summary: "回复语言",
        verdict: Verdict::Done,
        form: Form::Conditional("## 回复语言"),
        source: "07 §3.1 第 29 项；07 §5 条件段；07 §3.2",
        modes: ALL,
        absent_in: &[], // 无条件排除：出现与否只看 `response_language`
        not_done: "",
    },
    Row {
        number: 30,
        tag: "binary_context",
        summary: "二进制上下文",
        verdict: Verdict::NotDone,
        form: Form::Omitted,
        source: "07 §3.1 第 30 项",
        modes: &[],
        absent_in: ALL,
        not_done:
            "属多模态产物域（二进制上下文），本仓无多模态产物通道。重新考虑：接入多模态产物通道时。",
    },
];
