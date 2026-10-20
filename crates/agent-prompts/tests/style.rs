//! 风格层断言（A5）：七段内容、`render` 三分支、正交性、段级差异与空值省略。

use std::collections::HashSet;

use agent_prompts::style::{render, ToneStyle, TONE_AND_STYLE_HEADER};
use agent_prompts::{Composer, PromptMode, PromptVars};

const MODE_NAMES: [&str; 5] = ["craft", "ask", "plan", "quick", "expert"];

/// 1. 七个变体的正文两两不同。
#[test]
fn seven_variants_have_distinct_content() {
    let contents: HashSet<&str> = ToneStyle::ALL.iter().map(|style| style.content()).collect();
    assert_eq!(contents.len(), 7, "七段正文必须两两不同");
}

/// 2. 每段正文 ≤200 字符（按 `chars().count()`）。
#[test]
fn each_content_is_at_most_200_chars() {
    for style in ToneStyle::ALL {
        assert!(
            style.content().chars().count() <= 200,
            "{style:?} 正文超过 200 字符：{}",
            style.content().chars().count()
        );
    }
}

/// 3. 空串与纯空白 ⇒ 空串（整段省略）。
#[test]
fn empty_input_renders_empty() {
    assert_eq!(render(""), "");
    assert_eq!(render("   "), "");
    assert_eq!(render("\n\t "), "");
}

/// 4. 枚举名大小写不敏感、`-` / `_` 与无分隔等价。
#[test]
fn enum_names_are_normalized() {
    let expected = render("straightforward");
    assert_eq!(expected, render("Straightforward"));
    assert_eq!(expected, render("straight_forward"));
    assert!(expected.contains("<tone_and_style>"));
    assert!(expected.contains(ToneStyle::Straightforward.content()));
}

/// 5. 自由文本按原样包裹。
#[test]
fn free_text_is_wrapped_verbatim() {
    let out = render("自定义风格文本");
    assert!(out.starts_with(TONE_AND_STYLE_HEADER));
    assert!(out.contains("自定义风格文本"));
    assert!(out.contains("<tone_and_style>"));
    assert!(out.trim_end().ends_with("</tone_and_style>"));
}

/// 6. 正交性：正文不叙述工具、权限或模式名。
#[test]
fn content_is_orthogonal_to_tools_permissions_and_modes() {
    for style in ToneStyle::ALL {
        let lower = style.content().to_lowercase();
        for mode in MODE_NAMES {
            assert!(
                !lower.contains(mode),
                "{style:?} 正文含模式名 {mode}：{}",
                style.content()
            );
        }
        for forbidden in ["tool", "工具", "permission", "授权"] {
            assert!(
                !lower.contains(forbidden),
                "{style:?} 正文含正交性禁词 {forbidden}：{}",
                style.content()
            );
        }
    }
}

/// 同一份 vars 仅改 `tone_style`：差异只在 `<tone_and_style>` 区块内。
#[test]
fn switching_style_changes_only_the_style_block() {
    for mode in PromptMode::ALL {
        let left = vars_with("STYLE-ALPHA");
        let right = vars_with("STYLE-BETA");
        let a = Composer::build(mode, &left);
        let b = Composer::build(mode, &right);
        assert_ne!(a, b, "{mode:?} 改 tone_style 后输出未变");
        assert!(a.contains("<tone_and_style>"), "{mode:?} 缺风格段");
        assert_eq!(
            outside_style_block(&a),
            outside_style_block(&b),
            "{mode:?} 的差异泄漏到 <tone_and_style> 区块之外"
        );
    }
}

/// 8. `tone_style` 为空 ⇒ 五模式都不含 `<tone_and_style>`。
#[test]
fn empty_tone_style_omits_block_in_all_modes() {
    for mode in PromptMode::ALL {
        let out = Composer::build(mode, &PromptVars::default());
        assert!(
            !out.contains("<tone_and_style>"),
            "{mode:?} 在空 tone_style 下仍渲染了风格段"
        );
        assert!(!out.contains("</tone_and_style>"));
    }
}

/// 构造只含必需字段与指定风格的最小变量集。
fn vars_with(tone_style: &str) -> PromptVars {
    PromptVars {
        ai_name: "小澜".into(),
        user_name: "Kim".into(),
        workspace: "/work/demo".into(),
        date: "2026-05-06".into(),
        tone_style: tone_style.into(),
        ..PromptVars::default()
    }
}

/// 去掉 `<tone_and_style>` 区块后的逐行内容。
fn outside_style_block(text: &str) -> Vec<&str> {
    let mut inside = false;
    let mut outside = Vec::new();
    for line in text.lines() {
        if line.starts_with("<tone_and_style>") {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with("</tone_and_style>") {
                inside = false;
            }
            continue;
        }
        outside.push(line);
    }
    outside
}
