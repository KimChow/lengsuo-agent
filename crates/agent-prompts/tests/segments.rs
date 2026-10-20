//! 条件段机制的断言（07 册 §5 条件段实录）。
//!
//! 覆盖：`Cond` 四形态各一条、判定不满足 ⇒ 整段省略、判定满足 ⇒ 出现且内容正确、
//! 「或」组合、条件判定是纯函数（无 IO、无时钟、同输入同输出）。

mod common;

use agent_prompts::{Cond, Composer, PromptMode, PromptTemplate, Segment, WORK_MODE_KEY};
use common::{bare_vars, full_vars, has_all, has_none};

/// 构造一个只含单个条件段的模板。
fn cond_template(mode: PromptMode, cond: Cond, body: &str) -> PromptTemplate {
    PromptTemplate::new(
        mode,
        vec![Segment::If(cond, vec![Segment::Text(body.to_string())])],
    )
}

// ---- Cond 四形态 ----

#[test]
fn cond_true_reads_boolean_variable() {
    let mut vars = bare_vars();
    vars.is_windows = true;
    let tpl = cond_template(PromptMode::Craft, Cond::True("is_windows"), "WIN");
    assert_eq!(tpl.render(&vars), "WIN\n");

    vars.is_windows = false;
    assert_eq!(tpl.render(&vars), "\n", "布尔为假 ⇒ 整段省略");
}

#[test]
fn cond_eq_compares_variable_to_literal() {
    let mut vars = bare_vars();
    vars.response_language = "中文".into();
    let tpl = cond_template(PromptMode::Craft, Cond::Eq("response_language", "中文"), "EQ");
    assert_eq!(tpl.render(&vars), "EQ\n");

    vars.response_language = "English".into();
    assert_eq!(tpl.render(&vars), "\n");
}

#[test]
fn cond_eq_on_work_mode_compares_template_mode() {
    // `work_mode` 是保留键：值取自模板自身模式，不进 PromptVars。
    let tpl = cond_template(PromptMode::Ask, Cond::Eq(WORK_MODE_KEY, "ask"), "ASK");
    assert_eq!(tpl.render(&bare_vars()), "ASK\n");

    let tpl = cond_template(PromptMode::Craft, Cond::Eq(WORK_MODE_KEY, "ask"), "ASK");
    assert_eq!(tpl.render(&bare_vars()), "\n", "模式不是 ask ⇒ 整段省略");
}

#[test]
fn cond_any_non_empty_holds_when_any_key_filled() {
    let tpl = cond_template(
        PromptMode::Craft,
        Cond::AnyNonEmpty(&["memory_user", "memory_project"]),
        "MEM",
    );
    // 任一非空即成立（07 册 §5 的 `a or b or c` 形态）。
    let mut vars = bare_vars();
    vars.memory_user = "用户偏好".into();
    assert_eq!(tpl.render(&vars), "MEM\n");

    let mut vars = bare_vars();
    vars.memory_project = "项目约定".into();
    assert_eq!(tpl.render(&vars), "MEM\n");

    assert_eq!(tpl.render(&bare_vars()), "\n", "全空 ⇒ 整段省略");
}

#[test]
fn cond_any_non_empty_treats_whitespace_as_empty() {
    let tpl = cond_template(PromptMode::Craft, Cond::AnyNonEmpty(&["memory_user"]), "MEM");
    let mut vars = bare_vars();
    vars.memory_user = "   \n\t ".into();
    assert_eq!(tpl.render(&vars), "\n", "纯空白视为空");
}

#[test]
fn cond_contains_matches_substring() {
    let mut vars = bare_vars();
    vars.response_language = "简体中文".into();
    let tpl = cond_template(PromptMode::Craft, Cond::Contains("response_language", "中文"), "ZH");
    assert_eq!(tpl.render(&vars), "ZH\n");

    vars.response_language = "English".into();
    assert_eq!(tpl.render(&vars), "\n");
}

#[test]
fn cond_any_combines_conditions_with_or() {
    let cond = Cond::Any(&[
        Cond::Eq(WORK_MODE_KEY, "craft"),
        Cond::Eq(WORK_MODE_KEY, "expert"),
    ]);
    assert_eq!(
        cond_template(PromptMode::Craft, cond, "HIT").render(&bare_vars()),
        "HIT\n"
    );
    assert_eq!(
        cond_template(PromptMode::Expert, cond, "HIT").render(&bare_vars()),
        "HIT\n"
    );
    assert_eq!(
        cond_template(PromptMode::Quick, cond, "HIT").render(&bare_vars()),
        "\n"
    );
}

// ---- 条件段的渲染语义 ----

#[test]
fn unsatisfied_condition_omits_whole_section() {
    let tpl = PromptTemplate::new(
        PromptMode::Craft,
        vec![
            Segment::Text("前段".into()),
            Segment::If(
                Cond::True("feature_expert_management"),
                vec![Segment::Text("条件段".into())],
            ),
            Segment::Text("后段".into()),
        ],
    );
    let mut vars = bare_vars();
    vars.feature_expert_management = false;
    let out = tpl.render(&vars);
    has_none(&out, &["条件段"]);
    has_all(&out, &["前段", "后段"]);
    // 省略段不留空洞、不留多余空行。
    assert!(!out.contains("\n\n\n"), "省略段留下连续空行\n{out}");
}

#[test]
fn satisfied_condition_renders_inner_content() {
    let tpl = PromptTemplate::new(
        PromptMode::Craft,
        vec![Segment::If(
            Cond::True("feature_expert_management"),
            vec![Segment::Text("条件段正文".into())],
        )],
    );
    let mut vars = bare_vars();
    vars.feature_expert_management = true;
    assert_eq!(tpl.render(&vars), "条件段正文\n");
}

#[test]
fn condition_is_pure_function_of_vars() {
    // 同一份变量重复判定 ⇒ 同一结论（无时钟、无 IO、无随机）。
    let cond = Cond::Any(&[
        Cond::True("is_windows"),
        Cond::AnyNonEmpty(&["memory_user", "memory_project"]),
    ]);
    let vars = full_vars();
    let first = cond.holds(&vars, PromptMode::Craft);
    for _ in 0..8 {
        assert_eq!(cond.holds(&vars, PromptMode::Craft), first);
    }
    assert!(first, "full_vars 的 is_windows 为真 ⇒ 条件成立");
}

#[test]
fn nested_conditions_are_evaluated_in_order() {
    // 外层不成立 ⇒ 内层不评估；外层成立 ⇒ 内层按自己的条件决定。
    let tpl = PromptTemplate::new(
        PromptMode::Craft,
        vec![Segment::If(
            Cond::True("feature_expert_management"),
            vec![Segment::If(
                Cond::True("is_windows"),
                vec![Segment::Text("双层命中".into())],
            )],
        )],
    );
    let mut vars = bare_vars();
    vars.feature_expert_management = true;
    vars.is_windows = true;
    assert_eq!(tpl.render(&vars), "双层命中\n");

    vars.is_windows = false;
    assert_eq!(tpl.render(&vars), "\n");

    vars.feature_expert_management = false;
    vars.is_windows = true;
    assert_eq!(tpl.render(&vars), "\n");
}

// ---- 模板侧的条件段接线 ----

#[test]
fn windows_command_safety_only_when_is_windows() {
    // `windows_command_safety` 是 is_windows 条件段（07 册 §5 的 `IsWindows`）。
    let mut vars = full_vars();
    vars.is_windows = true;
    let on = Composer::build(PromptMode::Craft, &vars);
    has_all(&on, &["## Windows 命令安全"]);

    vars.is_windows = false;
    let off = Composer::build(PromptMode::Craft, &vars);
    has_none(&off, &["## Windows 命令安全"]);
}

#[test]
fn response_language_only_when_language_contains_chinese() {
    let mut vars = full_vars();
    vars.response_language = "简体中文".into();
    has_all(
        &Composer::build(PromptMode::Craft, &vars),
        &["## 回复语言"],
    );

    vars.response_language = "English".into();
    has_none(
        &Composer::build(PromptMode::Craft, &vars),
        &["## 回复语言"],
    );
}

#[test]
fn expert_management_only_when_feature_enabled() {
    let mut vars = full_vars();
    vars.feature_expert_management = true;
    has_all(
        &Composer::build(PromptMode::Craft, &vars),
        &["## 专家管理"],
    );

    vars.feature_expert_management = false;
    has_none(
        &Composer::build(PromptMode::Craft, &vars),
        &["## 专家管理"],
    );
}

#[test]
fn expert_persona_only_when_plugin_prompt_filled() {
    let mut vars = full_vars();
    vars.plugin_agent_prompt = "PLUGIN-代码专家".into();
    has_all(
        &Composer::build(PromptMode::Expert, &vars),
        &["## 专家人格\nPLUGIN-代码专家"],
    );

    vars.plugin_agent_prompt = String::new();
    has_none(
        &Composer::build(PromptMode::Expert, &vars),
        &["## 专家人格"],
    );
}

/// 条件段判定不引入 IO 也不引入模型调用：判定只看变量值与模板模式。
#[test]
fn condition_never_reads_outside_vars_and_mode() {
    // 未在 PromptVars 中登记的键一律不成立（不做环境变量/文件回退）。
    let cond = Cond::True("some_undeclared_key");
    assert!(!cond.holds(&full_vars(), PromptMode::Craft));
    assert!(!Cond::Eq("some_undeclared_key", "x").holds(&full_vars(), PromptMode::Craft));
    assert!(!Cond::Contains("some_undeclared_key", "x").holds(&full_vars(), PromptMode::Craft));
    assert!(!Cond::AnyNonEmpty(&["some_undeclared_key"]).holds(&full_vars(), PromptMode::Craft));
}

/// 条件段的省略与既有空值省略语义一致：不留标题、不留空行。
#[test]
fn omitted_condition_section_leaves_no_hollow_mark() {
    let mut vars = full_vars();
    vars.is_windows = false;
    vars.response_language = "English".into();
    vars.feature_expert_management = false;
    let out = Composer::build(PromptMode::Craft, &vars);
    assert!(!out.contains("## Windows 命令安全"));
    assert!(!out.contains("## 回复语言"));
    assert!(!out.contains("## 专家管理"));
    // 段间只允许一个空行。
    let mut prev_blank = false;
    for line in out.lines() {
        let blank = line.trim().is_empty();
        assert!(!(blank && prev_blank), "出现连续空行\n{out}");
        prev_blank = blank;
    }
}
