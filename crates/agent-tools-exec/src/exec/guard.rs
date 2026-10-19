//! 命令策略判定层：把已分词 argv 交给 argv 策略评估，并归一为三值结论。

use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;

/// 命令策略判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny(String),
    NeedsApproval(String),
}

// 全部说明文本均为静态常量：不含被检查命令的任何 token，避免把参数值回显给上层。

/// 无真实规则命中（或 argv 为空）时的保守说明。
const UNMATCHED_REASON: &str = "命令未命中任何策略规则，需人工确认";

/// 命中 deny 规则时的说明。
const DENIED_REASON: &str = "命令被策略规则拒绝";

/// 命中 ask 规则时的说明。
const ASKED_REASON: &str = "策略规则要求人工确认";

/// 内层命令藏在解释器内联脚本里时的说明。
const INLINE_SCRIPT_REASON: &str = "内联脚本的内层命令不可见，需人工确认";

/// 对已分词 argv 做策略判定。
///
/// 语义：
/// - 用 `Policy::check(argv, &heuristics)` 取 `Evaluation.decision`；
/// - `Decision::Allow` → `Verdict::Allow`；
/// - `Decision::Deny`  → `Verdict::Deny(静态说明，不含参数值)`；
/// - `Decision::Ask`   → `Verdict::NeedsApproval(静态说明)`；
/// - 空 argv 或不含任何真实规则命中（`Evaluation::is_match() == false`）→ `NeedsApproval`；
/// - argv 形如 `sh -c <字符串>` / `bash -c <字符串>` / 解释器内联脚本
///   → 一律 `NeedsApproval`（内层命令不可见，不得因外层放行而放行）；
/// - heuristics 回退闭包按保守 `Decision::Ask` 返回，不放行。
pub fn evaluate(policy: &Policy, argv: &[String]) -> Verdict {
    // 保守回退：真实规则未命中时不放行，交由上层审批。
    let heuristics = |_cmd: &[String]| Decision::Ask;
    let evaluation = policy.check(argv, &heuristics);

    if argv.is_empty() || !evaluation.is_match() {
        return Verdict::NeedsApproval(UNMATCHED_REASON.to_string());
    }

    if hides_inner_command(argv) {
        return Verdict::NeedsApproval(INLINE_SCRIPT_REASON.to_string());
    }

    match evaluation.decision {
        Decision::Allow => Verdict::Allow,
        Decision::Deny => Verdict::Deny(DENIED_REASON.to_string()),
        Decision::Ask => Verdict::NeedsApproval(ASKED_REASON.to_string()),
    }
}

/// 内层命令解析器：其内联脚本文本无法在 argv 层面展开评估。
const INLINE_SCRIPT_INTERPRETERS: [&str; 8] = [
    "sh", "bash", "dash", "zsh", "ksh", "env", "python", "python3",
];

/// 内联脚本标志：`-c` 系列及 `python -` / `-` 从标准输入读脚本。
const INLINE_SCRIPT_FLAGS: [&str; 6] = ["-c", "-lc", "-ic", "-ec", "-xc", "-cl"];

/// 该 argv 是否把内层命令藏在解释器内联脚本里（外层的放行不适用于内层）。
fn hides_inner_command(argv: &[String]) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    // 程序名按路径末段比较：`/bin/bash -c …` 与 `bash -c …` 同样应对。
    let program = program.rsplit(['/', '\\']).next().unwrap_or(program);

    if !INLINE_SCRIPT_INTERPRETERS.contains(&program) {
        return false;
    }

    argv[1..].iter().any(|token| {
        INLINE_SCRIPT_FLAGS.contains(&token.as_str())
            // `python -` / `python3 -` 从标准输入读脚本，同样不可见。
            || ((program == "python" || program == "python3") && token == "-")
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use agent_execpolicy::argv::Policy;
    use agent_execpolicy::Decision;

    use super::{evaluate, Verdict};

    /// 测试专用 argv 构造。
    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    /// 测试专用策略构造：逐条追加前缀规则（与既有 argv 策略测试同写法）。
    fn policy_with(rules: &[(&[&str], Decision)]) -> Policy {
        let mut policy = Policy::empty();
        for (prefix, decision) in rules {
            let prefix: Vec<String> = argv(prefix);
            policy
                .add_prefix_rule(&prefix, *decision)
                .expect("前缀规则构造必须成功");
        }
        policy
    }

    fn allow_reason(verdict: &Verdict) -> Option<&str> {
        match verdict {
            Verdict::Deny(reason) | Verdict::NeedsApproval(reason) => Some(reason.as_str()),
            Verdict::Allow => None,
        }
    }

    /// 1. 空 argv → NeedsApproval。
    #[test]
    fn empty_argv_needs_approval() {
        let policy = policy_with(&[(&["git", "status"], Decision::Allow)]);
        assert!(matches!(evaluate(&policy, &[]), Verdict::NeedsApproval(_)));
    }

    /// 2. 无规则命中 → NeedsApproval。
    #[test]
    fn unmatched_command_needs_approval() {
        let policy = policy_with(&[(&["git", "status"], Decision::Allow)]);
        assert!(matches!(
            evaluate(&policy, &argv(&["cargo", "publish"])),
            Verdict::NeedsApproval(_)
        ));
    }

    /// 3. 只命中 allow 规则 → Allow。
    #[test]
    fn allow_only_match_allows() {
        let policy = policy_with(&[(&["git", "status"], Decision::Allow)]);
        assert_eq!(
            evaluate(&policy, &argv(&["git", "status", "--short"])),
            Verdict::Allow
        );
    }

    /// 4. 只命中 deny 规则 → Deny。
    #[test]
    fn deny_only_match_denies() {
        let policy = policy_with(&[(&["rm", "-rf"], Decision::Deny)]);
        assert!(matches!(
            evaluate(&policy, &argv(&["rm", "-rf", "/tmp/x"])),
            Verdict::Deny(_)
        ));
    }

    /// 5. 同命令同时命中 allow 与 deny → Deny（最严优先，allow 不掩盖 deny）。
    #[test]
    fn deny_wins_over_allow_on_same_command() {
        let policy = policy_with(&[
            (&["git"], Decision::Allow),
            (&["git", "push"], Decision::Deny),
        ]);
        assert!(matches!(
            evaluate(&policy, &argv(&["git", "push", "origin", "main"])),
            Verdict::Deny(_)
        ));
    }

    /// 5b. ask 命中不被更早的 allow 掩盖，但真实命中已存在，故不是保守回退之外的新分支。
    #[test]
    fn ask_match_needs_approval() {
        let policy = policy_with(&[
            (&["git"], Decision::Allow),
            (&["git", "clean"], Decision::Ask),
        ]);
        assert!(matches!(
            evaluate(&policy, &argv(&["git", "clean", "-fd"])),
            Verdict::NeedsApproval(_)
        ));
    }

    /// 6. 解释器内联脚本一律 NeedsApproval，即使外层有 allow 规则。
    #[test]
    fn inline_script_needs_approval_even_with_outer_allow() {
        let policy = policy_with(&[
            (&["sh"], Decision::Allow),
            (&["bash"], Decision::Allow),
            (&["python3"], Decision::Allow),
        ]);
        for command in [
            argv(&["sh", "-c", "rm -rf /tmp/x"]),
            argv(&["bash", "-c", "rm -rf /tmp/x"]),
            argv(&["bash", "-lc", "rm -rf /tmp/x"]),
            argv(&["python3", "-c", "import os"]),
        ] {
            assert!(
                matches!(evaluate(&policy, &command), Verdict::NeedsApproval(_)),
                "{command:?} 不得因外层 allow 放行"
            );
        }
    }

    /// 7. 前缀边界：`git status` 命中，`git statusx` 不命中。
    #[test]
    fn prefix_boundary_distinguishes_status_from_statusx() {
        let policy = policy_with(&[(&["git", "status"], Decision::Allow)]);
        assert_eq!(evaluate(&policy, &argv(&["git", "status"])), Verdict::Allow);
        assert!(matches!(
            evaluate(&policy, &argv(&["git", "statusx"])),
            Verdict::NeedsApproval(_)
        ));
    }

    /// 8. 说明文本不回显被检查命令的任何参数值。
    #[test]
    fn reasons_do_not_echo_command_argv_values() {
        let secret = "/home/ubuntu/very-secret-path";
        let policy = policy_with(&[
            (&["rm", "-rf"], Decision::Deny),
            (&["sh", "-c"], Decision::Deny),
        ]);

        for command in [
            argv(&["rm", "-rf", secret]),
            argv(&["sh", "-c", secret]),
            argv(&["cargo", "publish", secret]),
        ] {
            let verdict = evaluate(&policy, &command);
            let Some(reason) = allow_reason(&verdict) else {
                panic!("{command:?} 不应被放行");
            };
            assert!(
                !reason.contains(secret),
                "说明文本回显了命令参数值: {reason}"
            );
            for token in &command {
                assert!(
                    !reason.contains(token.as_str()),
                    "说明文本回显了 token: {reason}"
                );
            }
        }
    }
}
