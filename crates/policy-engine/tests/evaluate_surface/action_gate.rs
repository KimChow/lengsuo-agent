//! 操作闸门：七个操作逐一 allow + deny；click_dom 选择器命中/未授权/缺失（任务卡 D6、SPEC §2.6）。
use crate::{request, rule, spec};
use policy_engine::evaluate::{evaluate, Decision};
use policy_engine::model::{Action, Actions, PolicyMode, RuleItem};

/// 同一条 URL 规则分别以「放行该操作」与「全拒」两种开关跑一遍。
fn gated(action: Action, enabled: Actions) -> (Decision, Decision) {
    let allowed = evaluate(
        &spec(
            PolicyMode::SomeWhite,
            vec![rule("example.com/*", true, false, enabled, &[])],
        ),
        request("https://example.com/a", action),
    );
    let blocked = evaluate(
        &spec(
            PolicyMode::SomeWhite,
            vec![rule("example.com/*", true, false, Actions::none(), &[])],
        ),
        request("https://example.com/a", action),
    );
    (allowed, blocked)
}

fn assert_gated(action: Action, enabled: Actions) {
    let (allowed, blocked) = gated(action, enabled);
    assert!(allowed.allowed, "{} 应放行", action.as_str());
    assert_eq!(allowed.reasons[0], "规则允许");
    assert_eq!(allowed.matched_rule.map(|id| id.index()), Some(0));
    assert!(!blocked.allowed, "{} 应拒绝", action.as_str());
    assert_eq!(blocked.reasons[0], "fail-closed: 操作未授权");
    // 拒绝同样报告命中规则：审计要能定位是哪条规则挡下的。
    assert_eq!(blocked.matched_rule.map(|id| id.index()), Some(0));
}

#[test]
fn access_operation_is_gated() {
    assert_gated(
        Action::Access,
        Actions {
            access: true,
            ..Actions::none()
        },
    );
}

#[test]
fn clipboard_operation_is_gated() {
    assert_gated(
        Action::Clipboard,
        Actions {
            clipboard: true,
            ..Actions::none()
        },
    );
}

#[test]
fn file_upload_operation_is_gated() {
    assert_gated(
        Action::FileUpload,
        Actions {
            file_upload: true,
            ..Actions::none()
        },
    );
}

#[test]
fn file_download_operation_is_gated() {
    assert_gated(
        Action::FileDownload,
        Actions {
            file_download: true,
            ..Actions::none()
        },
    );
}

#[test]
fn print_operation_is_gated() {
    assert_gated(
        Action::Print,
        Actions {
            print: true,
            ..Actions::none()
        },
    );
}

#[test]
fn console_operation_is_gated() {
    assert_gated(
        Action::Console,
        Actions {
            console: true,
            ..Actions::none()
        },
    );
}

fn click_rule(dom_selectors: &[&str]) -> RuleItem {
    rule(
        "example.com/*",
        true,
        false,
        Actions {
            click_dom: true,
            ..Actions::none()
        },
        dom_selectors,
    )
}

#[test]
fn click_dom_operation_not_enabled_is_denied() {
    let mut req = request("https://example.com/a", Action::ClickDom);
    req.selector = Some("#submit".to_string());
    let decision = evaluate(
        &spec(
            PolicyMode::SomeWhite,
            vec![rule(
                "example.com/*",
                true,
                false,
                Actions::none(),
                &["#submit"],
            )],
        ),
        req,
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 操作未授权");
}

#[test]
fn click_dom_allowed_when_selector_hits() {
    let mut req = request("https://example.com/a", Action::ClickDom);
    req.selector = Some("#submit".to_string());
    let decision = evaluate(
        &spec(PolicyMode::SomeWhite, vec![click_rule(&["#submit", "#ok"])]),
        req,
    );
    assert!(decision.allowed);
    assert_eq!(decision.reasons[0], "规则允许");
}

#[test]
fn click_dom_denied_when_selector_not_allowed() {
    let mut req = request("https://example.com/a", Action::ClickDom);
    req.selector = Some("#other".to_string());
    let decision = evaluate(
        &spec(PolicyMode::SomeWhite, vec![click_rule(&["#submit"])]),
        req,
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 点击操作选择器未授权");
}

#[test]
fn click_dom_denied_when_selector_missing() {
    let decision = evaluate(
        &spec(PolicyMode::SomeWhite, vec![click_rule(&["#submit"])]),
        request("https://example.com/a", Action::ClickDom),
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reasons[0], "fail-closed: 点击操作缺少选择器");
}
