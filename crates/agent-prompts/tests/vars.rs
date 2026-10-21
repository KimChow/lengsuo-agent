//! 变量注入、可省略段、用户规则追加、确定性与公开模板 API 的断言。

mod common;

use agent_prompts::{Composer, PromptMode, PromptTemplate, PromptVars, Segment, PROMPT_VAR_KEYS};
use common::{assert_no_empty_section, bare_vars, full_vars, has_all, ALL_MODES, MEMORY_TITLES};

// ---- 变量替换正确 ----

#[test]
fn variables_are_substituted_without_leftover_placeholders() {
    for mode in ALL_MODES {
        let out = Composer::build(mode, &full_vars());
        assert!(!out.contains("{{"), "{mode:?} 残留 {{ 占位符");
        assert!(!out.contains("}}"), "{mode:?} 残留 }} 占位符");
    }
}

/// 每个可注入键都必须有取值口：`get()` 逐键返回 `Some`，不漏键。
#[test]
fn every_declared_key_is_resolvable() {
    let vars = full_vars();
    for key in PROMPT_VAR_KEYS {
        assert!(vars.get(key).is_some(), "键 {key} 未在 get() 中接线");
    }
    // 布尔字段以文本形态注入。
    assert_eq!(vars.get("is_windows"), Some("true"));
    assert_eq!(vars.get("feature_browser_use"), Some("true"));
    assert_eq!(vars.get("feature_computer_use"), Some("true"));
    assert_eq!(vars.get("feature_expert_management"), Some("true"));
    let off = bare_vars();
    assert_eq!(off.get("is_windows"), Some("false"));
    assert_eq!(off.get("feature_browser_use"), Some("false"));
    assert_eq!(off.get("feature_computer_use"), Some("false"));
    assert_eq!(off.get("feature_expert_management"), Some("false"));
    // 未知键仍返回 None（占位符原样保留的语义不变）。
    assert_eq!(vars.get("no_such_key"), None);
}

/// 身份注入的 6 个新键：既在 `PROMPT_VAR_KEYS` 全集里，也有 `get()` 取值口。
#[test]
fn identity_keys_are_declared_and_resolvable() {
    let vars = full_vars();
    let expected = [
        ("user", "USER-用户档案：偏好中文提交信息"),
        ("identity_mode", "onboarding"),
        ("soul_path", "/data/user-1/SOUL.md"),
        ("identity_path", "/data/user-1/IDENTITY.md"),
        ("user_path", "/data/user-1/USER.md"),
        ("bootstrap_path", "/data/user-1/BOOTSTRAP.md"),
    ];
    for (key, value) in expected {
        assert!(
            PROMPT_VAR_KEYS.contains(&key),
            "身份键 {key} 未登记进 PROMPT_VAR_KEYS"
        );
        assert_eq!(vars.get(key), Some(value), "键 {key} 取值口不对");
    }
    // 定长键表长度随字段同步扩展（漏加键 ⇒ 「每个声明键都可解析」先红）。
    assert_eq!(PROMPT_VAR_KEYS.len(), 31);
    // 默认值：`identity_mode` 空串（不默认 onboarding，分支由调用方显式声明）。
    assert_eq!(PromptVars::default().get("identity_mode"), Some(""));
}

/// `{{key}}` 文本段里每个已声明键都能被替换（不留花括号）。
#[test]
fn declared_keys_are_substitutable_in_text_segments() {
    let vars = full_vars();
    for key in PROMPT_VAR_KEYS {
        let tpl = PromptTemplate::new(
            PromptMode::Craft,
            vec![Segment::Text(format!("[{{{{{key}}}}}]"))],
        );
        let out = tpl.render(&vars);
        assert!(!out.contains("{{"), "键 {key} 未被替换\n{out}");
        assert_eq!(out, format!("[{}]\n", vars.get(key).unwrap()));
    }
}

#[test]
fn variable_positions_match_expected_lines() {
    let out = Composer::build(PromptMode::Craft, &full_vars());
    assert!(out.contains("你是 LAN-XIAO"));
    assert!(out.contains("本次服务对象：KIM-USR。"));
    assert!(out.contains("人格设定：\nSOUL-冷静直接先验证\n"));
    assert!(out.contains("- 日期：DATE-2026-05-06"));
    assert!(out.contains("- 工作区：WS-/home/kim/demo"));
}

#[test]
fn variable_values_are_not_rescanned_for_placeholders() {
    let mut vars = bare_vars();
    vars.soul = "{{workspace}}".into(); // 注入值里的花括号原样透传，不二次替换
    let out = Composer::build(PromptMode::Quick, &vars);
    assert!(out.contains("人格设定：\n{{workspace}}\n"));
    assert!(
        out.contains("- 工作区：/home/kim/work/demo"),
        "真实变量仍应正常注入"
    );
}

// ---- 用户自定义规则追加 ----

#[test]
fn user_rules_appended_when_non_empty() {
    let out = Composer::build(PromptMode::Craft, &full_vars());
    assert!(
        out.contains("## 用户自定义规则\nRULE-回复不超过三行"),
        "缺用户规则段"
    );
    // 规格 §12 第 8 项：用户规则在硬规则之前，硬规则固定为最后一段。
    let hard_at = out.find("## 硬规则").unwrap();
    let rules_at = out.find("## 用户自定义规则").unwrap();
    assert!(
        rules_at < hard_at,
        "用户规则应在硬规则之前（规格 §12 第 8 项）"
    );
}

#[test]
fn user_rules_section_absent_when_empty() {
    for mode in ALL_MODES {
        let out = Composer::build(mode, &bare_vars());
        assert!(!out.contains("用户自定义规则"), "{mode:?} 空规则仍留段");
    }
}

// ---- 记忆四段：独立成段、空段省略 ----

/// 记忆段的可辨识整块（标题 + 标记行）：标题可能与公共头重名（如「## 身份」），
/// 故断言一律以「标题 + 标记行」整块为单位。
fn memory_block(title: &str, marker: &str) -> String {
    format!("{title}\n{marker}")
}

/// 只把第 `index` 段设为非空，其余三段留空。
fn vars_with_only(index: usize) -> PromptVars {
    let mut vars = bare_vars();
    let marker = format!("ONLY-段{index}-内容");
    match index {
        0 => vars.memory_identity = marker,
        1 => vars.memory_user = marker,
        2 => vars.memory_project = marker,
        3 => vars.memory_daily = marker,
        _ => unreachable!("记忆段下标越界"),
    }
    vars
}

#[test]
fn each_memory_slot_renders_alone() {
    for (index, title) in MEMORY_TITLES.iter().enumerate() {
        let marker = format!("ONLY-段{index}-内容");
        let out = Composer::build(PromptMode::Craft, &vars_with_only(index));
        assert!(
            out.contains(&memory_block(title, &marker)),
            "第 {index} 段未单独成段\n{out}"
        );
        for (other_index, other_title) in MEMORY_TITLES.iter().enumerate() {
            if other_index == index {
                continue;
            }
            let other_marker = format!("ONLY-段{other_index}-内容");
            assert!(
                !out.contains(&memory_block(other_title, &other_marker)),
                "第 {index} 段非空时不应出现 {other_title} 子块\n{out}"
            );
        }
        assert_no_empty_section(&out);
    }
}

#[test]
fn all_four_memory_slots_render_in_order() {
    let out = Composer::build(PromptMode::Craft, &full_vars());
    has_all(
        &out,
        &[
            "## 身份\nMEMID-身份层：小澜",
            "## 用户记忆\nMEMUSR-用户偏好中文提交信息",
            "## 项目记忆\nMEM-项目：提示词引擎；决定：plan 复用 craft 模板",
            "## 当日日志\nMEMDAY-当日：完成 PromptVars 扩容",
        ],
    );
    let mut previous = 0;
    for (index, title) in MEMORY_TITLES.iter().enumerate() {
        let at = out.find(title).unwrap_or_else(|| panic!("缺 {title}"));
        if index > 0 {
            assert!(at > previous, "{title} 顺序错乱\n{out}");
        }
        previous = at;
    }
    assert_no_empty_section(&out);
}

#[test]
fn all_four_memory_slots_empty_leaves_no_memory_titles() {
    let empty_markers: Vec<String> = (0..MEMORY_TITLES.len())
        .map(|index| memory_block(MEMORY_TITLES[index], &format!("ONLY-段{index}-内容")))
        .collect();
    for mode in ALL_MODES {
        let out = Composer::build(mode, &bare_vars());
        for (index, title) in MEMORY_TITLES.iter().enumerate() {
            // 「## 身份」与公共头身份段同名：以「该标题紧跟本段独有标记」判定，
            // 四段全空时不存在任何子块。
            assert!(
                !out.contains(&empty_markers[index]),
                "{mode:?} 空记忆段仍留子块 {title}"
            );
        }
        assert!(
            !out.contains("## 用户记忆")
                && !out.contains("## 项目记忆")
                && !out.contains("## 当日日志"),
            "{mode:?} 空记忆段仍留标题\n{out}"
        );
        assert!(!out.contains("记忆摘要"), "{mode:?} 仍残留单段摘要标题");
        assert_no_empty_section(&out);
    }
}

#[test]
fn whitespace_only_memory_slot_is_treated_as_empty() {
    let mut vars = bare_vars();
    vars.memory_identity = "   \n\t  \n".into();
    vars.memory_project = "\n \n".into();
    let out = Composer::build(PromptMode::Craft, &vars);
    assert!(
        !out.contains("## 身份\n   ") && !out.contains("## 身份\n\n"),
        "纯空白身份记忆段未视为空\n{out}"
    );
    assert!(!out.contains("## 项目记忆"), "纯空白项目记忆段未视为空");
    assert_no_empty_section(&out);
    // 公共头的「## 身份」仍在，但其下一行是身份模板行而非空白。
    assert!(out.contains("## 身份\n你是 小澜"));
}

#[test]
fn memory_segments_keep_their_own_text() {
    let mut vars = bare_vars();
    vars.memory_user = "用户偏好：\n- 中文提交信息".into();
    vars.memory_daily = "当日日志：\n- 完成四段拆分".into();
    let out = Composer::build(PromptMode::Ask, &vars);
    has_all(
        &out,
        &[
            "## 用户记忆\n用户偏好：\n- 中文提交信息",
            "## 当日日志\n当日日志：\n- 完成四段拆分",
        ],
    );
    assert!(!out.contains("## 项目记忆"), "空项目段不应出现");
    assert!(
        out.contains("## 身份\n你是 小澜"),
        "空身份记忆段不应覆盖公共头身份段"
    );
    assert_no_empty_section(&out);
}

// ---- 确定性 ----

#[test]
fn build_is_deterministic_for_same_input() {
    for mode in ALL_MODES {
        let a = Composer::build(mode, &full_vars());
        let b = Composer::build(mode, &full_vars());
        assert_eq!(a, b, "{mode:?} 两次 build 不一致");
        let c = Composer::build(mode, &bare_vars());
        let d = Composer::build(mode, &bare_vars());
        assert_eq!(c, d, "{mode:?} 空变量两次 build 不一致");
    }
}

#[test]
fn no_clock_injection_date_comes_only_from_vars() {
    let mut vars = full_vars();
    vars.date = "DATE-FROZEN".into();
    let out = Composer::build(PromptMode::Craft, &vars);
    assert!(out.contains("- 日期：DATE-FROZEN"));
    // 只改 date，其余结构不变：差异应局限在该行
    let mut vars2 = vars.clone();
    vars2.date = "DATE-OTHER".into();
    let out2 = Composer::build(PromptMode::Craft, &vars2);
    let diff: Vec<&str> = out
        .lines()
        .zip(out2.lines())
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a)
        .collect();
    assert_eq!(diff, vec!["- 日期：DATE-FROZEN"]);
}

// ---- 公开模板 API：段序列可直接构造 ----

#[test]
fn template_segments_are_public_and_renderable() {
    let tpl = PromptTemplate::new(
        PromptMode::Craft,
        vec![
            Segment::Text("# 自定义头 · {{ai_name}}\n- 工作区：{{workspace}}".into()),
            Segment::Var("soul"),
            Segment::Var("memory_project"),
            Segment::Var("date"),        // 普通变量作为独立段时输出裸值
            Segment::Var("unknown_key"), // 未知键：整段省略
            Segment::Gate,
            Segment::Var("user_rules"),
        ],
    );
    assert_eq!(tpl.mode(), PromptMode::Craft);
    assert_eq!(tpl.segments().len(), 7);

    let mut vars = bare_vars();
    vars.memory_project = "项目记忆正文".into();
    let out = tpl.render(&vars);
    assert_eq!(
        out,
        "# 自定义头 · 小澜\n- 工作区：/home/kim/work/demo\n\n\
         人格设定：\n冷静、直接、先验证再下结论。\n\n\
         ## 项目记忆\n项目记忆正文\n\n\
         2026-05-06\n\n\
         ## 规划门禁（plan）\n\
         - 当前处于规划状态：先给出计划（目标、步骤、影响文件、验证方式、风险），等待用户批准。\n\
         - 批准之前不得承诺或暗示任何写操作已经完成，也不得发起写入、删除、命令执行、外部网络或浏览器动作。\n\
         - 计划被批准后才退出规划状态，转为实际执行。\n"
    );
}

#[test]
fn gate_segment_equals_plan_gate() {
    let tpl = PromptTemplate::new(PromptMode::Plan, vec![Segment::Gate]);
    assert_eq!(
        tpl.render(&bare_vars()),
        format!("{}\n", Composer::plan_gate())
    );
    assert!(Composer::plan_gate().starts_with("## 规划门禁（plan）"));
}
