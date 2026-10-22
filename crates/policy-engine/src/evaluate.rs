//! 判定层：把策略数据模型与通配模式接到一个可调用的判定入口（规格 §2）。
//!
//! 入口 [`evaluate`] 的判定顺序是冻结的（任务卡 D4，先命中先返回）：
//!
//! 1. 生效对象为空 ⇒ 拒绝（`fail-closed: 生效对象为空`）
//! 2. 生效对象不匹配 ⇒ 拒绝（`fail-closed: 生效对象不匹配`）
//! 3. 生效时间窗不在窗内 ⇒ 拒绝（`fail-closed: 生效时间窗不在窗内`）
//! 4. 规则集为空 ⇒ 拒绝（`fail-closed: 规则集为空`）
//! 5. 请求 URL 非法 ⇒ 拒绝（`fail-closed: 请求 URL 非法`）
//! 6. 规则模式编译失败 ⇒ 拒绝（`fail-closed: 规则模式编译失败`）
//! 7. 挑命中规则：无命中走模式兜底，有命中走规则闸门
//!
//! 全部理由文案冻结在 [`decision`] 模块的常量里，`reasons[0]` 逐字可断言。
//! 本层是纯域逻辑：不做 IO、不取系统时间（时间只来自 `EvaluateRequest::now_ms`）、
//! 不解析 DOM（`click_dom` 只做选择器字符串比对）。

mod decision;
mod request;
mod select;

pub use decision::{
    Decision, RuleItemId, REASON_ACTION_DENIED, REASON_ALL_MODE, REASON_BAD_PATTERN,
    REASON_BAD_URL, REASON_BLACKLIST_HIT, REASON_NO_MATCH, REASON_NO_RULES, REASON_OUT_OF_WINDOW,
    REASON_RULE_ALLOWED, REASON_SELECTOR_DENIED, REASON_SELECTOR_MISSING, REASON_SOME_BLACK_MISS,
    REASON_TARGETS_EMPTY, REASON_TARGETS_MISMATCH,
};
pub use request::EvaluateRequest;

use crate::model::{Action, PolicyMode, PolicySpec};
use crate::pattern::NormalizedUrl;

/// 按一条策略判定一次请求（规格 §2.1）。
///
/// 判定顺序与 reason 文案均冻结，见本模块文档。
#[must_use]
pub fn evaluate(spec: &PolicySpec, req: EvaluateRequest) -> Decision {
    // 1. 生效对象为空：空生效对象不匹配任何请求（`model/target.rs` 的既定语义）。
    if spec.targets.is_empty() {
        return Decision::deny(REASON_TARGETS_EMPTY);
    }
    // 2. 生效对象不匹配（四个维度并列，任一非空维度命中即算匹配）。
    if !select::targets_match(&spec.targets, &req.targets_ctx) {
        return Decision::deny(REASON_TARGETS_MISMATCH);
    }
    // 3. 生效时间窗不在窗内。
    if !select::window_allows(spec, req.now_ms) {
        return Decision::deny(REASON_OUT_OF_WINDOW);
    }
    // 4. 规则集为空：排在模式兜底之前，空规则集一律拒绝。
    if spec.rules.is_empty() {
        return Decision::deny(REASON_NO_RULES);
    }
    // 5. 请求 URL 非法。
    let url = match NormalizedUrl::parse(&req.url) {
        Ok(url) => url,
        Err(_) => return Decision::deny(REASON_BAD_URL),
    };
    // 6. 规则模式编译失败：配置缺陷一律拒绝。
    let compiled = match select::compile_rules(&spec.rules) {
        Ok(compiled) => compiled,
        Err(_) => return Decision::deny(REASON_BAD_PATTERN),
    };

    // 7. 挑命中规则：无命中走模式兜底，有命中走规则闸门。
    match select::select_rule(&compiled, &url) {
        None => mode_fallback(spec.mode),
        Some(index) => rule_decision(spec, &req, index),
    }
}

/// 无命中任何规则时的模式兜底（任务卡 D5）。
///
/// 模式定性优先于规格 §2.2 的「无任何策略命中 ⇒ 拒绝」：后者在单策略 API 内
/// 解释为「白名单 / 特殊案例模式未命中」。
fn mode_fallback(mode: PolicyMode) -> Decision {
    match mode {
        PolicyMode::All => Decision::allow(REASON_ALL_MODE),
        PolicyMode::SomeBlack => Decision::allow(REASON_SOME_BLACK_MISS),
        PolicyMode::SomeWhite | PolicyMode::SpecialCase => Decision::deny(REASON_NO_MATCH),
    }
}

/// 命中规则后的闸门（任务卡 D6）：URL 命中后看 `actions.<action>`，不按模式区分。
fn rule_decision(spec: &PolicySpec, req: &EvaluateRequest, index: usize) -> Decision {
    let rule = &spec.rules[index];
    let id = RuleItemId::new(index);

    // 黑名单规则：不可访问，直接拒绝，且不记录。
    if !rule.is_accessible {
        return denied(REASON_BLACKLIST_HIT, id);
    }

    // 白名单规则：过操作闸门。拒绝时仍报告是哪条规则命中。
    if !rule.actions.enabled(req.action) {
        return denied(REASON_ACTION_DENIED, id);
    }
    if req.action == Action::ClickDom {
        let Some(selector) = req.selector.as_deref() else {
            return denied(REASON_SELECTOR_MISSING, id);
        };
        if !rule
            .dom_selectors
            .iter()
            .any(|allowed| allowed.as_str() == selector)
        {
            return denied(REASON_SELECTOR_DENIED, id);
        }
    }

    let mut decision = Decision::allow(REASON_RULE_ALLOWED);
    decision.matched_rule = Some(id);
    // `is_log` 在存在生效时间窗时被禁用（规格 §1）。
    decision.should_log = rule.is_log && spec.active_range.is_none();
    decision
}

/// 由已命中规则给出的拒绝结论：`matched_rule` 指向该规则（拒绝一律不记录）。
fn denied(reason: &str, id: RuleItemId) -> Decision {
    let mut decision = Decision::deny(reason);
    decision.matched_rule = Some(id);
    decision
}
