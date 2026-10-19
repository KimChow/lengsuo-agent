//! argv 级进程策略评估（对照参考实现 tests/basic.rs 的前缀规则子集转写）。
//!
//! 决策映射：参考 allow/prompt/forbidden ⇒ 本仓 allow/ask/deny；聚合语义不变：
//! 所有命中规则取最严决策（参考 strictest-wins），未命中走启发式回退（本卡约定恒 Ask）。

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;

use agent_execpolicy::argv::{
    Error, Evaluation, PatternToken, Policy, PrefixPattern, PrefixRule, RuleMatch, RuleRef,
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

fn deny(first: &str, rest: &[&str]) -> PrefixRule {
    let mut rule = allow(first, rest);
    rule.decision = Decision::Deny;
    rule
}

fn ask(first: &str, rest: &[&str]) -> PrefixRule {
    let mut rule = allow(first, rest);
    rule.decision = Decision::Ask;
    rule
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
fn basic_match() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    let evaluation = policy.check(&tokens(&["git", "status"]), &allow_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Allow,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["git", "status"]),
                decision: Decision::Allow,
                resolved_program: None,
                justification: None,
            }],
        },
        evaluation
    );
}

#[test]
fn git_status_matches_but_statusx_does_not() {
    // git statusx 不是 git status：token 级精确比较，不做子串匹配 ⇒ Ask。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    assert_eq!(
        policy.check(&tokens(&["git", "status"]), &ask_all).decision,
        Decision::Allow
    );
    let miss = policy.check(&tokens(&["git", "statusx"]), &ask_all);
    assert_eq!(miss.decision, Decision::Ask);
    assert!(!miss.is_match());
    assert_eq!(
        miss.matched_rules,
        vec![RuleMatch::HeuristicsRuleMatch {
            command: tokens(&["git", "statusx"]),
            decision: Decision::Ask,
        }]
    );
}

#[test]
fn command_longer_than_pattern_matches_with_truncated_prefix() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    let evaluation = policy.check(&tokens(&["git", "status", "--branch"]), &ask_all);
    assert_eq!(evaluation.decision, Decision::Allow);
    assert_eq!(
        evaluation.matched_rules,
        vec![RuleMatch::PrefixRuleMatch {
            matched_prefix: tokens(&["git", "status"]),
            decision: Decision::Allow,
            resolved_program: None,
            justification: None,
        }]
    );
}

#[test]
fn pattern_longer_than_command_does_not_match() {
    // 参数边界：规则要求 --porcelain，命令没给 ⇒ 不命中。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status", "--porcelain"]), Decision::Allow)
        .expect("valid prefix");
    let miss = policy.check(&tokens(&["git", "status"]), &ask_all);
    assert_eq!(miss.decision, Decision::Ask);
    assert!(!miss.is_match());
}

#[test]
fn spaced_argument_is_single_token() {
    // 带空格的单参数：token 整体比较；把命令整串按空格再切分会从命中变不命中。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["echo", "hello world"]), Decision::Allow)
        .expect("valid prefix");
    assert_eq!(
        policy
            .check(&tokens(&["echo", "hello world"]), &ask_all)
            .decision,
        Decision::Allow
    );
    assert_eq!(
        policy
            .check(&tokens(&["echo", "hello", "world"]), &ask_all)
            .decision,
        Decision::Ask
    );
}

#[test]
fn add_prefix_rule_extends_policy() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["ls", "-l"]), Decision::Ask)
        .expect("valid prefix");

    let rules = policy
        .rules()
        .get("ls")
        .unwrap_or_else(|| panic!("missing ls rules"));
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].program(), "ls");

    let evaluation = policy.check(&tokens(&["ls", "-l", "/some/important/folder"]), &allow_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Ask,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["ls", "-l"]),
                decision: Decision::Ask,
                resolved_program: None,
                justification: None,
            }],
        },
        evaluation
    );
}

#[test]
fn add_prefix_rule_rejects_empty_prefix() {
    let mut policy = Policy::empty();
    let error = policy.add_prefix_rule(&[], Decision::Allow).unwrap_err();
    assert_eq!(
        error,
        Error::InvalidPattern("prefix cannot be empty".to_string())
    );
}

#[test]
fn rules_for_same_program_keep_insertion_order() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");
    policy
        .add_prefix_rule(&tokens(&["git", "commit"]), Decision::Deny)
        .expect("valid prefix");
    let rules = policy
        .rules()
        .get("git")
        .unwrap_or_else(|| panic!("missing git rules"));
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].program(), "git");
    assert_eq!(rules[1].program(), "git");
}

#[test]
fn strictest_decision_wins_across_matches() {
    // 同一命令命中多条规则：聚合取最严（deny 优先），命中列表保持规则序。
    let policy = policy_with(vec![
        ("git", vec![ask("git", &[])]),
        ("git", vec![deny("git", &["commit"])]),
    ]);
    let commit = policy.check(&tokens(&["git", "commit", "-m", "hi"]), &allow_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Deny,
            matched_rules: vec![
                RuleMatch::PrefixRuleMatch {
                    matched_prefix: tokens(&["git"]),
                    decision: Decision::Ask,
                    resolved_program: None,
                    justification: None,
                },
                RuleMatch::PrefixRuleMatch {
                    matched_prefix: tokens(&["git", "commit"]),
                    decision: Decision::Deny,
                    resolved_program: None,
                    justification: None,
                },
            ],
        },
        commit
    );
}

#[test]
fn earlier_allow_does_not_mask_later_deny() {
    // 卡面场景：先写 allow 再写 deny，deny 仍须生效（首命中语义不适用本 API）。
    let policy = policy_with(vec![
        ("git", vec![allow("git", &[])]),
        ("git", vec![deny("git", &["push", "--force"])]),
    ]);
    assert_eq!(
        policy
            .check(&tokens(&["git", "push", "--force", "origin"]), &ask_all)
            .decision,
        Decision::Deny
    );
    assert_eq!(
        policy.check(&tokens(&["git", "status"]), &ask_all).decision,
        Decision::Allow
    );
}

#[test]
fn ask_outranks_allow_when_both_match() {
    // 最严序 Allow < Ask < Deny：ask 命中不被更早的 allow 覆盖。
    let policy = policy_with(vec![
        ("git", vec![allow("git", &[])]),
        ("git", vec![ask("git", &["reset"])]),
    ]);
    assert_eq!(
        policy
            .check(&tokens(&["git", "reset", "--hard"]), &allow_all)
            .decision,
        Decision::Ask
    );
}

#[test]
fn strictest_decision_across_multiple_commands() {
    let policy = policy_with(vec![
        ("git", vec![ask("git", &[])]),
        ("git", vec![deny("git", &["commit"])]),
    ]);
    let commands = vec![
        tokens(&["git", "status"]),
        tokens(&["git", "commit", "-m", "hi"]),
    ];
    let evaluation = policy.check_multiple(&commands, &allow_all);
    assert_eq!(evaluation.decision, Decision::Deny);
    assert_eq!(evaluation.matched_rules.len(), 3);
}

#[test]
fn heuristics_match_is_returned_when_no_policy_matches() {
    // 不命中：走调用方启发式回退（本卡约定恒 Ask = fail-closed）。
    let policy = Policy::empty();
    let command = tokens(&["python"]);
    let evaluation = policy.check(&command, &ask_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Ask,
            matched_rules: vec![RuleMatch::HeuristicsRuleMatch {
                command: command.clone(),
                decision: Decision::Ask,
            }],
        },
        evaluation
    );
    assert!(!evaluation.is_match());
}

#[test]
fn empty_command_is_ask_and_matches_nothing() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Allow)
        .expect("valid prefix");
    // 空 argv 不命中任何前缀 ⇒ Ask。
    let evaluation = policy.check(&[], &ask_all);
    assert_eq!(
        Evaluation {
            decision: Decision::Ask,
            matched_rules: vec![RuleMatch::HeuristicsRuleMatch {
                command: Vec::new(),
                decision: Decision::Ask,
            }],
        },
        evaluation
    );
    // 无回退时：无命中 ⇒ 空命中列表（参考 matches_for_command(None) 语义）。
    assert!(policy.matches_for_command(&[], None).is_empty());
    assert!(policy
        .matches_for_command(&tokens(&["python"]), None)
        .is_empty());
    assert_eq!(policy.matches_for_command(&tokens(&["git"]), None).len(), 1);
}

#[test]
fn shell_wrapped_inner_commands_stay_unreviewed() {
    // shell -c / 脚本解释器嵌套：首 token 是外层解释器，内层命令不因外层规则放行。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    for cmd in [
        ["bash", "-c", "git status"],
        ["sh", "-c", "git status"],
        ["bash", "-lc", "git status && rm -rf /"],
        ["python3", "-c", "import os; os.system('git status')"],
    ] {
        let evaluation = policy.check(&tokens(&cmd), &ask_all);
        assert_eq!(evaluation.decision, Decision::Ask, "cmd={cmd:?}");
        assert!(!evaluation.is_match(), "cmd={cmd:?}");
    }
}

#[test]
fn explicit_shell_prefix_rule_matches_only_its_prefix() {
    // 参考语义：显式允许 bash -c 前缀时按前缀命中并截断；
    // 内层命令不解析、不计入 matched_prefix（解释范围止于前缀本身）。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["bash", "-c"]), Decision::Allow)
        .expect("valid prefix");
    let evaluation = policy.check(&tokens(&["bash", "-c", "git status"]), &ask_all);
    assert_eq!(evaluation.decision, Decision::Allow);
    assert_eq!(
        evaluation.matched_rules,
        vec![RuleMatch::PrefixRuleMatch {
            matched_prefix: tokens(&["bash", "-c"]),
            decision: Decision::Allow,
            resolved_program: None,
            justification: None,
        }]
    );
}
