//! 负例（下）：§17 编号 15–25 —— 团队 / 头像 / 不可变字段 / 命名冲突 / 连接器 / 重名。
//!
//! 每条对应规格 §17 的一个校验项编号，断言「报出/不报出」与严重级别。
use super::support::*;

#[test]
fn item_15_team_lead_missing_is_reported() {
    let temp = TempDir::new("i15");
    let _manifest = build_team(temp.path());
    // 把主理人的 role 从 lead 改成 member ⇒ 没有 lead 了。
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    for member in value["members"].as_array_mut().expect("应是数组") {
        if member["id"] == serde_json::json!("pd-ai-quant-research-team") {
            member["role"] = serde_json::json!("member");
        }
    }
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::TEAM_LEAD_UNIQUE,
        "TEAM_LEAD_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_16_team_members_mismatch_is_reported() {
    let temp = TempDir::new("i16");
    let _manifest = build_team(temp.path());
    // memberAgents 少一个 ⇒ 与 members 集合不一致。
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    value["teamInfo"]["memberAgents"]
        .as_array_mut()
        .expect("应是数组")
        .pop();
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::TEAM_MEMBERS_MATCH,
        "TEAM_MEMBERS_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_16_lead_must_not_appear_in_member_agents() {
    let temp = TempDir::new("i16b");
    let _manifest = build_team(temp.path());
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    let lead = value["teamInfo"]["leadAgent"].clone();
    value["teamInfo"]["memberAgents"]
        .as_array_mut()
        .expect("应是数组")
        .push(lead);
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::TEAM_MEMBERS_MATCH,
        "TEAM_MEMBERS_MISMATCH",
        Severity::Error,
    );
}

#[test]
fn item_17_team_profession_mismatch_is_reported_as_warning() {
    let temp = TempDir::new("i17");
    let _manifest = build_team(temp.path());
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    value["profession"]["zh"] = serde_json::json!("另一个头衔");
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::TEAM_PROFESSION_MATCHES_DISPLAY,
        "TEAM_PROFESSION_DISPLAY_MISMATCH",
        Severity::Warning,
    );
}

#[test]
fn item_18_generic_agent_name_is_reported() {
    let temp = TempDir::new("i18");
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    value["agentName"] = serde_json::json!("team-lead");
    value["teamInfo"]["leadAgent"] = serde_json::json!("team-lead");
    value["members"][0]["id"] = serde_json::json!("team-lead");
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    // 目录需先建（原测试漏了这一步，写到不存在的路径会 ENOENT）。
    let plugin_dir = temp.path().join(".lengsuo-plugin");
    fs::create_dir_all(&plugin_dir).expect("插件目录应可创建");
    fs::write(plugin_dir.join("plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AGENT_NAME_NOT_GENERIC,
        "GENERIC_AGENT_NAME",
        Severity::Error,
    );
}

#[test]
fn item_19_skill_without_skill_md_is_reported() {
    let temp = TempDir::new("i19");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join("skills/browser-use/SKILL.md")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::SKILL_PATHS,
        "SKILL_PATH_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_19_missing_skill_dir_is_reported() {
    let temp = TempDir::new("i19b");
    let manifest = build_single(temp.path());
    fs::remove_dir_all(temp.path().join("skills/fullstack-dev")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::SKILL_PATHS,
        "SKILL_PATH_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_21_missing_avatar_is_reported() {
    let temp = TempDir::new("i21a");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join("avatars/expert.png")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AVATAR_VALID,
        "AVATAR_MISSING",
        Severity::Error,
    );
}

#[test]
fn item_21_oversized_avatar_is_reported() {
    let temp = TempDir::new("i21b");
    let manifest = build_single(temp.path());
    // 501KB > 500KB 上限。
    fs::write(
        temp.path().join("avatars/expert.png"),
        vec![0u8; 501 * 1024],
    )
    .expect("应可写");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AVATAR_VALID,
        "AVATAR_TOO_LARGE",
        Severity::Error,
    );
}

#[test]
fn item_21_wrong_avatar_dimensions_are_reported_when_strict() {
    let temp = TempDir::new("i21c");
    let manifest = build_single(temp.path());
    fs::write(temp.path().join("avatars/expert.png"), png_256()).expect("应可写");
    // 默认：§17 只列「存在 + ≤500KB」两条判据 ⇒ 不报尺寸。
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::AVATAR_VALID).is_none());
    // 严格口径（512×512，§3 目录结构）：报尺寸错。
    let strict = Validator::with_rules(RuleSet::new().strict_avatar_dimensions(true));
    let report = strict.report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::AVATAR_VALID,
        "AVATAR_DIMENSIONS",
        Severity::Error,
    );
}

#[test]
fn item_21_correct_avatar_dimensions_pass_when_strict() {
    let temp = TempDir::new("i21d");
    let manifest = build_single(temp.path());
    let strict = Validator::with_rules(RuleSet::new().strict_avatar_dimensions(true));
    let report = strict.report_dir(temp.path(), &manifest);
    assert!(
        report.blocking().is_empty(),
        "512×512 头像应通过严格口径，实际：{:#?}",
        report.blocking()
    );
}

#[test]
fn item_21_absent_avatar_key_is_not_reported() {
    let temp = TempDir::new("i21e");
    let mut value: serde_json::Value = serde_json::from_str(SENIOR_PLUGIN).expect("应可解析");
    value.as_object_mut().expect("应是对象").remove("avatar");
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    let manifest = build_package(temp.path(), &patched, false);
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::AVATAR_VALID).is_none());
}

#[test]
fn item_22_immutable_field_change_is_reported() {
    let temp = TempDir::new("i22");
    let manifest = build_single(temp.path());
    // 基线是原 manifest；把 name 改掉后与基线比对 ⇒ 不可变字段被改。
    let mut value: serde_json::Value = serde_json::from_str(SENIOR_PLUGIN).expect("应可解析");
    value["name"] = serde_json::json!("renamed-developer");
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let changed = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let validator = Validator::with_rules(RuleSet::new().with_baseline(manifest));
    let report = validator.report_dir(temp.path(), &changed);
    assert_finding(
        &report,
        item::IMMUTABLE_FIELDS_INTACT,
        "IMMUTABLE_FIELD",
        Severity::Error,
    );
}

#[test]
fn item_22_is_skipped_without_baseline() {
    let temp = TempDir::new("i22b");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::IMMUTABLE_FIELDS_INTACT).is_none());
}

#[test]
fn item_23_duplicate_name_profession_is_warning() {
    let temp = TempDir::new("i23");
    let _manifest = build_team(temp.path());
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    // 团员 name 与 profession 中文相同（上游实测违规形态）。
    value["members"][1]["profession"]["zh"] = value["members"][1]["name"]["zh"].clone();
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::MEMBER_NAME_NOT_PROFESSION,
        "DUPLICATE_NAME_PROFESSION",
        Severity::Warning,
    );
    // Warning 不阻断。
    assert!(report.is_ok(), "Warning 级不应阻断注册");
}

#[test]
fn item_23_lead_is_exempt_from_name_profession_rule() {
    let temp = TempDir::new("i23b");
    let _manifest = build_team(temp.path());
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    value["members"][0]["profession"]["zh"] = value["members"][0]["name"]["zh"].clone();
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::MEMBER_NAME_NOT_PROFESSION).is_none());
}

#[test]
fn item_24_invalid_connector_declaration_is_warning() {
    let temp = TempDir::new("i24");
    let _manifest = build_team(temp.path());
    let mut value: serde_json::Value = serde_json::from_str(PANDAI_PLUGIN).expect("应可解析");
    value["dependencies"]["connectors"] = serde_json::json!(["bad/connector"]);
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let manifest = ExpertManifest::from_json_str(&patched).expect("应可解析");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::CONNECTOR_DECL_RESOLVES,
        "CONNECTOR_DECL_INVALID",
        Severity::Warning,
    );
    assert!(report.is_ok(), "Warning 级不应阻断注册");
}

#[test]
fn item_24_valid_connector_declaration_passes() {
    let temp = TempDir::new("i24b");
    let manifest = build_team(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::CONNECTOR_DECL_RESOLVES).is_none());
}

#[test]
fn item_25_already_registered_name_is_reported() {
    let temp = TempDir::new("i25");
    let manifest = build_single(temp.path());
    // 传入已注册名单（注册表在卡 3；本卡由调用方注入）。
    let validator = Validator::with_rules(
        RuleSet::new().with_registered_names(["senior-developer", "other-expert"]),
    );
    let report = validator.report_dir(temp.path(), &manifest);
    assert_finding(
        &report,
        item::NAME_NOT_REGISTERED,
        "ALREADY_REGISTERED",
        Severity::Error,
    );
}

#[test]
fn item_25_is_skipped_without_registered_names() {
    let temp = TempDir::new("i25b");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(finding_for(&report, item::NAME_NOT_REGISTERED).is_none());
}
