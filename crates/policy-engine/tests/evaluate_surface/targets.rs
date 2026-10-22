//! 生效对象四维度：各一例命中 + 一例不命中；空 targets fail-closed（任务卡 D8）。
use crate::{allow_rule, request_with_ctx, spec_with_targets};
use policy_engine::evaluate::evaluate;
use policy_engine::model::{Action, PolicyMode, TargetContext, Targets};

const URL: &str = "https://example.com/a";

fn targets_with_user() -> Targets {
    Targets {
        user_ids: vec!["u1".to_string()],
        ..Targets::default()
    }
}

fn targets_with_role() -> Targets {
    Targets {
        role_ids: vec!["r1".to_string()],
        ..Targets::default()
    }
}

fn targets_with_dept() -> Targets {
    Targets {
        dept_ids: vec!["d1".to_string()],
        ..Targets::default()
    }
}

fn targets_with_account() -> Targets {
    Targets {
        account_ids: vec!["a1".to_string()],
        ..Targets::default()
    }
}

fn policy(targets: Targets) -> policy_engine::model::PolicySpec {
    spec_with_targets(
        PolicyMode::SomeWhite,
        targets,
        vec![allow_rule("example.com/*")],
    )
}

fn decision_for(targets: Targets, ctx: TargetContext) -> policy_engine::evaluate::Decision {
    evaluate(&policy(targets), request_with_ctx(URL, Action::Access, ctx))
}

#[test]
fn user_id_dimension_matches() {
    let decision = decision_for(targets_with_user(), TargetContext::new("u1"));
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn user_id_dimension_misses() {
    let decision = decision_for(targets_with_user(), TargetContext::new("u2"));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象不匹配");
}

#[test]
fn role_id_dimension_matches() {
    let ctx = TargetContext::new("u1").with_role("r1");
    let decision = decision_for(targets_with_role(), ctx);
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn role_id_dimension_misses() {
    let ctx = TargetContext::new("u1").with_role("r2");
    let decision = decision_for(targets_with_role(), ctx);
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象不匹配");
}

#[test]
fn dept_id_dimension_matches() {
    let ctx = TargetContext::new("u1").with_dept("d1");
    let decision = decision_for(targets_with_dept(), ctx);
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn dept_id_dimension_misses() {
    let ctx = TargetContext::new("u1").with_dept("d2");
    let decision = decision_for(targets_with_dept(), ctx);
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象不匹配");
}

#[test]
fn account_id_dimension_matches() {
    let ctx = TargetContext::new("u1").with_account("a1");
    let decision = decision_for(targets_with_account(), ctx);
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn account_id_dimension_misses() {
    // 请求没有账号上下文，该维度不命中。
    let decision = decision_for(targets_with_account(), TargetContext::new("u1"));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象不匹配");
}

#[test]
fn dimensions_are_parallel_any_hit_is_enough() {
    // user 维度不命中，但 role 维度命中：四个维度并列，任一命中即算匹配。
    let targets = Targets {
        user_ids: vec!["u1".to_string()],
        role_ids: vec!["r1".to_string()],
        ..Targets::default()
    };
    let ctx = TargetContext::new("u2").with_role("r1");
    let decision = decision_for(targets, ctx);
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn empty_targets_are_fail_closed() {
    let decision = decision_for(Targets::empty(), TargetContext::new("u1"));
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 生效对象为空");
}
