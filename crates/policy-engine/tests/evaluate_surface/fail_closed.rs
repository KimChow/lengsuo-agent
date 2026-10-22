//! fail-closed 拒绝面：10 条拒绝 reason 各一例，`reasons[0]` 逐字断言（任务卡 D3/D4）。
use crate::{allow_rule, request, request_as, rule, spec};
use policy_engine::evaluate::evaluate;
use policy_engine::model::{Action, Actions, PolicyMode, Targets};

#[test]
fn empty_targets_are_fail_closed() {
    let mut policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    policy.targets = Targets::empty();
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象为空");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn mismatched_targets_are_fail_closed() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    // 策略只对 u1 生效；u2 不在生效对象内。
    let decision = evaluate(
        &policy,
        request_as("u2", "https://example.com/a", Action::Access),
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象不匹配");
}

#[test]
fn outside_active_window_is_fail_closed() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")])
        .with_active_range(100, 200)
        .expect("正向区间");
    let mut req = request("https://example.com/a", Action::Access);
    req.now_ms = 500;
    let decision = evaluate(&policy, req);
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效时间窗不在窗内");
}

#[test]
fn empty_rule_set_is_fail_closed_even_in_allow_all_mode() {
    // 空规则集排在模式兜底之前：All 模式也不能把空规则集解释成放行。
    let policy = spec(PolicyMode::All, Vec::new());
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 规则集为空");
}

#[test]
fn uncompilable_rule_pattern_is_fail_closed() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/a**b")]);
    let decision = evaluate(&policy, request("https://example.com/a", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 规则模式编译失败");
}

#[test]
fn illegal_request_url_is_fail_closed() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request("", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 请求 URL 非法");
}

#[test]
fn no_matching_rule_is_fail_closed_in_whitelist_mode() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    let decision = evaluate(&policy, request("https://other.org/x", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 未命中任何规则");
    assert_eq!(decision.matched_rule, None);
}

#[test]
fn unauthorized_action_is_fail_closed() {
    let policy = spec(PolicyMode::SomeWhite, vec![allow_rule("example.com/*")]);
    // allow_rule 只开放 access；剪贴板操作未授权。
    let decision = evaluate(&policy, request("https://example.com/a", Action::Clipboard));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 操作未授权");
    assert_eq!(decision.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn click_dom_without_selector_is_fail_closed() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![rule(
            "example.com/*",
            true,
            false,
            Actions {
                click_dom: true,
                ..Actions::none()
            },
            &[],
        )],
    );
    let decision = evaluate(&policy, request("https://example.com/a", Action::ClickDom));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 点击操作缺少选择器");
}

#[test]
fn click_dom_selector_outside_allowed_list_is_fail_closed() {
    let policy = spec(
        PolicyMode::SomeWhite,
        vec![rule(
            "example.com/*",
            true,
            false,
            Actions {
                click_dom: true,
                ..Actions::none()
            },
            &["#ok"],
        )],
    );
    let mut req = request("https://example.com/a", Action::ClickDom);
    req.selector = Some("#other".to_string());
    let decision = evaluate(&policy, req);
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 点击操作选择器未授权");
}

#[test]
fn first_failing_gate_wins_in_frozen_order() {
    // 生效对象为空排在规则集为空之前；规则集为空排在 URL 非法之前。
    let policy = spec(PolicyMode::SomeWhite, Vec::new());
    let decision = evaluate(&policy, request("", Action::Access));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 规则集为空");
}
