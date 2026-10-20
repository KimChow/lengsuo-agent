//! 身份注入两分支（onboarding / 常态）与模式提醒的断言（提示词线 A3）。
//!
//! 依据：本地对照资料 18 §13（身份注入运行时 + 模式提醒原文）、04 §2.2（身份文件
//! 清单）；规格 `harness-prompts.md` §8 / §9 / §15 偏离表第 5、6 条。
//!
//! 口径：**纯数据面 + 渲染**——本 crate 不读身份文件，四文件正文与路径都由调用方
//! 填进 [`PromptVars`]；身份段缺失即信号，空值标 `(empty or missing)` **不省略**
//! （与记忆段「全空整段省略」语义相反）。

mod common;

use agent_prompts::{Composer, PromptMode, PromptVars};
use common::{bare_vars, full_vars, has_none, ALL_MODES};

/// 空文件 / 缺路径的显式标（18 §13 原文；身份段专用、记忆段不用）。
const EMPTY_MARK: &str = "(empty or missing)";

/// 四文件标题（有序；onboarding 分支多出 BOOTSTRAP）。
const FILE_TITLES: [&str; 3] = ["## SOUL.md", "## IDENTITY.md", "## USER.md"];

/// onboarding 分支样本：四文件与四路径全部填值。
fn onboarding_vars() -> PromptVars {
    let mut vars = bare_vars();
    vars.identity_mode = "onboarding".into();
    vars.soul = "SOUL-冷静直接先验证".into();
    vars.identity = "IDENT-执行助手".into();
    vars.user = "USER-用户档案：偏好中文".into();
    vars.bootstrap = "BOOT-首次引导".into();
    vars.soul_path = "/data/user-1/SOUL.md".into();
    vars.identity_path = "/data/user-1/IDENTITY.md".into();
    vars.user_path = "/data/user-1/USER.md".into();
    vars.bootstrap_path = "/data/user-1/BOOTSTRAP.md".into();
    vars
}

/// 常态分支样本：同一份四文件，只把分支切到 `normal`。
fn normal_vars() -> PromptVars {
    let mut vars = onboarding_vars();
    vars.identity_mode = "normal".into();
    vars
}

// ---- 两分支：onboarding ----

#[test]
fn onboarding_branch_renders_bootstrap_section_and_discipline() {
    let out = Composer::identity_context(&onboarding_vars());
    for title in FILE_TITLES {
        assert!(out.contains(title), "onboarding 缺 {title}\n{out}");
    }
    // 四文件全文都在（内容逐字注入，不省略）。
    assert!(
        out.contains("## BOOTSTRAP.md"),
        "onboarding 缺 BOOTSTRAP 段\n{out}"
    );
    assert!(out.contains("SOUL-冷静直接先验证"));
    assert!(out.contains("IDENT-执行助手"));
    assert!(out.contains("USER-用户档案：偏好中文"));
    assert!(out.contains("BOOT-首次引导"));
    // 出生证明 + 只在对话有空间时遵循 + 学完删 BOOTSTRAP.md（18 §13 原文要点）。
    assert!(out.contains("出生证明"), "缺「出生证明」定位\n{out}");
    assert!(out.contains("只在对话有空间时"), "缺礼貌时机约束\n{out}");
    assert!(out.contains("学完删 BOOTSTRAP.md"), "缺删档纪律\n{out}");
    assert!(out.contains("保持对话自然"));
}

// ---- 两分支：常态 ----

#[test]
fn normal_branch_omits_bootstrap_section_but_keeps_three_files() {
    let out = Composer::identity_context(&normal_vars());
    for title in FILE_TITLES {
        assert!(out.contains(title), "常态缺 {title}\n{out}");
    }
    // 常态不渲染 BOOTSTRAP 段，也不提删档纪律。
    has_none(
        &out,
        &["## BOOTSTRAP.md", "BOOT-首次引导", "学完删 BOOTSTRAP.md"],
    );
    // 常态纪律：与最近注入的档案一致 + 改了 SOUL.md 要告诉用户。
    assert!(out.contains("改了 SOUL.md 要告诉用户"), "缺告知纪律\n{out}");
    assert!(out.contains("与最近注入的身份/用户档案保持一致"));
}

// ---- 空值语义：显式标缺失，不省略 ----

#[test]
fn empty_file_content_renders_explicit_mark_not_omission() {
    let mut vars = onboarding_vars();
    vars.soul.clear();
    vars.identity.clear();
    vars.user.clear();
    vars.bootstrap.clear();
    let out = Composer::identity_context(&vars);
    // 标题一个都不能少：文件缺失本身就是 onboarding 的信号（18 §13）。
    for title in FILE_TITLES.iter().chain(["## BOOTSTRAP.md"].iter()) {
        assert!(out.contains(title), "空文件时 {title} 整段消失\n{out}");
    }
    assert_eq!(
        out.matches(EMPTY_MARK).count(),
        4,
        "四个空文件应各标一次缺失\n{out}"
    );
}

#[test]
fn identity_section_still_renders_when_all_four_files_are_empty() {
    // 常态分支 + 四文件全空：身份段整体仍出现（缺失即信号）。
    let mut vars = bare_vars();
    vars.identity_mode = "normal".into();
    vars.soul.clear();
    let out = Composer::identity_context(&vars);
    assert!(out.contains("<identity_context>"), "身份段整体消失\n{out}");
    assert!(out.contains("</identity_context>"));
    for title in FILE_TITLES {
        assert!(out.contains(title), "四文件全空时缺 {title}\n{out}");
    }
}

#[test]
fn whitespace_only_content_is_treated_as_missing() {
    let mut vars = normal_vars();
    vars.soul = "  \n\t ".into();
    vars.user = "\n \n".into();
    let out = Composer::identity_context(&vars);
    assert_eq!(
        out.matches(EMPTY_MARK).count(),
        2,
        "纯空白未视为缺失\n{out}"
    );
}

// ---- 路径注入 ----

#[test]
fn path_lines_carry_injected_paths() {
    let out = Composer::identity_context(&onboarding_vars());
    assert!(
        out.contains("## SOUL.md\nPath: /data/user-1/SOUL.md\nSOUL-冷静直接先验证"),
        "SOUL 段格式不是「标题 → Path → 正文」\n{out}"
    );
    assert!(out.contains("## IDENTITY.md\nPath: /data/user-1/IDENTITY.md\n"));
    assert!(out.contains("## USER.md\nPath: /data/user-1/USER.md\n"));
    assert!(out.contains("## BOOTSTRAP.md\nPath: /data/user-1/BOOTSTRAP.md\n"));
}

#[test]
fn empty_path_line_keeps_placeholder_instead_of_blank() {
    let mut vars = normal_vars();
    vars.soul_path.clear();
    let out = Composer::identity_context(&vars);
    assert!(
        out.contains("## SOUL.md\nPath: (empty or missing)\n"),
        "空路径应给占位而不是空行\n{out}"
    );
}

// ---- 渲染形态 ----

#[test]
fn identity_context_is_a_single_wrapped_block() {
    let out = Composer::identity_context(&onboarding_vars());
    assert!(out.starts_with("<identity_context>\n"), "缺开标签\n{out}");
    assert!(out.ends_with("</identity_context>\n"), "缺闭标签\n{out}");
    // 花括号注入值不得被二次展开，也不留 `{{` 占位符。
    assert!(!out.contains("{{"));
}

#[test]
fn identity_values_are_not_rescanned() {
    let mut vars = normal_vars();
    vars.soul = "SOUL-{{user_name}} 原样保留".into();
    let out = Composer::identity_context(&vars);
    assert!(out.contains("SOUL-{{user_name}} 原样保留"));
}

#[test]
fn identity_context_is_deterministic() {
    for vars in [onboarding_vars(), normal_vars(), bare_vars()] {
        let first = Composer::identity_context(&vars);
        let second = Composer::identity_context(&vars);
        assert_eq!(first, second, "身份段两次渲染不一致");
    }
}

// ---- 模式提醒（§9）----

#[test]
fn mode_reminder_is_none_outside_craft_and_ask() {
    for mode in [PromptMode::Quick, PromptMode::Plan, PromptMode::Expert] {
        assert_eq!(Composer::mode_reminder(mode), None, "{mode:?} 不应有提醒");
    }
}

#[test]
fn mode_reminder_returns_craft_and_ask_texts() {
    let craft = Composer::mode_reminder(PromptMode::Craft).expect("craft 缺提醒");
    assert!(craft.contains("You are now in Agent mode"));

    let ask = Composer::mode_reminder(PromptMode::Ask).expect("ask 缺提醒");
    assert!(ask.contains("Ask mode is active"));
    assert!(
        ask.contains("This supersedes any other instructions you have received"),
        "ask 提醒缺「压倒先前指令」声明"
    );
    assert!(ask.contains("MUST NOT make any edits"));
}

#[test]
fn mode_reminder_is_absent_from_build_output() {
    // 运行时追加（18 §13「切换时追加」）：不进确定性纯函数 `build` 的输出。
    for mode in ALL_MODES {
        let out = Composer::build(mode, &full_vars());
        for reminder in [PromptMode::Craft, PromptMode::Ask] {
            if let Some(text) = Composer::mode_reminder(reminder) {
                assert!(!out.contains(text), "{mode:?} 的 build 混入了模式提醒");
            }
        }
    }
}

// ---- 确定性不被破坏 ----

#[test]
fn build_stays_deterministic_with_identity_vars() {
    for mode in ALL_MODES {
        let first = Composer::build(mode, &onboarding_vars());
        let second = Composer::build(mode, &onboarding_vars());
        assert_eq!(first, second, "{mode:?} 两次 build 不一致");
    }
}
