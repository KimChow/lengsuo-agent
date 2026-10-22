//! 规则挑选：精确 > 通配、同精度按规则序、通配兜底 vs 精确规则（任务卡 D7、SPEC §2.4）。
use crate::{allow_rule, deny_rule, request, spec};
use policy_engine::evaluate::evaluate;
use policy_engine::model::{Action, PolicyMode};

const LOGIN_URL: &str = "https://example.com/login";
const PAGE_URL: &str = "https://example.com/a";

#[test]
fn exact_rule_beats_all_pages_wildcard() {
    // 下标 0 是全网页黑名单，下标 1 是精确白名单：精确规则胜出。
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![deny_rule("*"), allow_rule("example.com/login")],
    );
    let decision = evaluate(&policy, request(LOGIN_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(1));
}

#[test]
fn host_anchored_rule_beats_all_pages_wildcard() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![deny_rule("*"), allow_rule("example.com/*")],
    );
    let decision = evaluate(&policy, request(PAGE_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(1));
}

#[test]
fn host_wildcard_rule_beats_all_pages_wildcard() {
    let policy = spec(
        PolicyMode::SomeBlack,
        vec![allow_rule("*"), deny_rule("*.example.com/*")],
    );
    let decision = evaluate(&policy, request("https://a.example.com/x", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(1));
}

#[test]
fn same_specificity_tie_keeps_the_earlier_rule() {
    // `example.com/**` 与 `example.com/*` 同为 HostAnchored、字面锚点同长（11）：
    // 精度函数返回 false，按规则序由下标 0 的黑名单裁决。
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![deny_rule("example.com/**"), allow_rule("example.com/*")],
    );
    let decision = evaluate(&policy, request(PAGE_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn same_specificity_tie_reversed_order_prefers_earlier_allow() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![allow_rule("example.com/*"), deny_rule("example.com/**")],
    );
    let decision = evaluate(&policy, request(PAGE_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn duplicate_patterns_are_resolved_by_rule_order() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![
            allow_rule("example.com/login"),
            deny_rule("example.com/login"),
        ],
    );
    let decision = evaluate(&policy, request(LOGIN_URL, Action::Access));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn all_pages_blacklist_still_denies_urls_without_a_more_specific_rule() {
    let policy = spec(PolicyMode::SomeBlack, vec![deny_rule("*")]);
    let decision = evaluate(&policy, request(PAGE_URL, Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "命中黑名单规则");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}
