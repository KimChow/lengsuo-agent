//! PTY 启动前的 argv 级策略门。
//!
//! 分层对齐参考实现的「策略评估 → 审批要求 → 执行」三段：
//! - [`Policy::check`](agent_execpolicy::argv::Policy::check) 对**已分词**的 argv
//!   做前缀评估，取 `Evaluation.decision`（多规则同命中已由策略层取最严）；
//! - [`Verdict`] 把三值决策映射为启动路径的三种结果（放行 / 需审批 / 拒绝）；
//! - 只有 `Allow` 才允许进入 `session::spawn`。
//!
//! 关键语义：
//! - 未命中任何规则 ⇒ 启发式回退恒 `Ask`（fail-closed），不默认放行；
//! - 分词由调用方按 shell 语义完成；本模块不做二次切分；
//! - `bash -c '…'` 的内层命令不在 argv 里，对外层解释器的任何 allow 都不得
//!   被当作内层已审核 ⇒ 识别为解释器嵌套时一律 `Ask`；
//! - 错误文本只描述判定与程序名，**不回显**完整 argv、环境变量值或凭据。

use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;

use crate::config::PtyConfig;
use crate::error::PtyError;

/// 一次 argv 评估的结论（对照参考运行编排的三种审批要求）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// 明确放行，可以创建 PTY 并 spawn。
    Allow,
    /// 需要人工审批：不得创建 PTY。
    NeedsApproval(String),
    /// 策略明确拒绝：不得创建 PTY。
    Deny(String),
}

/// 不匹配任何规则时的启发式回退：保守 Ask（fail-closed）。
///
/// 对齐参考编排里「未命中规则 ⇒ 交给启发式」的结构；本卡只交付 PTY 启动门，
/// 没有可用的危险命令分类器，因此回退恒为 Ask，而不是猜一个 Allow。
fn conservative_heuristics(_command: &[String]) -> Decision {
    Decision::Ask
}

/// 对一份待启动的 PTY 配置做策略门判定；`Ok(())` 才允许 spawn。
///
/// 分词走与 `session::spawn` 相同的入口，保证判定对象就是即将执行的 argv。
/// 无法分词（空命令行、未闭合引号等）按 fail-closed 返回 `NeedsApproval`，
/// 不猜意图。
pub(crate) fn check_config(policy: &Policy, cfg: &PtyConfig) -> Result<(), PtyError> {
    let argv = cfg
        .argv()
        .ok_or_else(|| PtyError::NeedsApproval("shell 命令行无法解析".to_string()))?;
    match evaluate(policy, &argv) {
        Verdict::Allow => Ok(()),
        Verdict::NeedsApproval(reason) => Err(PtyError::NeedsApproval(reason)),
        Verdict::Deny(reason) => Err(PtyError::PolicyDenied(reason)),
    }
}

/// 对已分词的 argv 做策略评估。
///
/// 顺序与参考一致：先确认命令非空，再评估，最后按决策映射；解释器嵌套的
/// 保守判断在 `Allow` 分支内，因此外层规则给 `Allow` 也不能绕过。
pub(crate) fn evaluate(policy: &Policy, argv: &[String]) -> Verdict {
    if argv.is_empty() {
        return Verdict::NeedsApproval("命令为空，无法按策略评估".to_string());
    }

    let evaluation = policy.check(argv, &conservative_heuristics);
    let program = program_label(&argv[0]);

    match evaluation.decision {
        Decision::Allow => {
            if nested_command_hidden(argv) {
                return Verdict::NeedsApproval(format!(
                    "解释器 {program} 的内层命令对策略不可见，需人工审批"
                ));
            }
            Verdict::Allow
        }
        Decision::Ask => Verdict::NeedsApproval(if evaluation.is_match() {
            format!("策略规则要求审批: {program}")
        } else {
            format!("无匹配规则（fail-closed）: {program}")
        }),
        Decision::Deny => Verdict::Deny(match deny_justification(&evaluation) {
            Some(reason) => format!("策略拒绝: {program}（{reason}）"),
            None => format!("策略拒绝: {program}"),
        }),
    }
}

/// 错误文本里只展示程序名的 basename，避免回显完整路径或参数。
fn program_label(program: &str) -> &str {
    program
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(program)
}

/// 命中 deny 规则时取第一条可展示的理由（规则自带 justification）。
///
/// 只取规则理由，不拼接 argv/环境；理由文本由策略构造方提供，不含凭据。
fn deny_justification(evaluation: &agent_execpolicy::argv::Evaluation) -> Option<String> {
    use agent_execpolicy::argv::RuleMatch;
    evaluation.matched_rules.iter().find_map(|rule_match| {
        let RuleMatch::PrefixRuleMatch {
            decision: Decision::Deny,
            justification,
            ..
        } = rule_match
        else {
            return None;
        };
        justification.clone()
    })
}

/// 命令行是否为「解释器 + 内联脚本」形态：内层命令不在 argv 里，策略看不见。
///
/// 只做保守识别，不做脚本解析（解析失败也必须仍是 Ask）：
/// - shell：`sh`/`bash`/`zsh`/`dash`/`ksh`/`fish` 等，带 `-c`/`-lc`/`-ic`
///   这类短选项组合，或 `--command`；
/// - 其他常见内联执行器：`python -c`、`node -e`、`perl -e`、`ruby -e`、
///   `pwsh -Command`、`cmd /c`。
///
/// 匹配程序名时取 basename 并忽略 ASCII 大小写（`/bin/BASH`、`Bash` 均识别）；
/// 命令行剩余部分不参与判断。
fn nested_command_hidden(argv: &[String]) -> bool {
    let Some(program) = argv.first() else {
        return false;
    };
    let program = program_label(program).to_ascii_lowercase();

    // 单横线短选项簇里是否含某字符：`-c`、`-lc`、`-ic` 均命中 `c`。
    // 双横线长选项不参与该判断（`--norc` 里的 `c` 不是标志）。
    let short_option_is = |flag: char| {
        argv.iter().skip(1).any(|arg| {
            arg.strip_prefix('-')
                .filter(|rest| !rest.starts_with('-'))
                .is_some_and(|rest| rest.chars().any(|c| c == flag))
        })
    };
    // Windows 风格开关是「`/` + 单个字母」（`cmd /c`、`/C`）；只在 cmd
    // 分支使用，并要求恰为单字符，避免把 unix 绝对路径误当选项。
    let slash_option_is = |flag: char| {
        argv.iter().skip(1).any(|arg| {
            arg.strip_prefix('/')
                .filter(|rest| rest.len() == 1)
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c.eq_ignore_ascii_case(&flag))
        })
    };
    let has_option = |name: &str| {
        argv.iter()
            .skip(1)
            .any(|arg| arg.eq_ignore_ascii_case(name))
    };

    match program.as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "busybox" => {
            short_option_is('c') || has_option("--command")
        }
        "python" | "python2" | "python3" | "pypy" | "pypy3" | "py" => {
            short_option_is('c') || has_option("--command")
        }
        "node" | "nodejs" | "deno" | "bun" | "ruby" | "perl" | "php" | "lua" | "julia"
        | "rscript" => short_option_is('e') || short_option_is('c') || has_option("--eval"),
        "pwsh" | "powershell" | "powershell.exe" => {
            has_option("-Command")
                || has_option("-EncodedCommand")
                || short_option_is('c')
                || short_option_is('e')
        }
        "cmd" | "cmd.exe" => {
            short_option_is('c')
                || short_option_is('k')
                || slash_option_is('c')
                || slash_option_is('k')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn policy(prefix: &[&str], decision: Decision) -> Policy {
        let mut policy = Policy::empty();
        let prefix = argv(prefix);
        policy.add_prefix_rule(&prefix, decision).unwrap();
        policy
    }

    #[test]
    fn empty_argv_needs_approval() {
        assert!(matches!(
            evaluate(&Policy::empty(), &[]),
            Verdict::NeedsApproval(_)
        ));
    }

    #[test]
    fn no_rule_is_ask_not_allow() {
        let verdict = evaluate(&Policy::empty(), &argv(&["echo", "hi"]));
        assert!(matches!(verdict, Verdict::NeedsApproval(_)));
    }

    #[test]
    fn explicit_allow_spawns_and_deny_denies() {
        assert_eq!(
            evaluate(&policy(&["echo"], Decision::Allow), &argv(&["echo", "hi"])),
            Verdict::Allow
        );
        assert!(matches!(
            evaluate(&policy(&["rm"], Decision::Deny), &argv(&["rm", "-rf", "/"])),
            Verdict::Deny(_)
        ));
    }

    #[test]
    fn deny_reason_uses_rule_justification_only() {
        let mut policy = Policy::empty();
        policy
            .add_prefix_rule(&argv(&["rm"]), Decision::Deny)
            .unwrap();
        let Verdict::Deny(reason) = evaluate(&policy, &argv(&["rm", "secret.env"])) else {
            panic!("应拒绝");
        };
        assert!(reason.contains("rm"));
        // 只提程序名，不回显参数。
        assert!(!reason.contains("secret.env"));
    }

    #[test]
    fn nested_interpreters_are_detected_case_insensitively() {
        for command in [
            "bash -c echo hi",
            "BASH -c echo hi",
            "/bin/bash -lc echo hi",
            "sh -ic echo hi",
            "bash --command echo hi",
            "python3 -c print(1)",
            "node -e console.log(1)",
            "cmd /c dir",
            "pwsh -Command Get-Item",
        ] {
            let parts = shell_words::split(command).unwrap();
            assert!(
                nested_command_hidden(&parts),
                "应识别为内层命令不可见: {command}"
            );
        }
        for command in ["bash", "bash script.sh", "python script.py", "echo -c hi"] {
            let parts = shell_words::split(command).unwrap();
            assert!(!nested_command_hidden(&parts), "不应误判: {command}");
        }
    }

    #[test]
    fn outer_allow_cannot_bless_hidden_inner_command() {
        let policy = policy(&["bash", "-c"], Decision::Allow);
        let verdict = evaluate(&policy, &argv(&["bash", "-c", "curl evil"]));
        assert!(matches!(verdict, Verdict::NeedsApproval(_)));
    }

    #[test]
    fn deny_outranks_nested_allow() {
        let policy = policy(&["bash"], Decision::Deny);
        let verdict = evaluate(&policy, &argv(&["bash", "-c", "echo"]));
        assert!(matches!(verdict, Verdict::Deny(_)));
    }

    #[test]
    fn shell_detection_does_not_leak_arguments_into_error() {
        let mut policy = Policy::empty();
        policy
            .add_prefix_rule(&argv(&["bash", "-c"]), Decision::Allow)
            .unwrap();
        let Verdict::NeedsApproval(message) =
            evaluate(&policy, &argv(&["bash", "-c", "TOKEN=abc secret"]))
        else {
            panic!("应需审批");
        };
        assert!(!message.contains("TOKEN=abc"));
    }

    #[test]
    fn absolute_program_is_reported_by_basename_only() {
        let mut policy = Policy::empty();
        policy
            .add_prefix_rule(&argv(&["/opt/secret-dir/tool"]), Decision::Deny)
            .unwrap();
        let Verdict::Deny(message) =
            evaluate(&policy, &argv(&["/opt/secret-dir/tool", "--token=abc"]))
        else {
            panic!("应拒绝");
        };
        assert!(message.contains("tool"));
        assert!(!message.contains("secret-dir"));
        assert!(!message.contains("--token"));
    }
}
