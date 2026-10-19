//! argv 策略：宿主可执行名回退（basename fallback）、策略聚合与序列化形态。

#![allow(clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_execpolicy::argv::{
    MatchOptions, PatternToken, Policy, PrefixPattern, PrefixRule, RequirementsExecPolicy,
    RuleMatch, RuleRef,
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

fn resolve_options() -> MatchOptions {
    MatchOptions {
        resolve_host_executables: true,
    }
}

fn absolute(segments: &[&str]) -> String {
    let mut path = PathBuf::from("/");
    for segment in segments {
        path.push(segment);
    }
    path.to_string_lossy().into_owned()
}

// ---------- 宿主可执行名映射 ----------

#[test]
fn set_host_executable_paths_stores_mapping() {
    let mut policy = Policy::empty();
    policy.set_host_executable_paths(
        "git".to_string(),
        vec![PathBuf::from(absolute(&["usr", "bin", "git"]))],
    );
    let paths = policy
        .host_executables()
        .get("git")
        .expect("missing git host executable");
    assert_eq!(
        paths.as_ref(),
        [PathBuf::from(absolute(&["usr", "bin", "git"]))]
    );
}

#[test]
fn host_executable_resolution_uses_basename_rule_when_allowed() {
    let git_path = absolute(&["usr", "bin", "git"]);
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Ask)
        .expect("valid prefix");
    policy.set_host_executable_paths("git".to_string(), vec![PathBuf::from(&git_path)]);

    let evaluation = policy.check_with_options(
        &[git_path.clone(), "status".to_string()],
        &ask_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation,
        agent_execpolicy::argv::Evaluation {
            decision: Decision::Ask,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["git", "status"]),
                decision: Decision::Ask,
                resolved_program: Some(PathBuf::from(&git_path)),
                justification: None,
            }],
        }
    );
}

#[test]
fn host_executable_resolution_respects_explicit_empty_allowlist() {
    // 显式空允许清单：basename 回退被禁用 ⇒ 不命中。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");
    policy.set_host_executable_paths("git".to_string(), Vec::new());
    let git_path = absolute(&["usr", "bin", "git"]);

    let evaluation = policy.check_with_options(
        &[git_path.clone(), "status".to_string()],
        &allow_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation,
        agent_execpolicy::argv::Evaluation {
            decision: Decision::Allow,
            matched_rules: vec![RuleMatch::HeuristicsRuleMatch {
                command: vec![git_path, "status".to_string()],
                decision: Decision::Allow,
            }],
        }
    );
}

#[test]
fn host_executable_resolution_ignores_path_not_in_allowlist() {
    let allowed_git = absolute(&["usr", "bin", "git"]);
    let other_git = absolute(&["opt", "homebrew", "bin", "git"]);
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");
    policy.set_host_executable_paths("git".to_string(), vec![PathBuf::from(&allowed_git)]);

    let evaluation = policy.check_with_options(
        &[other_git.clone(), "status".to_string()],
        &allow_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation,
        agent_execpolicy::argv::Evaluation {
            decision: Decision::Allow,
            matched_rules: vec![RuleMatch::HeuristicsRuleMatch {
                command: vec![other_git, "status".to_string()],
                decision: Decision::Allow,
            }],
        }
    );
}

#[test]
fn host_executable_resolution_falls_back_without_mapping() {
    // 无 host_executable 条目 ⇒ basename 回退放行（对照参考语义）。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");
    let git_path = absolute(&["usr", "bin", "git"]);

    let evaluation = policy.check_with_options(
        &[git_path.clone(), "status".to_string()],
        &ask_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation,
        agent_execpolicy::argv::Evaluation {
            decision: Decision::Ask,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: tokens(&["git"]),
                decision: Decision::Ask,
                resolved_program: Some(PathBuf::from(&git_path)),
                justification: None,
            }],
        }
    );
}

#[test]
fn host_executable_resolution_does_not_override_exact_match() {
    // 精确首 token 命中优先，basename 回退不参与（解析路径不重复计规则）。
    let git_path = absolute(&["usr", "bin", "git"]);
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(std::slice::from_ref(&git_path), Decision::Allow)
        .expect("valid prefix");
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");
    policy.set_host_executable_paths("git".to_string(), vec![PathBuf::from(&git_path)]);

    let evaluation = policy.check_with_options(
        &[git_path.clone(), "status".to_string()],
        &ask_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation,
        agent_execpolicy::argv::Evaluation {
            decision: Decision::Allow,
            matched_rules: vec![RuleMatch::PrefixRuleMatch {
                matched_prefix: vec![git_path],
                decision: Decision::Allow,
                resolved_program: None,
                justification: None,
            }],
        }
    );
}

#[test]
fn host_executable_resolution_requires_absolute_program() {
    // 相对路径不做 basename 回退（AbsolutePathBuf 语义：非绝对即放弃）。
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git"]), Decision::Ask)
        .expect("valid prefix");

    let evaluation = policy.check_with_options(
        &[format!("./{}", "git"), "status".to_string()],
        &ask_all,
        &resolve_options(),
    );
    assert_eq!(
        evaluation.matched_rules,
        vec![RuleMatch::HeuristicsRuleMatch {
            command: vec!["./git".to_string(), "status".to_string()],
            decision: Decision::Ask,
        }]
    );
}

#[test]
fn basename_resolution_disabled_by_default_options() {
    // 默认 MatchOptions：不解析宿主可执行，绝对路径只按整 token 精确匹配。
    let git_path = absolute(&["usr", "bin", "git"]);
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Ask)
        .expect("valid prefix");

    let evaluation = policy.check_with_options(
        &[git_path, "status".to_string()],
        &ask_all,
        &MatchOptions::default(),
    );
    assert_eq!(evaluation.decision, Decision::Ask);
    assert!(!evaluation.is_match());
}

// ---------- 允许前缀导出与聚合 ----------

#[test]
fn get_allowed_prefixes_renders_sorted_deduped_prefixes() {
    let alts_install = PrefixRule {
        pattern: PrefixPattern {
            first: Arc::from("npm"),
            rest: vec![PatternToken::Alts(vec![
                "i".to_string(),
                "install".to_string(),
            ])]
            .into(),
        },
        decision: Decision::Allow,
        justification: None,
    };
    let mut map: HashMap<String, Vec<RuleRef>> = HashMap::new();
    map.insert("npm".to_string(), vec![Arc::new(alts_install)]);
    let mut policy = Policy::new(map);
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    policy
        .add_prefix_rule(&tokens(&["rm", "-rf"]), Decision::Deny)
        .expect("valid prefix");

    assert_eq!(
        policy.get_allowed_prefixes(),
        vec![
            tokens(&["git", "status"]),
            vec!["npm".to_string(), "[i|install]".to_string()],
        ]
    );
}

#[test]
fn merge_overlay_appends_rules_and_replaces_host_mappings() {
    let mut base = Policy::empty();
    base.add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    base.set_host_executable_paths("git".to_string(), vec![PathBuf::from("/old/git")]);

    let mut overlay = Policy::empty();
    overlay
        .add_prefix_rule(&tokens(&["git", "commit"]), Decision::Deny)
        .expect("valid prefix");
    overlay.set_host_executable_paths("git".to_string(), vec![PathBuf::from("/new/git")]);

    let merged = base.merge_overlay(&overlay);
    assert!(merged.rules().contains_key("git"));
    let evaluation = merged.check(&tokens(&["git", "commit", "-m", "x"]), &ask_all);
    assert_eq!(evaluation.decision, Decision::Deny);
    assert_eq!(
        merged
            .host_executables()
            .get("git")
            .map(|paths| paths.as_ref()),
        Some([PathBuf::from("/new/git")].as_slice())
    );
    // 原策略不受影响。
    assert!(base.rules().get("git").expect("git").len() == 1);
}

#[test]
fn requirements_policy_equality_is_order_independent() {
    let build_a = || {
        let mut policy = Policy::empty();
        policy
            .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
            .expect("valid prefix");
        policy
            .add_prefix_rule(&tokens(&["ls"]), Decision::Ask)
            .expect("valid prefix");
        policy
    };
    let build_b = || {
        let mut policy = Policy::empty();
        policy
            .add_prefix_rule(&tokens(&["ls"]), Decision::Ask)
            .expect("valid prefix");
        policy
            .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
            .expect("valid prefix");
        policy
    };
    assert_eq!(
        RequirementsExecPolicy::new(build_a()),
        RequirementsExecPolicy::new(build_b())
    );
    let mut different = Policy::empty();
    different
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Deny)
        .expect("valid prefix");
    assert_ne!(
        RequirementsExecPolicy::new(build_a()),
        RequirementsExecPolicy::new(different)
    );
}

// ---------- 序列化形态 ----------

#[test]
fn evaluation_serializes_reference_json_shape() {
    let mut policy = Policy::empty();
    policy
        .add_prefix_rule(&tokens(&["git", "status"]), Decision::Allow)
        .expect("valid prefix");
    let evaluation = policy.check(&tokens(&["git", "status"]), &ask_all);

    let json = serde_json::to_string(&evaluation).expect("serialize");
    assert!(
        json.contains(
            r#""matchedRules":[{"prefixRuleMatch":{"matchedPrefix":["git","status"],"decision":"allow"}}"#,
        ),
        "json={json}"
    );
    assert!(json.contains(r#""decision":"allow""#), "json={json}");

    let back: agent_execpolicy::argv::Evaluation =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, evaluation);
}

#[test]
fn optional_fields_are_omitted_and_heuristics_shape_round_trips() {
    let miss = Policy::empty().check(&tokens(&["python"]), &ask_all);
    let json = serde_json::to_string(&miss).expect("serialize");
    assert_eq!(
        json,
        r#"{"decision":"ask","matchedRules":[{"heuristicsRuleMatch":{"command":["python"],"decision":"ask"}}]}"#
    );
    let back: agent_execpolicy::argv::Evaluation =
        serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, miss);
}

#[test]
fn basename_resolution_ignores_directory_form() {
    // basename 提取走 file_name：目录部分不参与规则键，仅文件名命中。
    let raw = absolute(&["usr", "local", "bin", "git"]);
    let path = Path::new(raw.as_str());
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("git"));
}
