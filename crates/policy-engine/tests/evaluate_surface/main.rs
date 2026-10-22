//! 判定层 API 与语义集成测试（按主题拆分；单文件 700 硬限/400 软目标）。
//!
//! 共享构造器放在本文件：任务卡的 touches 表只允许本目录下这七个文件，
//! 另开 `fixtures.rs` 会越界，因此各主题文件用 `use crate::{..}` 取用这里的构造器。

mod action_gate;
mod fail_closed;
mod mode_fallback;
mod rule_priority;
mod targets;
mod window_and_log;

use policy_engine::evaluate::EvaluateRequest;
use policy_engine::model::{
    Action, Actions, PolicyMode, PolicySpec, RuleItem, TargetContext, Targets,
};

/// 规则的完整构造面（其余字段按白名单规则给默认值）。
pub fn rule(
    pattern: &str,
    is_accessible: bool,
    is_log: bool,
    actions: Actions,
    dom_selectors: &[&str],
) -> RuleItem {
    RuleItem {
        url_pattern: pattern.to_string(),
        is_accessible,
        is_log,
        actions,
        dom_selectors: dom_selectors.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// 白名单规则：可访问、只开放 access、不记录。
pub fn allow_rule(pattern: &str) -> RuleItem {
    rule(pattern, true, false, Actions::access_only(), &[])
}

/// 黑名单规则：不可访问、操作全拒。
pub fn deny_rule(pattern: &str) -> RuleItem {
    rule(pattern, false, false, Actions::none(), &[])
}

/// 策略骨架：模式 + 规则 + 单维生效对象（`u1`）+ 无时间窗。
pub fn spec(mode: PolicyMode, rules: Vec<RuleItem>) -> PolicySpec {
    spec_with_targets(mode, targets_of("u1"), rules)
}

/// 策略骨架：模式 + 规则 + 自定义生效对象 + 无时间窗。
pub fn spec_with_targets(mode: PolicyMode, targets: Targets, rules: Vec<RuleItem>) -> PolicySpec {
    PolicySpec::new("p-1", "测试策略", mode, targets, rules)
}

/// 生效对象只带 user_ids 一维（四个维度并列语义的最小骨架）。
pub fn targets_of(user_id: &str) -> Targets {
    Targets {
        user_ids: vec![user_id.to_string()],
        ..Targets::default()
    }
}

/// 与 [`targets_of`] 对应的访问者上下文。
pub fn ctx_of(user_id: &str) -> TargetContext {
    TargetContext::new(user_id)
}

/// 一次访问请求（访问者固定 `u1`，时刻固定 1000，无选择器）。
pub fn request(url: &str, action: Action) -> EvaluateRequest {
    request_with_ctx(url, action, ctx_of("u1"))
}

/// 一次访问请求（指定访问者）。
pub fn request_as(user_id: &str, url: &str, action: Action) -> EvaluateRequest {
    request_with_ctx(url, action, ctx_of(user_id))
}

/// 一次访问请求（自定义访问者上下文）。
pub fn request_with_ctx(url: &str, action: Action, targets_ctx: TargetContext) -> EvaluateRequest {
    EvaluateRequest {
        url: url.to_string(),
        action,
        now_ms: 1_000,
        targets_ctx,
        selector: None,
    }
}
