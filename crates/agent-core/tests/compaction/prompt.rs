//! 提示词构造与常量/错误契约的验收。
//!
//! 逐字对照 opencode `compaction.ts:137-143`（六条规则）、`:178-179`（提醒语）
//! 与 `:227-228`（overflow 预算瞄准 70%）。

use agent_core::runtime::compaction::prompt::{
    build_prompt, NUDGE, SUMMARY_RULES, SUMMARY_TEMPLATE,
};
use agent_core::runtime::compaction::{CompactionError, Trigger};

use super::support::{
    assistant, checkpoint, compaction, user, RESERVE_MIN, TOOL_OUTPUT_MAX_CHARS, UNKNOWN_WINDOW,
};

/// 首次提示含 "You MUST summarize the conversation above"；更新提示含
/// "Update the existing checkpoint"；两者都含完整八段模板。
#[test]
fn prompts_differ_by_mode_and_both_carry_full_template() {
    let first = build_prompt(false);
    let update = build_prompt(true);

    assert!(
        first.contains("You MUST summarize the conversation above"),
        "首次提示必须含该句"
    );
    assert!(
        update.contains("Update the existing checkpoint"),
        "更新提示必须含该句"
    );
    assert!(
        !first.contains("Update the existing checkpoint"),
        "首次提示不含更新句"
    );
    assert!(
        !update.contains("You MUST summarize the conversation above"),
        "更新提示不含首次句"
    );

    for prompt in [&first, &update] {
        for heading in [
            "## Objective",
            "## Requirements",
            "## Decisions",
            "## Work State",
            "### Completed",
            "### Active",
            "### Blocked",
            "## Next Move",
            "## Relevant Files",
            "## Important Context",
        ] {
            assert!(prompt.contains(heading), "提示缺段落标题：{heading}");
        }
        assert!(prompt.contains(SUMMARY_TEMPLATE), "提示必须含完整模板");
        assert!(prompt.contains(SUMMARY_RULES), "提示必须含完整规则");
    }
}

/// 六条规则逐条在位（照抄 `compaction.ts:137-143`）。
#[test]
fn summary_rules_carry_all_six_reference_rules() {
    for rule in [
        "Keep each section concise.",
        "Prefer short references over detailed restatement.",
        "Preserve exact file paths, symbols, commands, error strings, URLs, and identifiers.",
        "Carry forward only user questions or requests that remain unanswered or require further action.",
        "Preserve consequential workflow state",
        "Do not mention the summary process or that context was compacted.",
    ] {
        assert!(SUMMARY_RULES.contains(rule), "规则缺失：{rule}");
    }
}

/// 提醒语逐字照抄（参考 `compaction.ts:178-179`）。
#[test]
fn nudge_matches_reference_text() {
    assert_eq!(
        NUDGE,
        "The previous response did not fill in the required summary template. Do not call tools. Return the summary as text using the exact section headings from the template."
    );
}

/// 计划里的提示词按「有无既有 checkpoint」选首次/更新两套。
#[test]
fn plan_picks_update_prompt_when_a_checkpoint_exists() {
    let comp = compaction(Some(20_000), super::support::DEFAULT_KEEP);
    let first = comp
        .plan(&[user("需求"), assistant("回复")], Trigger::Auto)
        .expect("可压缩");
    assert!(first
        .prompt
        .contains("You MUST summarize the conversation above"));

    let second = comp
        .plan(
            &[checkpoint("摘要", ""), user("新需求"), assistant("新回复")],
            Trigger::Auto,
        )
        .expect("可压缩");
    assert!(
        second.prompt.contains("Update the existing checkpoint"),
        "已有 checkpoint ⇒ 用更新提示词"
    );
}

/// 无可压缩内容 ⇒ `compaction.unavailable`，不 panic。
#[test]
fn nothing_to_compact_yields_unavailable_error() {
    let comp = compaction(Some(20_000), super::support::DEFAULT_KEEP);
    let err = comp
        .plan(&[], Trigger::Auto)
        .expect_err("空历史无可压缩内容");
    assert_eq!(err.error_type(), "compaction.unavailable");
    assert_eq!(err.message(), "Nothing to compact yet");
}

/// 只有 checkpoint（无别的内容）⇒ 同样无可压缩内容。
#[test]
fn checkpoint_only_history_yields_unavailable_error() {
    let comp = compaction(Some(20_000), super::support::DEFAULT_KEEP);
    let err = comp
        .plan(&[checkpoint("摘要", "")], Trigger::Auto)
        .expect_err("只有 checkpoint ⇒ 无可压缩内容");
    assert_eq!(err.error_type(), "compaction.unavailable");
}

/// 常量逐项对照参考件。
#[test]
fn constants_match_reference_values() {
    use agent_core::runtime::compaction::{
        DEFAULT_KEEP, IMAGE_TOKEN_ESTIMATE, PDF_TOKEN_ESTIMATE, SHRINK_STEPS,
    };
    assert_eq!(SHRINK_STEPS, [0.7, 0.5, 0.35]);
    assert_eq!(RESERVE_MIN, 16_000);
    assert_eq!(UNKNOWN_WINDOW, 200_000);
    assert_eq!(TOOL_OUTPUT_MAX_CHARS, 1_250);
    assert_eq!(IMAGE_TOKEN_ESTIMATE, 1_500);
    assert_eq!(PDF_TOKEN_ESTIMATE, 2_000);
    assert_eq!(DEFAULT_KEEP, 15_000);
}

/// `overflow` 优先于 `auto`：overflow 的预算瞄准首次估算的 70%。
#[test]
fn overflow_trigger_shrinks_budget_to_seventy_percent() {
    let history = vec![user(&"x".repeat(1000))];
    let comp = compaction(Some(100_000), super::support::DEFAULT_KEEP);
    let auto = comp.plan(&history, Trigger::Auto).expect("可压缩");
    let overflow = comp.plan(&history, Trigger::Overflow).expect("可压缩");
    // 可见文本 = "[User]: " + 1000 字符 = 1008 token（CharEstimator）。
    // 窗口 100_000 ⇒ 上限 = 100_000 - max(10_000, RESERVE_MIN 16_000) = 84_000。
    assert_eq!(auto.budget, 84_000, "auto ⇒ 用上限");
    assert_eq!(overflow.budget, 705, "overflow ⇒ 瞄准首次估算的 70%");
}

/// `CompactionError` 的类型串与参考件 `SessionError.Error.type` 对齐。
#[test]
fn compaction_error_types_match_reference() {
    let unavailable = CompactionError::Unavailable {
        message: "Nothing to compact yet".into(),
    };
    assert_eq!(unavailable.error_type(), "compaction.unavailable");
    let failed = CompactionError::Failed {
        message: "boom".into(),
    };
    assert_eq!(failed.error_type(), "compaction.failed");
}
