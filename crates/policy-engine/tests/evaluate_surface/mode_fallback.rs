//! 无命中规则时的模式兜底：四种模式各 ≥2 例（任务卡 D5、SPEC §3）。
use crate::{allow_rule, deny_rule, request, spec};
use policy_engine::evaluate::evaluate;
use policy_engine::model::{Action, PolicyMode};

const MATCHING_URL: &str = "https://example.com/a";
const OTHER_URL: &str = "https://other.org/x";

#[test]
fn all_mode_allows_when_no_rule_matches() {
    let policy = spec(PolicyMode::All, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request(OTHER_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "全放行模式");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn all_mode_still_applies_a_matched_allow_rule() {
    let policy = spec(PolicyMode::All, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request(MATCHING_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn all_mode_still_applies_a_matched_blacklist_rule() {
    // 规格 §2.6：URL 命中后看规则，不按模式区分。
    let policy = spec(PolicyMode::All, vec![deny_rule("example.com/*")]);
    let decision = evaluate(&policy, request(MATCHING_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn some_black_mode_allows_when_no_rule_matches() {
    let policy = spec(PolicyMode::SomeBlack, vec![deny_rule("example.com/*")]);
    let decision = evaluate(&policy, request(OTHER_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "部分黑名单模式未命中黑名单规则");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn some_black_mode_denies_when_a_blacklist_rule_matches() {
    let policy = spec(PolicyMode::SomeBlack, vec![deny_rule("example.com/*")]);
    let decision = evaluate(&policy, request(MATCHING_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn some_white_mode_denies_when_no_rule_matches() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request(OTHER_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 未命中任何规则");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn some_white_mode_allows_when_a_whitelist_rule_matches() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request(MATCHING_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn special_case_mode_denies_when_no_rule_matches() {
    let policy = spec(PolicyMode::SpecialCase, vec![deny_rule("example.com/*")]);
    let decision = evaluate(&policy, request(OTHER_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 未命中任何规则");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn special_case_mode_applies_a_matched_blacklist_rule() {
    let policy = spec(PolicyMode::SpecialCase, vec![deny_rule("example.com/*")]);
    let decision = evaluate(&policy, request(MATCHING_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}
