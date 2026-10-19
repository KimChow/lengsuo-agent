//! 判定 → 执行的链式编排：`guard::evaluate` 的结果决定是否进入沙箱执行。
//!
//! 分层与调用顺序（严格对应参考实现 `UnifiedExecRuntime` 的
//! `run` 入口 + `build_unified_exec_sandbox_command` 的命令构造）：
//! 1. 策略判定 [`evaluate`]（等价 `ExecApprovalRequirement` 决定 first attempt）；
//! 2. `Deny` → 直接拒绝，**不 spawn**（等价 `Forbidden`）；
//! 3. `NeedsApproval` → 直接转人工审批，**不 spawn**（等价 `NeedsApproval`）；
//! 4. `Allow` → 拆 argv 为 `program` / `args`（等价 `split_first` 那一处），
//!    环境经 [`crate::exec::env`] 清洗后交给沙箱；
//! 5. 交给 `run_sandboxed_bounded`，由 `pref` + `policy` 决定降级链
//!    —— `Require` 且沙箱不可用时由 sandbox 层 fail-closed 报错，本层不重复判定。
//!
//! 网络一律 [`NetworkMode::Blocked`]：本层没有网络豁免入口，fail-closed。

use std::path::Path;

use agent_sandbox::{
    run_sandboxed_bounded, CommandSpec, ExecBounds, ExecOutcome, NetworkMode, SandboxError,
    SandboxPolicy, SandboxablePreference,
};

use super::env;
use super::guard::{evaluate, Verdict};
use crate::error::ExecError;

/// 判定 → 执行器：把策略、沙箱偏好、沙箱策略与执行预算绑在一起。
#[derive(Debug, Clone)]
pub struct GuardedExec {
    /// argv 策略；判定阶段只读。
    pub policy: agent_execpolicy::argv::Policy,
    /// 沙箱偏好；`Require` 时沙箱不可用必须报错（由 agent-sandbox 保证）。
    pub pref: SandboxablePreference,
    /// 沙箱策略；无沙箱时 `Auto` 是否转人工审批。
    pub sandbox_policy: SandboxPolicy,
    /// 超时与输出上限。
    pub bounds: ExecBounds,
}

impl GuardedExec {
    /// 判定 → 执行。`Deny`/`NeedsApproval` 直接返回，**不 spawn 任何进程**。
    pub fn run(&self, argv: &[String], cwd: Option<&Path>) -> Result<ExecOutcome, ExecError> {
        match evaluate(&self.policy, argv) {
            Verdict::Deny(reason) => Err(ExecError::Denied(reason)),
            Verdict::NeedsApproval(reason) => Err(ExecError::NeedsApproval(reason)),
            Verdict::Allow => {
                let spec = self.build_command_spec(argv, cwd)?;
                run_sandboxed_bounded(spec, self.pref, self.sandbox_policy, self.bounds)
                    .map_err(map_sandbox_error)
            }
        }
    }

    /// 把已放行的 argv 组装为沙箱命令：首 token 是 program，其余是参数。
    ///
    /// 空 argv 不可能走到这里（[`evaluate`] 已判 `NeedsApproval`），仍保留显式错误，
    /// 避免 `argv[0]` 越界——与参考实现里 `split_first` 的空命令拒绝同义。
    fn build_command_spec(
        &self,
        argv: &[String],
        cwd: Option<&Path>,
    ) -> Result<CommandSpec, ExecError> {
        let (program, args) = argv
            .split_first()
            .ok_or(ExecError::Denied("命令行为空".to_string()))?;
        Ok(CommandSpec {
            program: program.into(),
            args: args.to_vec(),
            env: clean_env(),
            cwd: cwd.map(Path::to_path_buf),
            // 网络 fail-closed：本层不放行任何网络访问。
            network: NetworkMode::Blocked,
            ..CommandSpec::default()
        })
    }
}

/// 派生环境：与 [`crate::exec::ExecRunner`] 走同一条清洗路径（剔除不可继承键）。
fn clean_env() -> Vec<(String, String)> {
    let mut env_vars: Vec<(String, String)> = env::create_child_env(std::env::vars_os())
        .into_iter()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    // HashMap 迭代顺序不稳定，排序让同一次运行结果可复现。
    env_vars.sort();
    env_vars
}

/// 沙箱错误 → 工具错误：保留「必须存在沙箱但不可用」与「需人工审批」两类语义。
///
/// `NeedsApproval` 单独映射，使 `Auto` 偏好下的末端降级也能被上层识别为审批请求，
/// 而不是被笼统当成 IO 失败。
fn map_sandbox_error(error: SandboxError) -> ExecError {
    match error {
        SandboxError::NeedsApproval(reason) => ExecError::NeedsApproval(reason),
        SandboxError::Unavailable(reason) => ExecError::Denied(reason),
        SandboxError::Denied(reason) => ExecError::Denied(reason),
        SandboxError::Spawn(reason) => ExecError::Io(reason),
        SandboxError::Io(error) => ExecError::Io(error.to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use agent_execpolicy::argv::Policy;
    use agent_execpolicy::Decision;
    use std::path::PathBuf;

    /// 测试专用 argv 构造。
    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_string()).collect()
    }

    /// 测试专用策略构造。
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

    fn guarded(policy: Policy, pref: SandboxablePreference) -> GuardedExec {
        GuardedExec {
            policy,
            pref,
            sandbox_policy: SandboxPolicy::default(),
            bounds: ExecBounds::default(),
        }
    }

    /// 1. deny 命中 ⇒ `Denied`，且命令确实没被执行（标记文件未产生）。
    #[test]
    fn deny_returns_denied_without_spawning() {
        let dir = tempfile::tempdir().expect("临时目录");
        let marker = dir.path().join("denied-marker");

        let policy = policy_with(&[(&["touch"], Decision::Deny)]);
        let exec = guarded(policy, SandboxablePreference::Forbid);
        let argv = argv(&["touch", marker.to_str().expect("标记路径")]);

        let result = exec.run(&argv, None);

        assert!(
            matches!(result, Err(ExecError::Denied(_))),
            "deny 必须返回 Denied: {result:?}"
        );
        assert!(!marker.exists(), "被拒绝的命令不得产生任何副作用");
    }

    /// 2. 无规则命中 ⇒ `NeedsApproval`，同样不 spawn。
    #[test]
    fn unmatched_returns_needs_approval_without_spawning() {
        let dir = tempfile::tempdir().expect("临时目录");
        let marker = dir.path().join("unmatched-marker");

        let policy = policy_with(&[(&["git", "status"], Decision::Allow)]);
        let exec = guarded(policy, SandboxablePreference::Forbid);
        let argv = argv(&["touch", marker.to_str().expect("标记路径")]);

        let result = exec.run(&argv, None);

        assert!(
            matches!(result, Err(ExecError::NeedsApproval(_))),
            "未命中规则必须转审批: {result:?}"
        );
        assert!(!marker.exists(), "待审批的命令不得执行");
    }

    /// 2b. ask 规则命中 ⇒ `NeedsApproval`。
    #[test]
    fn ask_rule_returns_needs_approval_without_spawning() {
        let dir = tempfile::tempdir().expect("临时目录");
        let marker = dir.path().join("ask-marker");

        let policy = policy_with(&[(&["touch"], Decision::Ask)]);
        let exec = guarded(policy, SandboxablePreference::Forbid);
        let argv = argv(&["touch", marker.to_str().expect("标记路径")]);

        let result = exec.run(&argv, None);

        assert!(
            matches!(result, Err(ExecError::NeedsApproval(_))),
            "ask 必须转审批: {result:?}"
        );
        assert!(!marker.exists(), "待审批的命令不得执行");
    }

    /// 3. `Forbid` + allow 规则 ⇒ 真实执行并拿到 stdout。
    ///
    /// 本地 `echo` 从 PATH 解析，故用 `sh` 之外的真实程序；系统无 `echo` 时跳过。
    #[test]
    fn forbid_with_allow_rule_runs_and_captures_stdout() {
        if which("echo").is_none() {
            eprintln!("跳过：本机 PATH 中没有 echo");
            return;
        }

        let policy = policy_with(&[(&["echo", "hi"], Decision::Allow)]);
        let exec = guarded(policy, SandboxablePreference::Forbid);

        let outcome = exec
            .run(&argv(&["echo", "hi"]), None)
            .expect("Forbid + allow 必须真实执行");

        assert!(outcome.status.success, "echo 应正常退出: {outcome:?}");
        assert_eq!(String::from_utf8_lossy(&outcome.stdout).trim(), "hi");
        assert!(!outcome.timed_out);
    }

    /// 5. 短超时 + `sleep 5` ⇒ `timed_out`，且确实在预算内返回。
    #[test]
    fn short_bounds_timeout_marks_timed_out() {
        if which("sleep").is_none() {
            eprintln!("跳过：本机 PATH 中没有 sleep");
            return;
        }

        let policy = policy_with(&[(&["sleep"], Decision::Allow)]);
        let mut exec = guarded(policy, SandboxablePreference::Forbid);
        exec.bounds = ExecBounds {
            timeout: std::time::Duration::from_millis(200),
            ..ExecBounds::default()
        };

        let started = std::time::Instant::now();
        let outcome = exec
            .run(&argv(&["sleep", "5"]), None)
            .expect("应返回而非挂死");
        let elapsed = started.elapsed();

        assert!(outcome.timed_out, "超时必须置 timed_out: {outcome:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(4),
            "超时必须受 bounds 约束，实际耗时 {elapsed:?}"
        );
    }

    /// 4. `Require` 且沙箱不可用 ⇒ 报错而非静默直跑。
    ///
    /// 沙箱可用时不构成本用例的前提（此时命令本就该被沙箱执行），故跳过。
    #[test]
    fn require_without_available_sandbox_reports_error() {
        let dir = tempfile::tempdir().expect("临时目录");
        let marker = dir.path().join("require-marker");

        let policy = policy_with(&[(&["touch"], Decision::Allow)]);
        let exec = guarded(policy, SandboxablePreference::Require);
        let command = argv(&["touch", marker.to_str().expect("标记路径")]);

        let sandbox_available = exec.run(&argv(&["echo", "probe"]), None).is_ok();
        if sandbox_available {
            eprintln!("跳过：本机沙箱可用，不构成 Require 降级场景");
            return;
        }

        let result = exec.run(&command, None);

        assert!(
            result.is_err(),
            "Require 且沙箱不可用必须报错，不得直跑: {result:?}"
        );
        assert!(!marker.exists(), "沙箱不可用时不得执行命令");
    }

    /// 未命中规则时也要区分：内联脚本一律审批（沿用 guard 语义，本层不放松）。
    #[test]
    fn inline_script_is_never_allowed_by_outer_rule() {
        let policy = policy_with(&[(&["sh"], Decision::Allow)]);
        let exec = guarded(policy, SandboxablePreference::Forbid);
        let result = exec.run(&argv(&["sh", "-c", "echo hi"]), None);
        assert!(matches!(result, Err(ExecError::NeedsApproval(_))));
    }

    /// `PATH` 中是否存在该程序（用于跳过依赖系统程序的用例）。
    fn which(program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }
}
