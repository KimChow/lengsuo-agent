//! 专家包 manifest 结构体测试。
//!
//! 样本来源：`本地对照资料（不入库）{senior-developer,pandaai-team}`
//! 的 `.codebuddy-plugin/plugin.json`，**逐字复制**进 `tests/fixtures/`
//! （CI 上无 `/home/ubuntu/ref` 路径，故不写死绝对路径）。
//!
//! 断言口径按调研 §3.1「plugin.json 逐字段判定表」：字段名/类型/必填三样
//! 都对着样本核；另覆盖规范外字段（`dependencies.connectors`）与
//! 样本事实（单专家样本无 `plugin` 键）。

use agent_expert::{CategoryId, ExpertError, ExpertManifest, ExpertType, MemberRole, Severity};
use serde_json::Value;
use std::str::FromStr;

const SENIOR: &str = include_str!("fixtures/senior-developer.plugin.json");
const PANDAI: &str = include_str!("fixtures/pandaai-team.plugin.json");

fn parse(text: &str) -> ExpertManifest {
    ExpertManifest::from_json_str(text).expect("真实样本应可解析")
}

#[test]
fn single_sample_parses_every_field() {
    let m = parse(SENIOR);
    // —— 身份 ——
    assert_eq!(m.name, "senior-developer");
    assert_eq!(m.version, "1.1.1");
    assert_eq!(m.description, "Senior full-stack developer with 10+ years experience, proficient in multiple languages and frameworks, delivers production-ready code with rigorous quality control");
    assert_eq!(m.author, None);
    assert_eq!(m.homepage, None);
    assert_eq!(m.license, None);
    assert!(m.keywords.is_empty());
    // —— 类型 ——
    assert_eq!(m.expert_type, ExpertType::Agent);
    assert_eq!(m.agent_name, "senior-developer");
    assert_eq!(m.category_id, CategoryId::Engineering);
    // —— 展示 ——
    assert_eq!(m.display_name.zh, "吴八哥");
    assert_eq!(m.display_name.en, "Will");
    assert_eq!(m.profession.zh, "高级开发工程师");
    assert_eq!(m.profession.en, "Senior Developer");
    assert_eq!(
        m.display_description.zh,
        "10年以上全栈经验，精通多种语言和框架，以严谨的工程纪律交付可运行的高质量代码"
    );
    assert_eq!(m.avatar.as_deref(), Some("avatars/expert.png"));
    // —— 提示 ——
    assert_eq!(m.default_init_prompt.zh, m.quick_prompts[0].zh);
    assert_eq!(m.default_init_prompt.en, m.quick_prompts[0].en);
    assert_eq!(m.tags.len(), 3);
    assert_eq!(m.tags[0].zh, "全栈开发");
    assert_eq!(m.quick_prompts.len(), 3);
    // —— 能力 ——
    assert_eq!(m.agents, vec!["./agents/senior-developer.md".to_string()]);
    assert_eq!(m.skills.len(), 3);
    assert_eq!(m.skills[0], "./skills/browser-use");
    assert_eq!(m.dependencies, None);
    // —— 团队（单专家无）——
    assert!(m.members.is_empty());
    assert!(m.team_info.is_none());
    assert!(!m.is_team());
}

#[test]
fn team_sample_parses_members_and_team_info() {
    let m = parse(PANDAI);
    assert_eq!(m.name, "pandaai-ai-quant-research-team");
    assert_eq!(m.version, "0.4.2");
    assert_eq!(m.expert_type, ExpertType::Team);
    assert_eq!(m.agent_name, "pd-ai-quant-research-team");
    assert_eq!(m.category_id, CategoryId::FinanceInvestment);
    assert_eq!(
        m.homepage.as_deref(),
        Some("https://github.com/quantskills/agent-ai-quant-research-team")
    );
    assert_eq!(m.keywords.len(), 4);
    assert_eq!(m.plugin.as_deref(), Some("pandaai-ai-quant-research-team"));
    assert_eq!(m.agents.len(), 6);
    assert_eq!(m.skills.len(), 6);
    assert_eq!(m.tags.len(), 3);
    assert_eq!(m.quick_prompts.len(), 3);
    // 作者是 `{name,email}` 对象（§3.1 表格的类型列）
    let author = m.author.as_ref().expect("pandaai 样本带 author");
    assert_eq!(author.name, "PandaAi");
    assert_eq!(author.email.as_deref(), Some("business@pandaai.online"));
    // teamInfo：leadAgent + memberAgents（不含主理人）
    let team = m.team_info.as_ref().expect("team 型须带 teamInfo");
    assert_eq!(team.lead_agent, "pd-ai-quant-research-team");
    assert_eq!(team.member_agents.len(), 5);
    assert!(!team
        .member_agents
        .contains(&"pd-ai-quant-research-team".to_string()));
    // members：含主理人（role=lead）
    assert_eq!(m.members.len(), 6);
    let lead = m.lead_member().expect("须能取到主理人");
    assert_eq!(lead.id, "pd-ai-quant-research-team");
    assert_eq!(lead.role, MemberRole::Lead);
    assert_eq!(lead.name.zh, "PandaAi主理人");
    assert_eq!(lead.profession.zh, "AI量化研究会诊主理人");
    assert_eq!(lead.avatar.as_deref(), Some("avatars/pandaai-logo-v2.png"));
    let members: Vec<&str> = m
        .members
        .iter()
        .filter(|x| x.role == MemberRole::Member)
        .map(|x| x.id.as_str())
        .collect();
    assert_eq!(
        members,
        vec![
            "source-replication-researcher",
            "factor-engineer",
            "pandaai-experimenter",
            "overfit-auditor",
            "performance-reporter",
        ]
    );
}

#[test]
fn team_sample_exposes_prompt_pair_and_persona() {
    let m = parse(PANDAI);
    // 主理人 / 成员两套提示（§8.2）：人格 MD 路径
    let pair = m.prompt_pair().expect("team 型须给出两套提示");
    assert_eq!(
        pair.lead.as_deref(),
        Some("./agents/pd-ai-quant-research-team.md")
    );
    assert_eq!(pair.members.len(), 5);
    assert_eq!(pair.members[0], "./agents/source-replication-researcher.md");
    assert_eq!(pair.members[4], "./agents/performance-reporter.md");
    // 人格文件路径 = agents 里 stem == agentName 的那项
    assert_eq!(m.persona(), Some("./agents/pd-ai-quant-research-team.md"));
}

#[test]
fn single_sample_has_no_prompt_pair_but_has_persona() {
    let m = parse(SENIOR);
    assert_eq!(m.prompt_pair(), None);
    assert_eq!(m.persona(), Some("./agents/senior-developer.md"));
}

#[test]
fn round_trip_is_semantically_equal_to_source_json() {
    for (label, text) in [("senior-developer", SENIOR), ("pandaai-team", PANDAI)] {
        let m = parse(text);
        // 类型层往返
        let again =
            ExpertManifest::from_json_str(&m.to_json_string().expect("序列化")).expect("再解析");
        assert_eq!(again, m, "{label}: 类型层往返应相等");
        // JSON 层往返（键集合与取值都相等；键序不参与比较）
        let original: Value = serde_json::from_str(text).expect("原样本是合法 JSON");
        let reserialized: Value = serde_json::from_str(&m.to_json_string().expect("序列化"))
            .expect("再序列化是合法 JSON");
        assert_eq!(reserialized, original, "{label}: JSON 层往返应语义相等");
    }
}

#[test]
fn dependencies_connectors_is_parsed_but_not_enforced() {
    // 规范外字段（§1.3）：只解析不强制
    let m = parse(PANDAI);
    let deps = m
        .dependencies
        .as_ref()
        .expect("pandaai 样本带 dependencies");
    assert_eq!(deps.connectors, vec!["pandadata".to_string()]);
    // 单专家样本无 dependencies ⇒ None
    assert_eq!(parse(SENIOR).dependencies, None);
}

#[test]
fn category_covers_twelve_ids_and_rejects_unknown() {
    assert_eq!(CategoryId::ALL.len(), 12);
    assert_eq!(CategoryId::ALL[0].as_str(), "01-ProductDesign");
    assert_eq!(CategoryId::ALL[1].as_str(), "02-Engineering");
    assert_eq!(CategoryId::ALL[7].as_str(), "08-FinanceInvestment");
    assert_eq!(CategoryId::ALL[11].as_str(), "12-IndustryConsultant");
    assert_eq!(CategoryId::FinanceInvestment.zh_label(), "金融投资");
    let err = CategoryId::from_str("99-Unknown").expect_err("未知分类应报错");
    match &err {
        ExpertError::UnknownCategory { value } => assert_eq!(value, "99-Unknown"),
        other => panic!("应报 UnknownCategory，实际 {other:?}"),
    }
    assert_eq!(err.code(), "UNKNOWN_CATEGORY");
}

#[test]
fn unknown_category_in_manifest_reports_unknown_category() {
    let text = PANDAI.replace("\"08-FinanceInvestment\"", "\"99-Unknown\"");
    let err = ExpertManifest::from_json_str(&text).expect_err("未知分类应报错");
    match err {
        ExpertError::UnknownCategory { value } => assert_eq!(value, "99-Unknown"),
        other => panic!("应报 UnknownCategory，实际 {other:?}"),
    }
}

#[test]
fn missing_required_field_reports_missing_field() {
    let text = PANDAI.replace("\"version\": \"0.4.2\",", "");
    let err = ExpertManifest::from_json_str(&text).expect_err("缺必填字段应报错");
    match &err {
        ExpertError::MissingField { field } => assert_eq!(field, "version"),
        other => panic!("应报 MissingField，实际 {other:?}"),
    }
    assert_eq!(err.code(), "MISSING_FIELD");
}

#[test]
fn malformed_and_mistyped_json_report_manifest_parse() {
    let err = ExpertManifest::from_json_str("{ not json").expect_err("坏 JSON 应报错");
    assert!(matches!(err, ExpertError::ManifestParse { .. }));
    assert_eq!(err.code(), "MANIFEST_PARSE_FAILED");
    // 类型错：tags 必须是数组
    let text = PANDAI.replace("\"tags\": [", "\"tags\": \"");
    let err = ExpertManifest::from_json_str(&text).expect_err("类型错应报错");
    assert!(matches!(err, ExpertError::ManifestParse { .. }));
    // 根不是对象
    let err = ExpertManifest::from_json_str("[]").expect_err("根非对象应报错");
    assert!(matches!(err, ExpertError::ManifestParse { .. }));
}

#[test]
fn severity_separates_blocking_from_warning() {
    let warning = ExpertError::DuplicateNameProfession {
        member_id: "factor-engineer".to_string(),
    };
    assert_eq!(warning.severity(), Severity::Warning);
    assert_eq!(warning.code(), "DUPLICATE_NAME_PROFESSION");
    let blocking = ExpertError::MissingField {
        field: "name".to_string(),
    };
    assert_eq!(blocking.severity(), Severity::Error);
}
