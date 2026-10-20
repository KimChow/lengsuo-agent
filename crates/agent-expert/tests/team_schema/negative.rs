//! 三个反例：各自命中**对应的**错误码（不是「随便报个错」）。
//!
//! | 反例 | 改动 | 期望错误码 | §17 |
//! |---|---|---|---|
//! | 无 lead | 主理人 `role` 改成 `member` | `TEAM_LEAD_MISSING` | #15 |
//! | 两个 lead | 第二个成员 `role` 改成 `lead` | `TEAM_LEAD_MISSING` | #15 |
//! | `memberAgents` 多一个 | 追加一个不在 `members` 里的 id | `TEAM_MEMBERS_MISMATCH` | #16 |
//!
//! 断言口径：**命中期望码** + 视图两分法的对应表现。
//!
//! 注：「无 lead / 两个 lead」在 #16 里也会连带报 `TEAM_MEMBERS_MISMATCH`
//! （`memberAgents` 与「非 lead 成员」集合对不上，属合法联动），故只断言
//! **命中**期望码；`memberAgents` 多一个则不影响 #15。

use super::support::*;

/// 反例 ①：无 lead ⇒ `TEAM_LEAD_MISSING`。
#[test]
fn no_lead_hits_team_lead_missing() {
    let manifest = parse(NO_LEAD);
    let view = TeamView::new(&manifest);
    let codes = codes(&view);
    assert!(codes.contains(&"TEAM_LEAD_MISSING"), "{codes:?}");
    assert!(view.lead().is_none(), "无 lead 时视图取不到主理人");
    assert_eq!(view.member_ids().count(), manifest.members.len());
}

/// 反例 ②：两个 lead ⇒ `TEAM_LEAD_MISSING`（§17 #15 要求**恰一**）。
#[test]
fn two_leads_hit_team_lead_missing() {
    let manifest = parse(TWO_LEADS);
    let view = TeamView::new(&manifest);
    let codes = codes(&view);
    assert!(codes.contains(&"TEAM_LEAD_MISSING"), "{codes:?}");
    assert_eq!(view.table().leads, 2);
}

/// 反例 ③：`memberAgents` 多一个 ⇒ `TEAM_MEMBERS_MISMATCH`。
#[test]
fn extra_member_agent_hits_team_members_mismatch() {
    let manifest = parse(EXTRA_MEMBER_AGENT);
    let view = TeamView::new(&manifest);
    let codes = codes(&view);
    assert!(codes.contains(&"TEAM_MEMBERS_MISMATCH"), "{codes:?}");
    assert!(!codes.contains(&"TEAM_LEAD_MISSING"), "{codes:?}");
    // 视图的两分法仍按 `members` 走：多出来的 id 只在 `teamInfo` 里。
    assert_eq!(view.member_ids().count(), 3);
    assert_eq!(
        manifest
            .team_info
            .as_ref()
            .expect("应有 teamInfo")
            .member_agents
            .len(),
        4
    );
}

/// 反例的错误码与 §17 编号一一对应（`validate::item` 常量）。
///
/// 视图只暴露错误码；编号由校验器报告携带（与 `support::item` 常量同源）。
#[test]
fn negative_codes_map_to_section_17_items() {
    assert_eq!(item::TEAM_LEAD_UNIQUE, 15);
    assert_eq!(item::TEAM_MEMBERS_MATCH, 16);
    let manifest = parse(EXTRA_MEMBER_AGENT);
    let found = TeamView::new(&manifest).check_consistency();
    assert!(found
        .iter()
        .any(|err| err.code() == "TEAM_MEMBERS_MISMATCH"));
}
