//! 负例（上）：§17 编号 01–13 —— 清单 / 结构 / 命名 / 工具声明 / 提示词 / 插件 / 技能。
//!
//! 每条对应规格 §17 的一个校验项编号，断言「报出/不报出」与严重级别。
use super::support::*;

// ---------------------------------------------------------------------------
// 负例：逐条 §17 编号
// ---------------------------------------------------------------------------

#[test]
fn item_01_missing_manifest_is_reported() {
    let temp = TempDir::new("i01");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join(".lengsuo-plugin/plugin.json")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::MANIFEST_PRESENT,
        "PLUGIN_MANIFEST_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_02_broken_manifest_is_reported() {
    let temp = TempDir::new("i02");
    let manifest = build_single(temp.path());
    fs::write(
        temp.path().join(".lengsuo-plugin/plugin.json"),
        "{ not json",
    )
    .expect("应可写");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::MANIFEST_PARSES,
        "MANIFEST_PARSE_FAILED",
        Severity::Error,
    );
}

#[test]
fn item_03a_non_kebab_name_is_reported() {
    let temp = TempDir::new("i03a");
    let patched = patch(SENIOR_PLUGIN, "name", serde_json::json!("Senior_Developer"));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::NAME_KEBAB_CASE,
        "INVALID_NAME",
        Severity::Error,
    );
}

#[test]
fn item_03b_dir_name_mismatch_is_warning_by_default_and_error_when_strict() {
    let temp = TempDir::new("i03b");
    let manifest = build_single(temp.path());
    // 默认：Warning（真实样本形态）。
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::NAME_MATCHES_DIR,
        "NAME_DIR_MISMATCH",
        Severity::Warning,
    );
    // 严格口径：Error。
    let strict = Validator::with_rules(RuleSet::new().strict_dir_name(true));
    let report = strict.report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::NAME_MATCHES_DIR,
        "NAME_DIR_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_04_invalid_version_is_reported() {
    let temp = TempDir::new("i04");
    let patched = patch(SENIOR_PLUGIN, "version", serde_json::json!("1.1"));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::VERSION_SEMVER,
        "INVALID_VERSION",
        Severity::Error,
    );
}

#[test]
fn item_05_type_structure_mismatch_is_reported() {
    let temp = TempDir::new("i05");
    // agent 型却带 teamInfo ⇒ 结构与声明不一致。
    let patched = patch(
        SENIOR_PLUGIN,
        "teamInfo",
        serde_json::json!({"leadAgent": "senior-developer", "memberAgents": []}),
    );
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::TYPE_MATCHES_STRUCTURE,
        "TYPE_STRUCTURE_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_06_agent_name_mismatch_is_reported() {
    let temp = TempDir::new("i06");
    let patched = patch(SENIOR_PLUGIN, "agentName", serde_json::json!("other-agent"));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AGENT_NAME_MATCHES_STEM,
        "AGENT_NAME_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_07_missing_agent_file_is_reported() {
    let temp = TempDir::new("i07");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join("agents/senior-developer.md")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AGENT_PATHS,
        "AGENT_PATH_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_07_path_escape_is_reported() {
    let temp = TempDir::new("i07e");
    let patched = patch(
        SENIOR_PLUGIN,
        "agents",
        serde_json::json!(["../outside/senior-developer.md"]),
    );
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(&report, item::AGENT_PATHS, "PATH_ESCAPE", Severity::Error);
}

#[test]
fn item_08_forbidden_tools_declaration_is_reported() {
    let temp = TempDir::new("i08");
    let manifest = build_single(temp.path());
    fs::write(
        temp.path().join("agents/senior-developer.md"),
        "---\nname: senior-developer\ntools:\n  - Bash\n---\n\n正文\n",
    )
    .expect("应可写");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::NO_TOOLS_DECLARATION,
        "FORBIDDEN_TOOL_DECLARATION",
        Severity::Error,
    );
}

#[test]
fn item_09_frontmatter_name_mismatch_is_reported() {
    let temp = TempDir::new("i09");
    let manifest = build_single(temp.path());
    fs::write(
        temp.path().join("agents/senior-developer.md"),
        "---\nname: other-name\n---\n\n正文\n",
    )
    .expect("应可写");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::FRONTMATTER_NAME_MATCHES_STEM,
        "AGENT_FRONTMATTER_NAME_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_10_tag_count_is_reported() {
    let temp = TempDir::new("i10a");
    let mut tags: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(SENIOR_PLUGIN)
        .expect("应可解析")
        .get("tags")
        .cloned()
        .expect("应有 tags")
        .as_array()
        .cloned()
        .expect("tags 应是数组");
    tags.pop();
    let patched = patch(SENIOR_PLUGIN, "tags", serde_json::json!(tags));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(&report, item::TAG_COUNT, "TAG_COUNT", Severity::Error);
}

#[test]
fn item_10_quick_prompt_count_is_reported() {
    let temp = TempDir::new("i10b");
    let mut prompts: Vec<serde_json::Value> =
        serde_json::from_str::<serde_json::Value>(SENIOR_PLUGIN)
            .expect("应可解析")
            .get("quickPrompts")
            .cloned()
            .expect("应有 quickPrompts")
            .as_array()
            .cloned()
            .expect("quickPrompts 应是数组");
    prompts.pop();
    let patched = patch(SENIOR_PLUGIN, "quickPrompts", serde_json::json!(prompts));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::QUICK_PROMPT_COUNT,
        "QUICK_PROMPT_COUNT",
        Severity::Error,
    );
}

#[test]
fn item_11_init_prompt_mismatch_is_reported() {
    let temp = TempDir::new("i11");
    let patched = patch(
        SENIOR_PLUGIN,
        "defaultInitPrompt",
        serde_json::json!({"zh": "换个开场白", "en": "another opener"}),
    );
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::INIT_PROMPT_MATCHES,
        "INIT_PROMPT_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_12_description_length_is_reported() {
    let temp = TempDir::new("i12");
    let patched = patch(
        SENIOR_PLUGIN,
        "displayDescription",
        serde_json::json!({"zh": "太短", "en": "too short"}),
    );
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::DESCRIPTION_LENGTH,
        "DESCRIPTION_LENGTH",
        Severity::Warning,
    );
}

#[test]
fn item_13_plugin_name_mismatch_is_reported() {
    let temp = TempDir::new("i13");
    let patched = patch(SENIOR_PLUGIN, "plugin", serde_json::json!("other-plugin"));
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::PLUGIN_NAME_MATCHES,
        "PLUGIN_NAME_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_13_absent_plugin_key_is_not_reported() {
    // 单专家样本无 `plugin` 键 ⇒ 结构层放宽为 Option ⇒ 不报缺失。
    let temp = TempDir::new("i13b");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::PLUGIN_NAME_MATCHES).is_none());
}
