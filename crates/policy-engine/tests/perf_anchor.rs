//! 性能锚：1000 规则 × 单次 evaluate < 1ms（SPEC §3「验收边界」最后一条）。
//!
//! 本卡守的是「复杂度没退化」（O(n²) 会直接冲破 10ms），不是精确性能验收。
//! 1ms 是 SPEC 在参考机上的锚；CI 机器负载会抖动，若把 1ms 写进 assert!
//! 会随机红，故硬断言用宽松上界 10ms，同时 eprintln! 打印实测平均值供人工比对。
//! 若要精确锚，走 benches（不在本卡范围）。

use std::time::Instant;

use policy_engine::evaluate::{evaluate, EvaluateRequest};
use policy_engine::model::{Action, PolicyMode, PolicySpec, RuleItem, TargetContext, Targets};

const TARGET_USER: &str = "u1";
const NOW_MS: u64 = 1_000;
const RULE_COUNT: usize = 1000;
const WARMUP_RUNS: usize = 10;
const TIMED_RUNS: usize = 20;
const CI_UPPER_MS: f64 = 10.0;

/// 生效对象：只带 user_ids 一维（与 tests/evaluate_surface/main.rs 的 targets_of 一致）。
fn targets() -> Targets {
    Targets {
        user_ids: vec![TARGET_USER.to_string()],
        ..Targets::default()
    }
}

/// 与 [`targets`] 对应的访问者上下文。
fn ctx() -> TargetContext {
    TargetContext::new(TARGET_USER)
}

/// 构造近似真实的 1000 条规则：通配 host + 通配 path + 精确 host 混合，
/// `actions.access` 命中与不命中各半（is_accessible 按奇偶交替），
/// 避免「全部首条命中」把 O(n) 退化成 O(1) 的假样本。
fn build_rules() -> Vec<RuleItem> {
    let mut rules = Vec::with_capacity(RULE_COUNT);
    for i in 0..RULE_COUNT {
        let pattern = match i % 3 {
            // 通配 host：任意单段主机下的固定路径
            0 => format!("*.example.com/path{}", i),
            // 通配 path：固定主机下的任意单段路径
            1 => format!("host{}.example.com/*", i),
            // 精确 host + 精确 path
            _ => format!("exact{}.example.com/exact/{}", i, i),
        };
        // 偶数为白名单（可访问、开放 access），奇数为黑名单（不可访问、全拒）
        if i % 2 == 0 {
            rules.push(RuleItem::allow(pattern));
        } else {
            rules.push(RuleItem::deny(pattern));
        }
    }
    rules
}

/// 构造 1000 条规则的策略（SomeWhite：默认拒，命中白名单规则才允许）。
fn build_spec() -> PolicySpec {
    PolicySpec::new(
        "p-perf",
        "性能锚策略",
        PolicyMode::SomeWhite,
        targets(),
        build_rules(),
    )
}

/// 命中请求：访问 host4.example.com/a，命中规则 #4（host4.example.com/*，白名单）。
fn hit_request() -> EvaluateRequest {
    EvaluateRequest {
        url: "https://host4.example.com/a".to_string(),
        action: Action::Access,
        now_ms: NOW_MS,
        targets_ctx: ctx(),
        selector: None,
    }
}

/// 平均耗时（ms），≥20 次计时取平均，≥10 次预热后再计时。
fn average_ms(spec: &PolicySpec) -> f64 {
    for _ in 0..WARMUP_RUNS {
        let _ = evaluate(spec, hit_request());
    }
    let mut total_ns = 0u128;
    for _ in 0..TIMED_RUNS {
        let start = Instant::now();
        let _ = evaluate(spec, hit_request());
        total_ns += start.elapsed().as_nanos();
    }
    total_ns as f64 / TIMED_RUNS as f64 / 1_000_000.0
}

#[test]
fn perf_anchor_1000_rules_under_10ms() {
    let spec = build_spec();
    // 规模自证：防空样本，构造失败时测试不得绿。
    assert_eq!(
        spec.rules.len(),
        RULE_COUNT,
        "规则数不是 1000，测试将是假绿"
    );

    let avg_ms = average_ms(&spec);

    eprintln!(
        "性能锚：1000 规则 × 单次 evaluate 平均 {:.3} ms（SPEC §3 锚：1ms，CI 上界：10ms）",
        avg_ms
    );

    assert!(
        avg_ms < CI_UPPER_MS,
        "evaluate 平均耗时 {:.3} ms 超过 10ms 上界，疑似复杂度退化（O(n²）)",
        avg_ms
    );
}

#[test]
fn perf_anchor_scale_and_hit_self_certify() {
    let spec = build_spec();
    assert_eq!(spec.rules.len(), RULE_COUNT);

    // 1000 条规则里挑一条靠后的白名单规则做命中判定，证明不是
    // 「全部规则都不匹配就快速返回」的空转。
    let rule_idx = RULE_COUNT - 2; // 998，偶数 ⇒ 白名单，exact998.example.com/exact/998
    let url = format!("https://exact{}.example.com/exact/{}", rule_idx, rule_idx);
    let req = EvaluateRequest {
        url,
        action: Action::Access,
        now_ms: NOW_MS,
        targets_ctx: ctx(),
        selector: None,
    };
    let decision = evaluate(&spec, req);
    assert!(
        decision.allowed,
        "靠后规则 #{rule_idx} 应命中并放行，证明评估不是空转"
    );
    assert_eq!(
        decision.matched_rule.map(|id| id.index()),
        Some(rule_idx),
        "靠后规则 #{rule_idx} 应被选中为命中规则"
    );
}

#[test]
fn perf_anchor_access_actions_half_hit_half_miss() {
    // 验证构造写法：1000 条规则里 access 开关一半开启（白名单）、一半关闭（黑名单）。
    let spec = build_spec();
    let access_on = spec
        .rules
        .iter()
        .filter(|r| r.is_accessible && r.actions.enabled(Action::Access))
        .count();
    let access_off = spec.rules.len() - access_on;
    assert_eq!(access_on, RULE_COUNT / 2, "白名单 access 规则应占一半");
    assert_eq!(access_off, RULE_COUNT / 2, "黑名单 access 规则应占一半");
}
