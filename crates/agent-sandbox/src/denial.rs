//! 沙箱拒绝识别（分层对照 参考实现 `sandboxing/src/denial.rs`）。
//!
//! 纯判定、零副作用：只回答「这次失败像不像被沙箱拦下的」，不落审计、不改执行契约。
//! 判定刻意保守——只有命中已知退出码/信号/关键字才算，避免把普通失败误判成沙箱拒绝。

use crate::types::{ExecOutcome, SandboxType};

/// 快速拒绝退出码：shell/exec 层的「命令没跑起来」，不算沙箱拒绝（参考实现 `QUICK_REJECT_EXIT_CODES`）。
const QUICK_REJECT_EXIT_CODES: [i32; 3] = [2, 126, 127];
/// 信号退出码基址（参考实现 `EXIT_CODE_SIGNAL_BASE`）。
const EXIT_CODE_SIGNAL_BASE: i32 = 128;
/// SIGSYS：seccomp 杀进程时内核使用的信号号（参考实现取自 `libc::SIGSYS`）。
const SIGSYS_CODE: i32 = 31;
/// 输出里的沙箱拒绝关键字（参考实现 `SANDBOX_DENIED_KEYWORDS`）。
const SANDBOX_DENIED_KEYWORDS: [&str; 7] = [
    "operation not permitted",
    "permission denied",
    "read-only file system",
    "seccomp",
    "sandbox",
    "landlock",
    "failed to write file",
];

/// 是否像「被沙箱拒绝」（参考实现 `is_likely_sandbox_denied`）。
///
/// 顺序与参考实现一致：未沙箱/成功退出 → 否；执行器托管沙箱关键字命中 → 是；
/// 快速拒绝退出码 → 否；LinuxSeccomp 下的 SIGSYS → 是；其余 → 否。
pub fn is_likely_sandbox_denied(outcome: &ExecOutcome) -> bool {
    if outcome.sandbox == SandboxType::None
        || outcome.status.code == Some(0)
        || outcome.status.success
    {
        return false;
    }

    if is_likely_executor_managed_sandbox_denied(outcome) {
        return true;
    }

    if let Some(code) = outcome.status.code {
        if QUICK_REJECT_EXIT_CODES.contains(&code) {
            return false;
        }
        if outcome.sandbox == SandboxType::LinuxSeccomp
            && code == EXIT_CODE_SIGNAL_BASE + SIGSYS_CODE
        {
            return true;
        }
    }

    // ExecStatus 直接带信号号：不必再借 128+SIGSYS 的编码绕一圈。
    outcome.sandbox == SandboxType::LinuxSeccomp && outcome.status.signal == Some(SIGSYS_CODE)
}

/// 后端未知时的执行器托管沙箱拒绝识别（参考实现 `is_likely_executor_managed_sandbox_denied`）。
pub fn is_likely_executor_managed_sandbox_denied(outcome: &ExecOutcome) -> bool {
    if outcome.status.code == Some(0) || outcome.status.success {
        return false;
    }

    [&outcome.stderr, &outcome.stdout]
        .into_iter()
        .any(|section| {
            let lower = String::from_utf8_lossy(section).to_lowercase();
            SANDBOX_DENIED_KEYWORDS
                .iter()
                .any(|needle| lower.contains(needle))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExecStatus;

    fn outcome(
        sandbox: SandboxType,
        code: Option<i32>,
        signal: Option<i32>,
        stderr: &str,
    ) -> ExecOutcome {
        ExecOutcome {
            status: ExecStatus {
                code,
                signal,
                success: code == Some(0),
            },
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
            sandbox,
            timed_out: false,
            truncated: false,
        }
    }

    #[test]
    fn unsandboxed_and_successful_runs_are_never_denials() {
        assert!(!is_likely_sandbox_denied(&outcome(
            SandboxType::None,
            Some(1),
            None,
            "permission denied"
        )));
        assert!(!is_likely_sandbox_denied(&outcome(
            SandboxType::LinuxSeccomp,
            Some(0),
            None,
            ""
        )));
    }

    #[test]
    fn keyword_hits_are_denials() {
        for needle in SANDBOX_DENIED_KEYWORDS {
            let o = outcome(
                SandboxType::LinuxSeccomp,
                Some(1),
                None,
                &format!("error: {needle}"),
            );
            assert!(is_likely_sandbox_denied(&o), "{needle}");
        }
        // 关键字判定大小写不敏感，且同样看 stdout。
        let mut o = outcome(SandboxType::LinuxSeccomp, Some(1), None, "");
        o.stdout = b"READ-ONLY FILE SYSTEM".to_vec();
        assert!(is_likely_sandbox_denied(&o));
    }

    #[test]
    fn quick_reject_codes_win_over_keyword_scan() {
        for code in QUICK_REJECT_EXIT_CODES {
            let o = outcome(SandboxType::LinuxSeccomp, Some(code), None, "");
            assert!(!is_likely_sandbox_denied(&o), "exit code {code}");
        }
    }

    #[test]
    fn sigsys_is_a_seccomp_denial() {
        // 128 + SIGSYS 的编码形态。
        let o = outcome(SandboxType::LinuxSeccomp, Some(159), None, "");
        assert!(is_likely_sandbox_denied(&o));
        // 直接带信号号的形态。
        let o = outcome(SandboxType::LinuxSeccomp, None, Some(SIGSYS_CODE), "");
        assert!(is_likely_sandbox_denied(&o));
        // 非 LinuxSeccomp 后端不套用该规则。
        let o = outcome(SandboxType::MacosSeatbelt, None, Some(SIGSYS_CODE), "");
        assert!(!is_likely_sandbox_denied(&o));
    }

    #[test]
    fn unrelated_failure_is_not_a_denial() {
        let o = outcome(SandboxType::LinuxSeccomp, Some(3), None, "assertion failed");
        assert!(!is_likely_sandbox_denied(&o));
    }
}
