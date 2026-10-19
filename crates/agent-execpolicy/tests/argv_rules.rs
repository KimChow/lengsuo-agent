//! argv 策略：规则构造形态（justification、多选一 token、尾部别名不笛卡尔展开）。

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use agent_execpolicy::argv::{
    Evaluation, PatternToken, Policy, PrefixPattern, PrefixRule, RuleMatch, RuleRef,
};
use agent_execpolicy::Decision;

fn tokens(cmd: &[&str]) -> Vec<String> {
    cmd.iter().map(|s| s.to_string()).collect()
}

fn allow_all(_: &[String]) -> Decision {
    Decision::Allow
}

fn ask_all(_: &[String]) -> Decision {
    Decision::Ask
}

fn allow(first: &str, rest: &[&str]) -> PrefixRule {
    PrefixRule {
        pattern: PrefixPattern {
            first: Arc::from(first),
            rest: rest
                .iter()
                .map(|token| PatternToken::Single(token.to_string()))
                .collect::<Vec<_>>()
                .into(),
        },
        decision: Decision::Allow,
        justification: None,
    }
}

/// 按规则顺序组装策略（同 program 的多条规则保持插入序，对照参考多文件合并语义）。
fn policy_with(rules: Vec<(&str, Vec<PrefixRule>)>) -> Policy {
    let mut map: HashMap<String, Vec<RuleRef>> = HashMap::new();
    for (program, program_rules) in rules {
        let entry = map.entry(program.to_string()).or_default();
        for rule in program_rules {
            let rule_ref: RuleRef = Arc::new(rule);
            entry.push(rule_ref);
        }
    }
    Policy::new(map)
}

#[test]
fn justification_is_attached_to_denied_matches() {
    let mut rule = allow("rm", &[]);
    rule.decision = Decision::Deny;
    rule.justification = Some("destructive command".to_string());
    let policy = policy_with(vec![("rm", vec![rule])]);
    let evaluation = policy.check(
        &tokens(&["rm", "-rf", "/some/important/folder"]),
        &allow_all,
    );
    assert_eq!(
        Evaluation {
            decision: Decision::Deny,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["rm"]),
                decision: Decision::Deny,
                resolved_program: None,
                justification: Some("destructive command".to_string()),
            }],
        },
        evaluation
    );
}

#[test]
fn justification_can_be_used_with_allow_decision() {
    let mut rule = allow("ls", &[]);
    rule.justification = Some("safe and commonly used".to_string());
    let policy = policy_with(vec![("ls", vec![rule])]);
    let evaluation = policy.check(&tokens(&["ls", "-l"]), &ask_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Allow,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["ls"]),
                decision: Decision::Allow,
                resolved_program: None,
                justification: Some("safe and commonly used".to_string()),
            }],
        },
        evaluation
    );
}

#[test]
fn only_first_token_alias_expands_to_multiple_rules() {
    // 多选一 token（对照参考 parser 展开后的形态）：[["bash","sh"],["-c","-l"]]。
    let alts = |first: &str| PrefixRule {
        pattern: PrefixPattern {
            first: Arc::from(first),
            rest: vec![PatternToken::Alts(vec!["-c".to_string(), "-l".to_string()])].into(),
        },
        decision: Decision::Allow,
        justification: None,
    };
    let policy = policy_with(vec![("bash", vec![alts("bash")]), ("sh", vec![alts("sh")])]);
    assert_eq!(
        policy
            .check(&tokens(&["bash", "-c", "echo", "hi"]), &ask_all)
            .decision,
        Decision::Allow
    );
    assert_eq!(
        policy
            .check(&tokens(&["sh", "-l", "echo", "hi"]), &ask_all)
            .decision,
        Decision::Allow
    );
    // 交替项不匹配 ⇒ 不命中。
    assert_eq!(
        policy
            .check(&tokens(&["bash", "-x", "echo", "hi"]), &ask_all)
            .decision,
        Decision::Ask
    );
}

#[test]
fn tail_aliases_are_not_cartesian_expanded() {
    // 尾部多选一逐位匹配，不做笛卡尔展开；matched_prefix 截到规则长度。
    let rule = PrefixRule {
        pattern: PrefixPattern {
            first: Arc::from("npm"),
            rest: vec![
                PatternToken::Alts(vec!["i".to_string(), "install".to_string()]),
                PatternToken::Alts(vec![
                    "--legacy-peer-deps".to_string(),
                    "--no-save".to_string(),
                ]),
            ]
            .into(),
        },
        decision: Decision::Allow,
        justification: None,
    };
    let policy = policy_with(vec![("npm", vec![rule])]);

    let npm_i = policy.check(&tokens(&["npm", "i", "--legacy-peer-deps"]), &ask_all);
    assert_eq!(
        npm_i.matched_rules,
        vec![RuleMatch::PrefixRuleMatch {
            matched_prefix: tokens(&["npm", "i", "--legacy-peer-deps"]),
            decision: Decision::Allow,
            resolved_program: None,
            justification: None,
        }]
    );

    let npm_install = policy.check(
        &tokens(&["npm", "install", "--no-save", "leftpad"]),
        &ask_all,
    );
    assert_eq!(
        npm_install.matched_rules,
        vec![RuleMatch::PrefixRuleMatch {
            matched_prefix: tokens(&["npm", "install", "--no-save"]),
            decision: Decision::Allow,
            resolved_program: None,
            justification: None,
        }]
    );
}
