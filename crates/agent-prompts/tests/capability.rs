//! 五模式能力面断言（从 `modes.rs` 按职责拆出，保持所有断言原样）。
//!
//! 能力面按 quick → ask → craft → plan(=craft+gate) → expert(=craft+协作) 递增：
//! 每个模式声明自己的工具白名单与行为段，plan / expert 在 craft 之上只增不减。

#[path = "common/mod.rs"]
mod common;

use agent_prompts::{Composer, PromptMode};
use common::{
    assert_no_blank_line_runs, assert_no_empty_section, bare_vars, full_vars, has_all, has_none,
    ALL_MODES, CRAFT_LINES, GATE_LINES,
};

const PLAN_TASK_SECTION: &str = "## 规划态的任务管理
- 进入规划态后，把方案拆成可独立验证的步骤再建任务，批准后逐条推进。
- 计划未获批准前，任务只建不开始：不要把未执行的动作标记成进行中或已完成。";

// ---- 五模式能力面 ----

#[test]
fn quick_declares_no_tools_and_answer_only() {
    let out = Composer::build(PromptMode::Quick, &full_vars());
    has_all(
        &out,
        &[
            "## 能力边界（quick · 只答不操作）",
            "- 本模式不启用任何工具：不读取文件、不写入文件、不执行命令、不访问网络、不操作浏览器。",
            "- 只基于用户提供的信息与既有知识作答；不改动任何文件，不安排执行任务。",
        ],
    );
    // quick 不含任何工具说明
    has_none(
        &out,
        &[
            "## 工具能力（ask · 只读白名单）",
            "## 工具能力（craft · 全工具）",
            "## 规划门禁（plan）",
            "## 专家协作（expert）",
            "fs.read",
            "shell.exec",
            "browser.open",
            "权限门",
            "## 智能体循环",
            "## 工具调用",
            "## 任务管理",
        ],
    );
}

#[test]
fn ask_documents_readonly_whitelist_only() {
    let out = Composer::build(PromptMode::Ask, &full_vars());
    has_all(
        &out,
        &[
            "## 工具能力（ask · 只读白名单）",
            "- 允许的工具仅限只读类别：fs.list、fs.read、fs.search。",
            "- 只读白名单之外的动作一律不发起：不写入、不删除、不执行命令、不访问外部网络、不操作浏览器。",
            "- 需要改动时先说明缺口与所需授权，请用户切换到 plan 或 craft。",
        ],
    );
    has_none(
        &out,
        &[
            "## 工具能力（craft · 全工具）",
            "## 规划门禁（plan）",
            "## 专家协作（expert）",
            "fs.write",
            "fs.delete",
            "shell.exec",
            "net.fetch",
            "browser.open",
            "- 本模式不启用任何工具",
            // ask 不挂 agent-loop（21 册 §1「ask | agent-loop(空)」）。
            "## 智能体循环",
            // ask 不建任务。
            "## 任务管理",
        ],
    );
}

#[test]
fn craft_documents_full_tool_surface() {
    let out = Composer::build(PromptMode::Craft, &full_vars());
    has_all(&out, &CRAFT_LINES);
    has_none(
        &out,
        &[
            "## 工具能力（ask · 只读白名单）",
            "## 规划门禁（plan）",
            "## 专家协作（expert）",
            "- 本模式不启用任何工具",
        ],
    );
}

#[test]
fn plan_is_craft_plus_gate() {
    let craft = Composer::build(PromptMode::Craft, &full_vars());
    let plan = Composer::build(PromptMode::Plan, &full_vars());
    has_all(&plan, &CRAFT_LINES); // craft 段完整复用
    has_all(&plan, &GATE_LINES);
    has_none(&plan, &["## 专家协作（expert）"]);

    // 门禁段插在 craft 段之后、硬规则之前
    let craft_at = plan.find("## 工具能力（craft · 全工具）").unwrap();
    let gate_at = plan.find("## 规划门禁（plan）").unwrap();
    let hard_at = plan.find("## 硬规则").unwrap();
    assert!(craft_at < gate_at && gate_at < hard_at, "plan 段落顺序错误");

    // 除门禁段、plan 专属任务管理段与模式标题行外，plan 与 craft 逐字一致
    // （craft 段完整复用）。
    let removed = plan
        .replace(&format!("{}\n\n", Composer::plan_gate()), "")
        .replace(&format!("{}\n\n", PLAN_TASK_SECTION), "");
    assert_ne!(removed, plan, "门禁段未被单独定位");
    assert_eq!(removed.replace("模式：plan", "模式：craft"), craft);
}

#[test]
fn expert_is_craft_plus_expertise_section() {
    let out = Composer::build(PromptMode::Expert, &full_vars());
    has_all(&out, &CRAFT_LINES);
    has_all(
        &out,
        &[
            "## 专家协作（expert）",
            "- 可把子任务分派给专家角色（研究、编码、评审、测试），并汇总其结论与证据。",
            "- 专家分工必须在计划里显式列出：谁负责什么、如何验收，不重复结论。",
            "- 专家的产出同样受硬规则与权限门约束：未实际发生的调用不得转述为已完成。",
        ],
    );
    has_none(
        &out,
        &["## 规划门禁（plan）", "## 工具能力（ask · 只读白名单）"],
    );
}

#[test]
fn different_modes_produce_different_prompts() {
    let base = Composer::build(PromptMode::Quick, &full_vars());
    for mode in [
        PromptMode::Ask,
        PromptMode::Plan,
        PromptMode::Craft,
        PromptMode::Expert,
    ] {
        assert_ne!(
            base,
            Composer::build(mode, &full_vars()),
            "{mode:?} 与 quick 相同"
        );
    }
}

#[test]
fn every_mode_renders_without_hollow_sections() {
    for mode in ALL_MODES {
        let out = Composer::build(mode, &bare_vars());
        assert_no_empty_section(&out);
        assert_no_blank_line_runs(&out);
    }
}
