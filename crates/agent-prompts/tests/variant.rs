//! 模板变体层 v1 的断言（提示词线 A8）。
//!
//! 覆盖：变体枚举归一化（大小写 / 分隔符 / 未知值回落）、模式 × 变体回落表
//! （回落在**返回值**上可观测：段序列相等）、craft 三变体互不相同且是默认
//! 段集的**超序列**、变体正文的正交性（不含工具名 / 权限 / 模式名）、
//! 旧签名 `build` 与新签名 `build_with` 的兼容性、渲染确定性。
//!
//! 变体正文由 `template.rs` 的三个 `pub(crate)` 常量提供；集成测试取不到
//! 私有常量，故从模板段序列里取出「变体侧重」段的正文再做正交性判定
//! （判定对象与常量本体逐字相同）。

use agent_prompts::{
    template_for, template_for_variant, Composer, PromptMode, PromptTemplate, PromptVariant,
    PromptVars, Segment,
};

/// 五个模式（兼容性断言逐个跑）。
const MODES: [PromptMode; 5] = [
    PromptMode::Quick,
    PromptMode::Ask,
    PromptMode::Plan,
    PromptMode::Craft,
    PromptMode::Expert,
];

/// 模式名（小写形态）：变体正文里不得出现。
const MODE_NAMES: [&str; 5] = ["quick", "ask", "plan", "craft", "expert"];

/// 本仓真实工具名：变体正文里不得出现（正文只讲关注点，不点工具）。
const TOOL_NAMES: [&str; 9] = [
    "fs.list",
    "fs.read",
    "fs.write",
    "fs.delete",
    "shell.exec",
    "net.fetch",
    "browser.open",
    "plan.enter",
    "plan.exit",
];

/// 权限 / 授权叙述的禁词（与风格层的正交性判据同口径）。
const PERMISSION_WORDS: [&str; 6] = ["tool", "工具", "permission", "权限", "授权", "allow"];

/// 变体段正文在段序列里的段首标记。
const VARIANT_HEADER: &str = "## 变体侧重";

// ---- 断言 1：变体名两两不同；`from_name` 大小写 / 分隔符等价 ----

#[test]
fn variant_names_are_pairwise_distinct() {
    let mut names: Vec<&str> = PromptVariant::ALL.iter().map(|v| v.name()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "四个变体的 name() 必须两两不同");
    assert_eq!(
        PromptVariant::ALL.len(),
        4,
        "PromptVariant::ALL 必须是四个变体"
    );
}

#[test]
fn from_name_normalizes_case_and_separators() {
    for (input, expected) in [
        ("Code", PromptVariant::Code),
        ("code", PromptVariant::Code),
        ("co_de", PromptVariant::Code),
        ("CO-DE", PromptVariant::Code),
        ("Coding", PromptVariant::Coding),
        ("cod_ing", PromptVariant::Coding),
        ("Design", PromptVariant::Design),
        ("de_sign", PromptVariant::Design),
        ("Default", PromptVariant::Default),
        ("de_fault", PromptVariant::Default),
    ] {
        assert_eq!(
            PromptVariant::from_name(input),
            expected,
            "from_name({input:?}) 应归一为 {expected:?}"
        );
    }
}

// ---- 断言 2：未知值回落 Default，不 panic ----

#[test]
fn unknown_names_fall_back_to_default_without_panicking() {
    for input in ["", "   ", "nope", "设计", "code++", "codingg", "💥"] {
        assert_eq!(
            PromptVariant::from_name(input),
            PromptVariant::Default,
            "未知变体名 {input:?} 应回落 Default 而非 panic"
        );
    }
}

// ---- 断言 3：回落可观测（段序列相等） ----

/// 模式 × 变体 → 期望生效的变体（本卡回落表的可执行形态）。
///
/// | 模式 | Default | Code | Coding | Design |
/// |---|---|---|---|---|
/// | Craft | Default | Code | Coding | Design |
/// | Plan | Default | Code | Code | Default |
/// | Expert | Default | Code | Code | Default |
/// | Ask / Quick | Default | Default | Default | Default |
fn expected_variant(mode: PromptMode, variant: PromptVariant) -> PromptVariant {
    match (mode, variant) {
        (_, PromptVariant::Default) => PromptVariant::Default,
        (PromptMode::Craft, other) => other,
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Code) => PromptVariant::Code,
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Coding) => PromptVariant::Code,
        (PromptMode::Plan | PromptMode::Expert, PromptVariant::Design) => PromptVariant::Default,
        (PromptMode::Ask | PromptMode::Quick, _) => PromptVariant::Default,
    }
}

#[test]
fn fallback_is_observable_on_the_returned_template() {
    for mode in MODES {
        for variant in PromptVariant::ALL {
            let expected = expected_variant(mode, variant);
            let got = template_for_variant(mode, variant);
            let target = template_for_variant(mode, expected);
            assert_eq!(
                got.variant(),
                expected,
                "{mode:?} 的 {variant:?} 回落后 variant() 应为 {expected:?}"
            );
            assert_eq!(
                got.segments(),
                target.segments(),
                "{mode:?} 的 {variant:?} 回落后段序列必须与 {expected:?} 变体相等"
            );
        }
    }
}

#[test]
fn ask_design_falls_back_to_default() {
    let fell_back = template_for_variant(PromptMode::Ask, PromptVariant::Design);
    let target = template_for_variant(PromptMode::Ask, PromptVariant::Default);
    assert_eq!(
        fell_back.segments(),
        target.segments(),
        "ask 的设计类必须回落到默认变体（段序列相等）"
    );
    assert_eq!(fell_back.variant(), PromptVariant::Default);
}

// ---- 断言 4：plan / expert 的编码类回落代码类；quick 三变体全回落默认 ----

#[test]
fn plan_and_expert_coding_falls_back_to_code() {
    for mode in [PromptMode::Plan, PromptMode::Expert] {
        let coding = template_for_variant(mode, PromptVariant::Coding);
        let code = template_for_variant(mode, PromptVariant::Code);
        assert_eq!(
            coding.segments(),
            code.segments(),
            "{mode:?} 的编码类应回落代码类"
        );
        assert_eq!(coding.variant(), PromptVariant::Code);
    }
}

#[test]
fn quick_falls_back_to_default_for_all_variants() {
    let base = template_for_variant(PromptMode::Quick, PromptVariant::Default);
    for variant in [
        PromptVariant::Code,
        PromptVariant::Coding,
        PromptVariant::Design,
    ] {
        let got = template_for_variant(PromptMode::Quick, variant);
        assert_eq!(
            got.segments(),
            base.segments(),
            "quick 的 {variant:?} 应回落默认"
        );
        assert_eq!(got.variant(), PromptVariant::Default);
    }
}

// ---- 断言 5：craft 三变体互不相同，且都包含 craft 默认段集的全部段 ----

#[test]
fn craft_variants_are_distinct_and_superset_of_default_segments() {
    let base = template_for(PromptMode::Craft);
    let code = template_for_variant(PromptMode::Craft, PromptVariant::Code);
    let coding = template_for_variant(PromptMode::Craft, PromptVariant::Coding);
    let design = template_for_variant(PromptMode::Craft, PromptVariant::Design);

    assert_ne!(code.segments(), coding.segments(), "代码类与编码类必须不同");
    assert_ne!(code.segments(), design.segments(), "代码类与设计类必须不同");
    assert_ne!(
        coding.segments(),
        design.segments(),
        "编码类与设计类必须不同"
    );
    assert_ne!(base.segments(), code.segments(), "代码类必须与默认段集不同");

    for (variant, template) in [
        (PromptVariant::Code, &code),
        (PromptVariant::Coding, &coding),
        (PromptVariant::Design, &design),
    ] {
        assert!(
            is_subsequence(base.segments(), template.segments()),
            "craft 的 {variant:?} 必须包含默认段集的全部段（子序列判定）"
        );
        assert_eq!(template.variant(), variant);
    }
}

// ---- 断言 6：变体正文不含工具名 / 权限叙述 / 模式名 ----

#[test]
fn variant_bodies_are_orthogonal_to_tools_permissions_and_modes() {
    // 只判**实际生效**的变体：plan / expert 的编码类回落代码类、设计类回落默认，
    // 两者的正文与代码类 / 默认段集逐字相同（回落可观测），不必重复判定。
    let cases = [
        (PromptMode::Craft, PromptVariant::Code),
        (PromptMode::Craft, PromptVariant::Coding),
        (PromptMode::Craft, PromptVariant::Design),
        (PromptMode::Plan, PromptVariant::Code),
        (PromptMode::Expert, PromptVariant::Code),
    ];
    for (mode, variant) in cases {
        let body = variant_body(&template_for_variant(mode, variant))
            .unwrap_or_else(|| panic!("{mode:?} 的 {variant:?} 应带变体段"));
        let lower = body.to_lowercase();
        for tool in TOOL_NAMES {
            assert!(
                !lower.contains(tool),
                "{mode:?} 的 {variant:?} 正文含工具名 {tool}：{body}"
            );
        }
        for word in PERMISSION_WORDS {
            assert!(
                !lower.contains(word),
                "{mode:?} 的 {variant:?} 正文含权限叙述 {word}：{body}"
            );
        }
        for mode_name in MODE_NAMES {
            assert!(
                !lower.contains(mode_name),
                "{mode:?} 的 {variant:?} 正文含模式名 {mode_name}：{body}"
            );
        }
        let lines = body.lines().count();
        assert!(lines <= 12, "{variant:?} 正文 {lines} 行，超过 12 行上限");
    }
}

// ---- 断言 7：旧签名 `build` 与新签名 `build_with` 等价 ----

#[test]
fn legacy_build_matches_build_with_default_variant() {
    let vars = sample_vars();
    for mode in MODES {
        assert_eq!(
            Composer::build(mode, &vars),
            Composer::build_with(mode, PromptVariant::Default, &vars),
            "{mode:?}：旧签名 build 必须等于 build_with(Default)"
        );
    }
}

// ---- 断言 8：渲染确定性（逐字节相同） ----

#[test]
fn build_with_is_deterministic() {
    let vars = sample_vars();
    for mode in MODES {
        for variant in PromptVariant::ALL {
            let first = Composer::build_with(mode, variant, &vars);
            let second = Composer::build_with(mode, variant, &vars);
            assert_eq!(
                first, second,
                "{mode:?} 的 {variant:?} 两次渲染必须逐字节相同"
            );
        }
    }
}

/// 取模板里「变体侧重」段的正文（常量本体的逐字内容）。
fn variant_body(template: &PromptTemplate) -> Option<String> {
    template
        .segments()
        .iter()
        .find_map(|segment| match segment {
            Segment::Text(body) if body.starts_with(VARIANT_HEADER) => Some(body.clone()),
            _ => None,
        })
}

/// 子序列判定：`needle` 是 `haystack` 的子序列（不要求连续）。
fn is_subsequence(needle: &[Segment], haystack: &[Segment]) -> bool {
    let mut cursor = haystack.iter();
    needle
        .iter()
        .all(|wanted| cursor.find(|candidate| *candidate == wanted).is_some())
}

/// 最小可用变量集（只填必填字段，其余留空 ⇒ 整段省略）。
fn sample_vars() -> PromptVars {
    PromptVars {
        ai_name: "小澜".into(),
        user_name: "Kim".into(),
        workspace: "/work/demo".into(),
        date: "2026-05-06".into(),
        ..PromptVars::default()
    }
}
