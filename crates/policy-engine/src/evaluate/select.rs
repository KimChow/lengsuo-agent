//! 生效对象匹配、时间窗与规则挑选（规格 §2.2–§2.4、任务卡 D7/D8）。

use crate::model::{PolicySpec, RuleItem, TargetContext, Targets};
use crate::pattern::{more_specific, CompileError, NormalizedUrl, Pattern};

/// 生效对象是否匹配：四个维度**并列**，任一非空维度命中即算匹配；
/// 空维度不构成约束（规格 §1「四个维度并列」，任务卡 D8）。
pub(super) fn targets_match(targets: &Targets, ctx: &TargetContext) -> bool {
    let user_hit = !targets.user_ids.is_empty()
        && ctx
            .user_id
            .as_ref()
            .is_some_and(|id| targets.user_ids.contains(id));
    let role_hit =
        !targets.role_ids.is_empty() && ctx.role_ids.iter().any(|id| targets.role_ids.contains(id));
    let dept_hit =
        !targets.dept_ids.is_empty() && ctx.dept_ids.iter().any(|id| targets.dept_ids.contains(id));
    let account_hit = !targets.account_ids.is_empty()
        && ctx
            .account_id
            .as_ref()
            .is_some_and(|id| targets.account_ids.contains(id));
    user_hit || role_hit || dept_hit || account_hit
}

/// 判定时刻是否落在生效时间窗内；无时间窗即不限时段。
pub(super) fn window_allows(spec: &PolicySpec, now_ms: u64) -> bool {
    spec.active_range.is_none_or(|range| range.contains(now_ms))
}

/// 编译全部规则模式：任一条失败即整体失败（配置缺陷一律拒绝，不做宽松解释）。
pub(super) fn compile_rules(rules: &[RuleItem]) -> Result<Vec<Pattern>, CompileError> {
    rules
        .iter()
        .map(|rule| Pattern::compile(&rule.url_pattern))
        .collect()
}

/// 挑选命中的规则（规格 §2.4：精确规则 > 通配规则；同精度时按规则序）。
///
/// 候选按 `spec.rules` 下标升序遍历；[`more_specific`] 返回 `false` 时保留先出现的
/// （下标小者）⇒ 同精度同字面锚点长度由规则序裁决。精度比较只走既有孤岛函数，
/// 不在这里重写字符串长度或通配符个数比较。
///
/// 返回 `compiled` 里的下标（与 `spec.rules` 同序），避免复制已编译模式。
pub(super) fn select_rule(compiled: &[Pattern], url: &NormalizedUrl) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, pattern) in compiled.iter().enumerate() {
        if !pattern.is_match(url) {
            continue;
        }
        best = match best {
            None => Some(index),
            Some(best_index) => {
                if more_specific(pattern, &compiled[best_index]) {
                    Some(index)
                } else {
                    Some(best_index)
                }
            }
        };
    }
    best
}
