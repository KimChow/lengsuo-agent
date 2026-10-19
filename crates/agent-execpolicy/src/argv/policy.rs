//! argv 策略集合与评估（对照参考 policy.rs 转写）。
//!
//! 与参考的差异（详见 docs/w3-policy-command-argv-status.md 偏离清单）：
//! - 规则多映射用 `HashMap<String, Vec<RuleRef>>` 等价实现（同 program 保插入序，
//!   聚合输出处均显式排序，可观察行为不变）；未引入第三方 multimap 依赖。
//! - 绝对路径用 `std::path` 表达（无独立新类型）：回退前显式 `is_absolute()` 校验。
//! - 未搬 network_rule 相关字段与 `compiled_network_domains`（网络仍走既有字符串版
//!   [`crate::Policy`] 的 network 段，本模块只做进程 argv 评估）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::argv::executable_name::executable_path_lookup_key;
use crate::argv::rule::{PatternToken, PrefixPattern, PrefixRule, RuleMatch, RuleRef};
use crate::argv::Error;
use crate::types::Decision;

/// 无命中时由调用方提供的启发式回退（`None` ⇒ 未命中返回空命中列表）。
type HeuristicsFallback<'a> = Option<&'a dyn Fn(&[String]) -> Decision>;

/// 匹配开关：是否允许绝对路径程序经 basename 回退命中同名规则。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MatchOptions {
    pub resolve_host_executables: bool,
}

#[derive(Clone, Debug)]
pub struct Policy {
    rules_by_program: HashMap<String, Vec<RuleRef>>,
    host_executables_by_name: HashMap<String, Arc<[PathBuf]>>,
}

/// 需求方持有的执行策略视图：相等性与插入顺序无关（按指纹比较）。
#[derive(Debug, Clone)]
pub struct RequirementsExecPolicy {
    policy: Policy,
}

impl RequirementsExecPolicy {
    pub fn new(policy: Policy) -> Self {
        Self { policy }
    }

    pub fn fingerprint(&self) -> Vec<String> {
        let mut entries = Vec::new();
        for (program, rules) in self.policy.rules_by_program.iter() {
            for rule in rules {
                entries.push(format!("{program}:{rule:?}"));
            }
        }
        entries.sort();
        entries
    }
}

impl PartialEq for RequirementsExecPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint() == other.fingerprint()
    }
}

impl Eq for RequirementsExecPolicy {}

impl AsRef<Policy> for RequirementsExecPolicy {
    fn as_ref(&self) -> &Policy {
        &self.policy
    }
}

impl Policy {
    pub fn new(rules_by_program: HashMap<String, Vec<RuleRef>>) -> Self {
        Self::from_parts(rules_by_program, HashMap::new())
    }

    pub fn from_parts(
        rules_by_program: HashMap<String, Vec<RuleRef>>,
        host_executables_by_name: HashMap<String, Arc<[PathBuf]>>,
    ) -> Self {
        Self {
            rules_by_program,
            host_executables_by_name,
        }
    }

    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }

    pub fn rules(&self) -> &HashMap<String, Vec<RuleRef>> {
        &self.rules_by_program
    }

    pub fn host_executables(&self) -> &HashMap<String, Arc<[PathBuf]>> {
        &self.host_executables_by_name
    }

    /// 全部 Allow 前缀的渲染（多选一 token 渲染为 `[a|b]`），排序去重。
    pub fn get_allowed_prefixes(&self) -> Vec<Vec<String>> {
        let mut prefixes = Vec::new();

        for rules in self.rules_by_program.values() {
            for rule in rules {
                let Some(prefix_rule) = rule.as_any().downcast_ref::<PrefixRule>() else {
                    continue;
                };
                if prefix_rule.decision != Decision::Allow {
                    continue;
                }

                let mut prefix: Vec<String> =
                    Vec::with_capacity(prefix_rule.pattern.rest.len() + 1);
                prefix.push(prefix_rule.pattern.first.as_ref().to_string());
                prefix.extend(prefix_rule.pattern.rest.iter().map(render_pattern_token));
                prefixes.push(prefix);
            }
        }

        prefixes.sort();
        prefixes.dedup();
        prefixes
    }

    /// 追加一条逐 token 的前缀规则；空前缀报错（空模式不放行）。
    pub fn add_prefix_rule(&mut self, prefix: &[String], decision: Decision) -> Result<(), Error> {
        let (first_token, rest) = prefix
            .split_first()
            .ok_or_else(|| Error::InvalidPattern("prefix cannot be empty".to_string()))?;

        let rule: RuleRef = Arc::new(PrefixRule {
            pattern: PrefixPattern {
                first: Arc::from(first_token.as_str()),
                rest: rest
                    .iter()
                    .map(|token| PatternToken::Single(token.clone()))
                    .collect::<Vec<_>>()
                    .into(),
            },
            decision,
            justification: None,
        });

        self.rules_by_program
            .entry(first_token.clone())
            .or_default()
            .push(rule);
        Ok(())
    }

    /// 设置某可执行名允许回退命中的绝对路径清单（后设覆盖前设）。
    pub fn set_host_executable_paths(&mut self, name: String, paths: Vec<PathBuf>) {
        self.host_executables_by_name.insert(name, paths.into());
    }

    /// 合并 overlay：规则追加、宿主可执行映射覆盖（后者的条目胜出）。
    pub fn merge_overlay(&self, overlay: &Policy) -> Policy {
        let mut combined_rules = self.rules_by_program.clone();
        for (program, rules) in overlay.rules_by_program.iter() {
            let entry = combined_rules.entry(program.clone()).or_default();
            entry.extend(rules.iter().cloned());
        }

        let mut host_executables_by_name = self.host_executables_by_name.clone();
        host_executables_by_name.extend(
            overlay
                .host_executables_by_name
                .iter()
                .map(|(name, paths)| (name.clone(), paths.clone())),
        );

        Policy::from_parts(combined_rules, host_executables_by_name)
    }

    pub fn check<F>(&self, cmd: &[String], heuristics_fallback: &F) -> Evaluation
    where
        F: Fn(&[String]) -> Decision,
    {
        let matched_rules = self.matches_for_command_with_options(
            cmd,
            Some(heuristics_fallback),
            &MatchOptions::default(),
        );
        Evaluation::from_matches(matched_rules)
    }

    pub fn check_with_options<F>(
        &self,
        cmd: &[String],
        heuristics_fallback: &F,
        options: &MatchOptions,
    ) -> Evaluation
    where
        F: Fn(&[String]) -> Decision,
    {
        let matched_rules =
            self.matches_for_command_with_options(cmd, Some(heuristics_fallback), options);
        Evaluation::from_matches(matched_rules)
    }

    /// 评估多条命令并聚合（整体决策 = 各命令全部命中的最严者）。
    pub fn check_multiple<Commands, F>(
        &self,
        commands: Commands,
        heuristics_fallback: &F,
    ) -> Evaluation
    where
        Commands: IntoIterator,
        Commands::Item: AsRef<[String]>,
        F: Fn(&[String]) -> Decision,
    {
        self.check_multiple_with_options(commands, heuristics_fallback, &MatchOptions::default())
    }

    pub fn check_multiple_with_options<Commands, F>(
        &self,
        commands: Commands,
        heuristics_fallback: &F,
        options: &MatchOptions,
    ) -> Evaluation
    where
        Commands: IntoIterator,
        Commands::Item: AsRef<[String]>,
        F: Fn(&[String]) -> Decision,
    {
        let matched_rules: Vec<RuleMatch> = commands
            .into_iter()
            .flat_map(|command| {
                self.matches_for_command_with_options(
                    command.as_ref(),
                    Some(heuristics_fallback),
                    options,
                )
            })
            .collect();

        Evaluation::from_matches(matched_rules)
    }

    /// 返回命令命中的全部规则。无命中且提供 `heuristics_fallback` 时，返回单条
    /// [`RuleMatch::HeuristicsRuleMatch`]（其决策由回退闭包给出）；
    /// 因此提供回退时返回值恒非空。
    pub fn matches_for_command(
        &self,
        cmd: &[String],
        heuristics_fallback: HeuristicsFallback<'_>,
    ) -> Vec<RuleMatch> {
        self.matches_for_command_with_options(cmd, heuristics_fallback, &MatchOptions::default())
    }

    pub fn matches_for_command_with_options(
        &self,
        cmd: &[String],
        heuristics_fallback: HeuristicsFallback<'_>,
        options: &MatchOptions,
    ) -> Vec<RuleMatch> {
        let matched_rules = self
            .match_exact_rules(cmd)
            .filter(|matched_rules| !matched_rules.is_empty())
            .or_else(|| {
                options
                    .resolve_host_executables
                    .then(|| self.match_host_executable_rules(cmd))
                    .filter(|matched_rules| !matched_rules.is_empty())
            })
            .unwrap_or_default();

        if matched_rules.is_empty() {
            if let Some(heuristics_fallback) = heuristics_fallback {
                return vec![RuleMatch::HeuristicsRuleMatch {
                    command: cmd.to_vec(),
                    decision: heuristics_fallback(cmd),
                }];
            }
        }
        matched_rules
    }

    /// 精确首 token 匹配；`None` 仅表示命令本身为空。
    fn match_exact_rules(&self, cmd: &[String]) -> Option<Vec<RuleMatch>> {
        let first = cmd.first()?;
        Some(
            self.rules_by_program
                .get(first)
                .map(|rules| rules.iter().filter_map(|rule| rule.matches(cmd)).collect())
                .unwrap_or_default(),
        )
    }

    /// basename 回退：程序须为绝对路径；有宿主可执行映射时仅清单内路径可回退。
    fn match_host_executable_rules(&self, cmd: &[String]) -> Vec<RuleMatch> {
        let Some(first) = cmd.first() else {
            return Vec::new();
        };
        let program = Path::new(first);
        if !program.is_absolute() {
            return Vec::new();
        }
        let Some(basename) = executable_path_lookup_key(program) else {
            return Vec::new();
        };
        let Some(rules) = self.rules_by_program.get(&basename) else {
            return Vec::new();
        };
        if let Some(paths) = self.host_executables_by_name.get(&basename) {
            if !paths.iter().any(|path| path.as_path() == program) {
                return Vec::new();
            }
        }

        let basename_command = std::iter::once(basename)
            .chain(cmd.iter().skip(1).cloned())
            .collect::<Vec<_>>();
        rules
            .iter()
            .filter_map(|rule| rule.matches(&basename_command))
            .map(|rule_match| rule_match.with_resolved_program(program))
            .collect()
    }
}

fn render_pattern_token(token: &PatternToken) -> String {
    match token {
        PatternToken::Single(value) => value.clone(),
        PatternToken::Alts(alternatives) => format!("[{}]", alternatives.join("|")),
    }
}

/// 一次评估的结论：聚合决策 + 全部命中规则。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evaluation {
    pub decision: Decision,
    #[serde(rename = "matchedRules")]
    pub matched_rules: Vec<RuleMatch>,
}

impl Evaluation {
    /// 命中列表里是否存在真实规则（启发式回退不计入）。
    pub fn is_match(&self) -> bool {
        self.matched_rules
            .iter()
            .any(|rule_match| !matches!(rule_match, RuleMatch::HeuristicsRuleMatch { .. }))
    }

    /// 调用方须保证 `matched_rules` 非空；聚合取最严决策（严格度序的 `max`）。
    fn from_matches(matched_rules: Vec<RuleMatch>) -> Self {
        let decision = matched_rules.iter().map(RuleMatch::decision).max();
        let decision = decision.expect("invariant failed: matched_rules must be non-empty");

        Self {
            decision,
            matched_rules,
        }
    }
}
