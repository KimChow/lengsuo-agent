//! fail-closed 与错误码契约：解析失败即拒绝，不降级放行。
//!
//! 以及 team 类型包的识别（成员数量关系）。
use super::support::*;

// ---------------------------------------------------------------------------
// fail-closed 与错误码契约
// ---------------------------------------------------------------------------

#[test]
fn fail_closed_returns_first_blocking_finding() {
    let temp = TempDir::new("fc");
    let _manifest = build_single(temp.path());
    // 同时破坏两项：tags 数量（Error）+ 人格文件缺失（Error）。
    let mut value: serde_json::Value = serde_json::from_str(SENIOR_PLUGIN).expect("应可解析");
    value["tags"] = serde_json::json!([]);
    let patched = serde_json::to_string_pretty(&value).expect("应可序列化");
    fs::write(temp.path().join(".lengsuo-plugin/plugin.json"), &patched).expect("应可写");
    let changed = ExpertManifest::from_json_str(&patched).expect("应可解析");
    fs::remove_file(temp.path().join("agents/senior-developer.md")).expect("应可删");

    let error = Validator::new()
        .validate_dir(temp.path(), &changed)
        .expect_err("Error 级发现应让整体不通过");
    assert_eq!(error.severity(), Severity::Error);
    assert!(error.is_blocking());
    // 错误要指明是哪一条（§17 编号）。
    assert!(
        (1..=25).contains(&error.item()),
        "错误须带 §17 编号，实际 {}",
        error.item()
    );
    // 编号 + 码 + 文案都可读。
    let text = error.to_string();
    assert!(text.contains("§17 #"), "文案应含编号：{text}");
    assert!(!text.is_empty());
}

#[test]
fn every_finding_carries_section17_item_number() {
    let temp = TempDir::new("numbers");
    let manifest = build_single(temp.path());
    // 一次性破坏多项，看报告里每条发现都带编号。
    fs::remove_file(temp.path().join("avatars/expert.png")).expect("应可删");
    fs::remove_file(temp.path().join("skills/browser-use/SKILL.md")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert!(report.findings.len() >= 2);
    for finding in &report.findings {
        assert!(
            (1..=25).contains(&finding.item()),
            "发现缺编号：{finding:?}"
        );
        assert!(!finding.code().is_empty());
    }
}

#[test]
fn report_lists_all_findings_not_just_the_first() {
    let temp = TempDir::new("allfind");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join("avatars/expert.png")).expect("应可删");
    fs::remove_file(temp.path().join("skills/browser-use/SKILL.md")).expect("应可删");
    fs::remove_file(temp.path().join("agents/senior-developer.md")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_eq!(report.blocking().len(), 3, "三条独立发现都应列出");
}

#[test]
fn validate_error_exposes_frozen_expert_error_variant() {
    let temp = TempDir::new("variant");
    let manifest = build_single(temp.path());
    fs::remove_file(temp.path().join("avatars/expert.png")).expect("应可删");
    let report = Validator::new().report_dir(temp.path(), &manifest);
    let finding = finding_for(&report, item::AVATAR_VALID).expect("应报头像缺失");
    assert!(
        finding.expert_error().is_some(),
        "应包着卡 1 冻结的 ExpertError"
    );
    assert_eq!(finding.expert_error().unwrap().code(), "AVATAR_MISSING");
}

#[test]
fn error_code_matches_frozen_expert_error_code_table() {
    // 卡 1 冻结的错误码全集（error.rs `code()`）——本卡只包它，不新增同名码。
    let temp = TempDir::new("codes");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    for finding in &report.findings {
        if let Some(err) = finding.expert_error() {
            assert_eq!(finding.code(), err.code(), "码应与 ExpertError 一致");
        }
    }
}

#[test]
fn single_expert_package_has_no_team_findings() {
    // 单专家包不该报团队项（否则误报）。
    let temp = TempDir::new("noteam");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    for number in [
        item::TEAM_LEAD_UNIQUE,
        item::TEAM_MEMBERS_MATCH,
        item::TEAM_PROFESSION_MATCHES_DISPLAY,
        item::AGENT_NAME_NOT_GENERIC,
    ] {
        assert!(
            finding_for(&report, number).is_none(),
            "单专家包不应报团队项 §17 #{number}"
        );
    }
}

#[test]
fn team_type_is_recognised_by_validator() {
    let temp = TempDir::new("teamtype");
    let manifest = build_team(temp.path());
    assert_eq!(manifest.expert_type, ExpertType::Team);
    assert_eq!(manifest.members.len(), 6);
    assert_eq!(manifest.team_info.as_ref().unwrap().member_agents.len(), 5);
    assert_eq!(PANDAI_MEMBERS.len(), 5);
    assert_eq!(PANDAI_SKILLS.len(), 6);
    assert_eq!(SENIOR_SKILLS.len(), 3);
}
