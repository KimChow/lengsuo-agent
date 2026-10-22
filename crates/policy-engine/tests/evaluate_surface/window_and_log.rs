//! 生效时间窗与 `is_log`：窗内/窗外、窗内禁用 `is_log`（任务卡 D4/D6、SPEC §1）。
use crate::{request, rule, spec};
use policy_engine::evaluate::evaluate;
use policy_engine::model::{Action, Actions, PolicyMode, RuleItem};

fn logged_rule() -> RuleItem {
    rule("example.com/*", true, true, Actions::access_only(), &[])
}

fn policy_with_window() -> policy_engine::model::PolicySpec {
    spec(PolicyMode::SomeWhite, vec![logged_rule()])
        .with_active_range(100, 200)
        .expect("正向区间")
}

#[test]
fn inside_window_is_allowed() {
    let policy = policy_with_window();
    let mut req = request("https://example.com/a", Action::Access);
    req.now_ms = 150;
    let decision = evaluate(&policy, req);
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn window_bounds_are_inclusive() {
    let policy = policy_with_window();
    for now_ms in [100, 200] {
        let mut req = request("https://example.com/a", Action::Access);
        req.now_ms = now_ms;
        assert!(evaluate(&policy, req).allowed, "闭区间端点 {now_ms} 应生效");
    }
}

#[test]
fn outside_window_is_fail_closed() {
    let policy = policy_with_window();
    let mut req = request("https://example.com/a", Action::Access);
    req.now_ms = 500;
    let decision = evaluate(&policy, req);
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效时间窗不在窗内");
}

#[test]
fn active_window_disables_is_log() {
    // 规格 §1：`is_log` 记录访问行为，但 `active_range` 存在时禁用。
    let policy = policy_with_window();
    let mut req = request("https://example.com/a", Action::Access);
    req.now_ms = 150;
    let decision = evaluate(&policy, req);
    assert!(decision.allowed);
    assert!(!decision.should_log);
}

#[test]
fn without_window_should_log_follows_is_log() {
    let policy = spec(PolicyMode::SomeWhite, vec![logged_rule()]);
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(decision.allowed);
    assert!(decision.should_log);
}

#[test]
fn without_window_and_without_is_log_nothing_is_recorded() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![rule(
            "example.com/*",
            true,
            false,
            Actions::access_only(),
            &[],
        )],
    );
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(decision.allowed);
    assert!(!decision.should_log);
}

#[test]
fn blacklist_hit_never_sets_should_log() {
    let policy = spec(
        PolicyMode::SomeBlack,
        vec![rule("example.com/*", false, true, Actions::none(), &[])],
    );
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(!decision.allowed);
    assert!(!decision.should_log);
}
