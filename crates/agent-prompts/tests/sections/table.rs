//! 断言 ①④：30 行表（编号 1..=30 不缺不重）+ 做了的段在应该出现的模式出现、
//! 不该出现的模式必然不出现 + 不做的段五模式一律不渲染。

use agent_prompts::{Composer, PromptMode};

use super::support::{bare_craft, full_craft, table, Form, Row, Verdict, NOT_DONE_MARKERS};
use crate::common::{bare_vars, full_vars, has_all, ALL_MODES};

/// 断言 ①：表就是 07 §3.1 的 30 个标签，原文顺序，不缺不重。
#[test]
fn table_has_thirty_rows_in_reference_order() {
    let table = table();
    assert_eq!(table.len(), 30, "规格 §2 全表 30 行");
    for (index, row) in table.iter().enumerate() {
        assert_eq!(
            usize::from(row.number),
            index + 1,
            "编号必须是 1..=30 的原文顺序：{}",
            row.tag
        );
    }
    // 07 §3.1 的原文标签清单逐字比对（顺序即编号）。
    let reference = [
        "user_query",
        "content_policy",
        "personal_files_safety",
        "windows_command_safety",
        "regional_conventions",
        "working_modes",
        "agent_loop",
        "result_presentation",
        "sharing_files",
        "final_answer_instructions",
        "automations",
        "tool_use",
        "file_id",
        "doc_url",
        "instructions_for_visualizer",
        "user_info",
        "visualizer_examples",
        "task_management",
        "example",
        "asking_questions",
        "user-prompt-submit-hook",
        "tool_usage_policy",
        "system-reminder",
        "agent_mail",
        "agent_skills",
        "available_skills",
        "expert_management",
        "mcp_configuration",
        "response_language",
        "binary_context",
    ];
    let tags: Vec<&str> = table.iter().map(|row| row.tag).collect();
    assert_eq!(tags, reference, "标签或顺序与 07 §3.1 不一致");
}

/// 断言 ②：做了的段按「出现 / 不出现」模式逐条核对（两种变量下都成立）。
#[test]
fn done_sections_appear_exactly_in_their_modes() {
    let table = table();
    let mut done = 0;
    for row in &table {
        if row.verdict == Verdict::NotDone {
            continue;
        }
        done += 1;
        let marker = row.marker().expect("做了的段必须有定位标记");
        for vars in [full_vars(), bare_vars()] {
            for mode in ALL_MODES {
                let out = Composer::build(mode, &vars);
                let expected = row.renders_in(mode, &vars);
                assert_eq!(
                    out.contains(marker),
                    expected,
                    "第 {} 项 <{}> 在 {mode:?} 的渲染结果与判定不符（期望出现={expected}）\n{out}",
                    row.number,
                    row.tag,
                    out = out.replace('\n', " | ")
                );
            }
        }
        // 「做了」的段要么属于全模式，要么显式列出不出现的模式；非条件段的
        // 正反两张名单必须互为补集（条件段只单向列「可出现」模式，条件成立与否
        // 由 `condition_holds` 判定，不在 `absent_in` 里重复）。
        if !matches!(row.form, Form::Conditional(_)) {
            let mut union: Vec<PromptMode> = row.modes.to_vec();
            union.extend_from_slice(row.absent_in);
            assert_eq!(
                union.len(),
                ALL_MODES.len(),
                "第 {} 项 <{}> 的出现 / 不出现名单有重叠或遗漏",
                row.number,
                row.tag
            );
            for mode in ALL_MODES {
                assert!(
                    row.modes.contains(&mode) != row.absent_in.contains(&mode),
                    "第 {} 项 <{}> 的 {mode:?} 不应同时出现在两张名单里",
                    row.number,
                    row.tag
                );
            }
        }
    }
    // 出处可回查：做了的行都标了册号 + 节号（如「18 §10.1」），可 grep 回参考册。
    for row in &table {
        if row.verdict == Verdict::NotDone {
            continue;
        }
        assert!(
            row.source
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_digit)
                && row.source.contains("§"),
            "第 {} 项 <{}> 出处缺册号节号：{:?}",
            row.number,
            row.tag,
            row.source
        );
    }
    // 必做 15 + 可后置落地 6 = 21 段。
    assert_eq!(done, 21, "落地段数应为 21");
    // 不做段 9 段的「不出现」名单必须覆盖全部模式。
    let not_done: Vec<&Row> = table
        .iter()
        .filter(|row| row.verdict == Verdict::NotDone)
        .collect();
    assert_eq!(not_done.len(), 9, "不做段应为 9 段");
    for row in not_done {
        assert_eq!(row.modes, &[] as &[PromptMode]);
        assert_eq!(row.absent_in.len(), ALL_MODES.len());
        assert!(!row.not_done.is_empty(), "不做段必须写理由");
        assert_eq!(row.form, Form::Omitted);
    }
}

/// 断言 ③：不做的 9 段五模式一律不渲染（无占位段、无空壳标题）。
#[test]
fn not_done_sections_never_rendered() {
    for vars in [full_vars(), bare_vars()] {
        for mode in PromptMode::ALL {
            let out = Composer::build(mode, &vars);
            for (tag, markers) in NOT_DONE_MARKERS {
                for marker in markers {
                    assert!(
                        !out.contains(marker),
                        "不做的段 <{tag}> 在 {mode:?} 出现了标记 {marker:?}\n{out}"
                    );
                }
            }
        }
    }
    // 条件全关时，三个条件段整段省略（不留标题）。
    let bare = bare_craft();
    let conditional: Vec<&str> = table()
        .iter()
        .filter_map(|row| match row.form {
            Form::Conditional(title) => Some(title),
            _ => None,
        })
        .collect();
    assert_eq!(
        conditional.len(),
        3,
        "三处条件段：Windows / 专家管理 / 回复语言"
    );
    for title in &conditional {
        assert!(!bare.contains(title), "条件不成立仍渲染 {title}\n{bare}");
    }
    has_all(&full_craft(), &conditional);
}
