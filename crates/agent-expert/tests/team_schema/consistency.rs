//! `TeamView` 的两分法与三处数量关系。

use super::support::*;

/// 主理人只从 `members` 里 `role == lead` 的那项取，**不重复出现在团员里**。
#[test]
fn lead_is_not_repeated_in_members() {
    let manifest = parse(TEAM);
    let view = TeamView::new(&manifest);
    let lead = view.lead().expect("应有主理人");
    assert_eq!(lead.id, "qr-research-team");
    let member_ids: Vec<&str> = view.member_ids().collect();
    assert!(
        !member_ids.contains(&lead.id.as_str()),
        "主理人不得出现在团员里"
    );
    assert_eq!(
        member_ids,
        vec!["source-replicator", "factor-engineer", "platform-runner"]
    );
    // 两分法合起来正好是 `members` 全集（不重不漏）。
    let lead_count = usize::from(view.lead().is_some());
    assert_eq!(lead_count + member_ids.len(), manifest.members.len());
}

/// 正例：三处数量关系都成立（agents 4 = members 4；memberAgents 3 = members - 1）。
#[test]
fn team_sample_satisfies_three_relations() {
    let manifest = parse(TEAM);
    let view = TeamView::new(&manifest);
    let table = view.table();
    assert_eq!(table.agents, 4);
    assert_eq!(table.members, 4);
    assert_eq!(table.member_agents, 3);
    assert_eq!(table.leads, 1);
    assert!(table.holds(), "三处数量关系应全部成立：{table:?}");
    let found = view.check_consistency();
    assert!(found.is_empty(), "正例不应有发现：{found:?}");
}

/// 单专家（`agent` 型）没有团队语义 ⇒ 视图恒空。
#[test]
fn single_agent_has_no_team_findings() {
    let manifest = parse(AGENT);
    let view = TeamView::new(&manifest);
    assert!(view.lead().is_none());
    assert_eq!(view.member_ids().count(), 0);
    assert!(view.check_consistency().is_empty());
    assert_eq!(view.agent_paths().count(), 1);
}

/// `agents[]` 与 `members[]` 数量不等 ⇒ 聚合视图报一条成员集合不一致。
#[test]
fn agent_count_mismatch_is_reported() {
    let mut manifest = parse(TEAM);
    manifest.agents.pop();
    let view = TeamView::new(&manifest);
    assert!(
        reports(&view, "TEAM_MEMBERS_MISMATCH"),
        "{:?}",
        codes(&view)
    );
}

/// 缺 `teamInfo` ⇒ 视图补报（#16 校验函数对缺 teamInfo 放行，§17 #5 另报）。
#[test]
fn missing_team_info_is_reported() {
    let mut manifest = parse(TEAM);
    manifest.team_info = None;
    let view = TeamView::new(&manifest);
    assert!(
        reports(&view, "TEAM_MEMBERS_MISMATCH"),
        "{:?}",
        codes(&view)
    );
}

/// §17 编号与错误码的对应关系：聚合视图只取团队两项（#15 / #16）。
#[test]
fn consistency_reuses_section_17_codes() {
    assert_eq!(item::TEAM_LEAD_UNIQUE, 15);
    assert_eq!(item::TEAM_MEMBERS_MATCH, 16);
    let manifest = parse(NO_LEAD);
    let view = TeamView::new(&manifest);
    let codes = codes(&view);
    assert!(codes.contains(&"TEAM_LEAD_MISSING"), "{codes:?}");
    assert!(
        !codes.contains(&"TEAM_PROFESSION_DISPLAY_MISMATCH"),
        "{codes:?}"
    );
}

/// `teamInfo.workflow` 不影响数量关系判定（只 schema，不参与一致性）。
#[test]
fn workflow_does_not_affect_counters() {
    let mut manifest = parse(TEAM);
    let before = TeamView::new(&manifest).table();
    manifest.team_info.as_mut().expect("应有 teamInfo").workflow = Some(TeamWorkflow::default());
    let after = TeamView::new(&manifest).table();
    assert_eq!(before, after);
}

/// 严重级别口径：`TEAM_*` 错误码为 Error 级。
#[test]
fn team_findings_are_error_level() {
    let manifest = parse(NO_LEAD);
    let view = TeamView::new(&manifest);
    for err in view.check_consistency() {
        assert_eq!(err.severity(), Severity::Error, "{err:?}");
    }
}
