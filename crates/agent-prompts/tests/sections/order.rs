//! 断言 ⑤⑥：段序 = 07 §3.1 的 section 顺序且两次构建一致；agent loop 8 步有序、
//! 不含文件传输步骤；plan 专项核对。

use agent_prompts::{Composer, PromptMode};

use super::support::{full_craft, table, Form};
use crate::common::{assert_no_blank_line_runs, has_all, has_none};

/// 段序基准表：各段在输出中的定位标记，按 07 §3.1 的 section 编号升序排列。
///
/// 不纳入条件段（它们在 `vars()` 里关掉），也不纳入并入头部的 `<user_info>`
/// ——条件段的出现与否由 `table.rs` 断言，纳入基准表会让段序随变量漂移。
const ORDER_BASELINE: [&str; 17] = [
    "## 内容政策",
    "## 个人目录安全",
    "## 区域惯例",
    "## 工作模式",
    "## 智能体循环",
    "## 结果呈现",
    "## 文件分享",
    "## 最终答复要求",
    "## 工具调用",
    "## 任务管理",
    "## 行为示例",
    "## 追问纪律",
    "## 工具使用策略",
    "## 系统提醒",
    "## 技能加载",
    "## 可用技能",
    "## MCP 配置",
];

/// 断言 ⑤：craft 段序 = 07 §3.1 的顺序；两次构建逐字一致（确定性）。
#[test]
fn section_order_matches_reference_and_is_stable() {
    let first = full_craft();
    let second = full_craft();
    assert_eq!(first, second, "两次构建不一致（确定性被破坏）");

    // 基准表本身就是按 §3.1 编号升序取出来的（自检）。
    let numbered: Vec<u8> = ORDER_BASELINE
        .iter()
        .map(|marker| {
            table()
                .iter()
                .find(|row| matches!(row.form, Form::Section(title) if title == *marker))
                .unwrap_or_else(|| panic!("基准表里的 {marker} 不在 30 行表中"))
                .number
        })
        .collect();
    let mut sorted = numbered.clone();
    sorted.sort_unstable();
    assert_eq!(numbered, sorted, "基准表必须按 §3.1 编号升序");

    let mut previous = 0usize;
    for marker in ORDER_BASELINE {
        let at = first
            .find(marker)
            .unwrap_or_else(|| panic!("缺 {marker}\n{first}"));
        assert!(at > previous, "{marker} 顺序错乱\n{first}");
        previous = at;
    }
    assert_eq!(
        ORDER_BASELINE.len(),
        17,
        "基准表 17 段（不含条件段与并入头部的段）"
    );
    assert_no_blank_line_runs(&first);
}

/// `agent_loop` 必须落在 `result_presentation` 之前（§3.1 第 7 项 < 第 8 项）。
#[test]
fn agent_loop_precedes_result_presentation() {
    let out = full_craft();
    let loop_at = out.find("## 智能体循环").expect("缺智能体循环");
    let result_at = out.find("## 结果呈现").expect("缺结果呈现");
    assert!(
        loop_at < result_at,
        "agent_loop 必须排在 result_presentation 之前"
    );
}

/// plan：门禁与规划态任务管理紧跟能力段，其余段序与 craft 相同。
#[test]
fn plan_inserts_gate_after_capability_without_reordering() {
    let plan = Composer::build(PromptMode::Plan, &super::support::full_craft_vars());
    let capability = plan
        .find("## 工具能力（craft · 全工具）")
        .expect("缺能力段");
    let gate = plan.find("## 规划门禁（plan）").expect("缺门禁段");
    let planned_tasks = plan.find("## 规划态的任务管理").expect("缺规划态任务管理");
    let common = plan.find("## 内容政策").expect("缺公共层");
    assert!(
        capability < gate && gate < planned_tasks && planned_tasks < common,
        "plan 的片段顺序错误"
    );
    // plan 段序与 craft 一致（去掉两个插片段后逐字回落到 craft 段序：以基准表核对）。
    let mut previous = 0usize;
    for marker in ORDER_BASELINE {
        let at = plan
            .find(marker)
            .unwrap_or_else(|| panic!("plan 缺 {marker}"));
        assert!(at > previous, "plan 的 {marker} 顺序错乱");
        previous = at;
    }
}

/// 断言 ⑥：agent loop 8 步有序，且不含第 8 步 File transfer。
#[test]
fn agent_loop_has_eight_ordered_steps() {
    let out = full_craft();
    let start = out.find("## 智能体循环").expect("缺智能体循环");
    let body = &out[start..];
    let body = body.split("\n\n## ").next().unwrap_or(body);
    let lines: Vec<&str> = body.lines().skip(1).collect();
    assert_eq!(lines.len(), 8, "agent loop 必须是 8 步\n{body}");
    for (index, line) in lines.iter().enumerate() {
        let prefix = format!("{}. ", index + 1);
        assert!(
            line.starts_with(&prefix),
            "第 {} 步应以 {prefix:?} 开头：{line}",
            index + 1
        );
    }
    assert_eq!(
        lines[0],
        "1. 分析上下文：基于上下文理解用户意图与当前状态。"
    );
    assert_eq!(lines[5], "6. 迭代循环：耐心重复，直至任务完全完成。");
    assert!(lines[6].starts_with("7. 【强制】呈现结果"));
    assert!(lines[7].starts_with("8. 【强制】最终答复"));
    // 不含第 8 步 File transfer（21 §3：它是交付变体特化步，不进共用基座）。
    has_none(
        &out,
        &[
            "File transfer",
            "从电脑传文件",
            "产物回传到小程序",
            "小程序回传开关",
            "9. 【强制】",
        ],
    );
    // ask 不挂 agent loop（07 §3.3 ask 片段 agent-loop 为空）。
    let ask = Composer::build(PromptMode::Ask, &super::support::full_craft_vars());
    has_none(&ask, &["## 智能体循环"]);
    // craft / plan / expert 挂同一份 8 步（07 §3.3 三模式共用 agent-loop）。
    for mode in [PromptMode::Plan, PromptMode::Expert] {
        let other = Composer::build(mode, &super::support::full_craft_vars());
        let at = other
            .find(body)
            .expect("craft 系模式应共用同一份 agent loop");
        assert!(at > 0);
    }
    has_all(&ask, &["## 结果呈现"]);
}
