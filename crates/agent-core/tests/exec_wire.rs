//! `shell.exec` 接线验收：spec 清单、真实执行、危险命令强制 ask、沙箱链生效。
//!
//! 本测试**不经过注册表**（注册由汇总卡在 `tool.rs` 的 `default_registry()` 负责），
//! 直接调本卡产出的 `tool_specs()` 与适配函数。测试内按汇总卡的同一段循环把
//! spec 装进 `ToolRegistry`，用于验证「注册后权限门认可、且危险命令仍走 ask」。
//!
//! 源文件按 `#[path]` 引入而非 `use agent_core::…`：正式模块声明在共享文件
//! `lib.rs` 上（汇总卡负责），本卡不改共享文件，故测试自带一份等价副本运行。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::permission::{Decision, PermissionGate};
use agent_core::r#loop::ToolExecutor;
use agent_core::schema::{Mode, PermissionMode};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_execpolicy::Decision as PolicyDecision;
use agent_sandbox::{ExecBounds, SandboxPolicy, SandboxablePreference};
use agent_tools_exec::GuardedExec;
use serde_json::{json, Value};

#[path = "../src/exec_adapter.rs"]
mod exec_adapter;
#[path = "../src/tool_exec.rs"]
mod tool_exec;

// 源文件用 `crate::` 前缀引用本 crate 的类型；测试内用等价 shim 顶上。
pub mod executor {
    pub use agent_core::executor::{ExecToolExecutor, EXEC_TOOL_NAME};
}
pub mod tool {
    pub use agent_core::tool::ToolKind;
}
pub mod r#loop {
    pub use agent_core::r#loop::ToolExecutor;
}

use exec_adapter::{shell_exec_adapter, shell_exec_executor};
use tool_exec::{tool_specs, SHELL_EXEC_TOOL};

/// 按汇总卡的同一段循环把 spec 清单装进注册表。
fn registered() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for (name, description, kind, sensitive_arguments, timeout_ms) in tool_specs() {
        registry.register(ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            kind,
            sensitive_arguments,
            timeout_ms,
        });
    }
    registry
}

/// 只允许 `shell.exec` 的 Craft 门。
fn gate() -> PermissionGate {
    PermissionGate::new(registered())
        .with_whitelist(Mode::Craft, BTreeSet::from(["shell.exec".to_string()]))
}

fn check(gate: &PermissionGate, command: &str) -> Decision {
    gate.check(
        Mode::Craft,
        PermissionMode::Full,
        SHELL_EXEC_TOOL,
        &json!({ "command": command }),
    )
}

fn argv(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_string()).collect()
}

fn policy_with(rules: &[(&[&str], PolicyDecision)]) -> agent_execpolicy::argv::Policy {
    let mut policy = agent_execpolicy::argv::Policy::empty();
    for (prefix, decision) in rules {
        policy
            .add_prefix_rule(&argv(prefix), *decision)
            .expect("前缀规则构造必须成功");
    }
    policy
}

/// `Forbid` 偏好（不启用沙箱）+ 默认预算的判定 → 执行器。
fn guarded(policy: agent_execpolicy::argv::Policy) -> Arc<GuardedExec> {
    Arc::new(GuardedExec {
        policy,
        pref: SandboxablePreference::Forbid,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds::default(),
    })
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 1) spec 清单注册后含 `shell.exec`，类别/敏感标记/超时与冻结规格一致。
#[test]
fn specs_register_shell_exec_with_frozen_attributes() {
    let registry = registered();
    let spec = registry
        .get(SHELL_EXEC_TOOL)
        .expect("spec 清单应含 shell.exec");

    assert_eq!(spec.name, "shell.exec");
    assert_eq!(
        spec.kind,
        ToolKind::Exec,
        "执行命令的类别；权限门的危险命令层只扫 Exec 类工具"
    );
    assert!(
        spec.sensitive_arguments,
        "命令串可能含凭据，且需过敏感动作扫描"
    );
    assert_eq!(spec.timeout_ms, 30_000, "命令比文件读写慢");

    // 描述必须写清「做什么 + 失败怎么表现 + 能力边界」。
    for needle in ["执行", "沙箱", "确认", "失败", "退出码"] {
        assert!(
            spec.description.contains(needle),
            "描述缺「{needle}」：{}",
            spec.description
        );
    }

    // 名字与模式白名单、既有执行体逐字一致（漂移会被权限门拒掉）。
    assert!(
        agent_core::mode::WRITE_TOOLS.contains(&SHELL_EXEC_TOOL),
        "shell.exec 必须在写/执行白名单里"
    );
    assert_eq!(SHELL_EXEC_TOOL, agent_core::executor::EXEC_TOOL_NAME);
}

/// 2) 无害命令经适配器真实执行（`echo` 缺失时跳过）。
#[test]
fn harmless_command_executes_through_the_adapter() {
    if which("echo").is_none() {
        eprintln!("跳过：本机 PATH 中没有 echo");
        return;
    }

    let adapter = shell_exec_executor(guarded(policy_with(&[(&["echo"], PolicyDecision::Allow)])));
    let value = adapter
        .execute(SHELL_EXEC_TOOL, &json!({ "command": "echo exec-wire-ok" }))
        .expect("allow 规则下 echo 必须真实执行");

    let stdout = value["stdout"].as_str().expect("stdout 必须是字符串");
    assert!(
        stdout.contains("exec-wire-ok"),
        "stdout 应含标记: {value:?}"
    );
    assert_eq!(value["exit_code"].as_i64(), Some(0), "退出码应为 0");
    assert_eq!(value["timed_out"], json!(false));
    assert!(
        value["sandbox"].as_str().is_some(),
        "结果必须带沙箱标签，说明走的是 GuardedExec 链: {value:?}"
    );
}

/// 3) 危险命令 ⇒ 强制 ask（即使已显式 auto 放行、且沙箱可用），不是 allow。
///
/// 对照组：同一扇门下无害命令维持 auto allow；未注册时仍是 deny「未注册」。
#[test]
fn dangerous_commands_force_ask_even_when_auto_allowed() {
    let gate = gate()
        .with_auto_allowed(SHELL_EXEC_TOOL)
        .with_sandbox_probe(|| Some("seccomp".to_string()));

    // 只被危险命令层（全文子串表）命中的两条，必须给出该层原因。
    for (command, rule) in [
        ("git reset --hard HEAD~1", "git-reset-hard"),
        ("shutdown -h now", "power-off"),
    ] {
        let assessed = gate.check_assessed(
            Mode::Craft,
            PermissionMode::Full,
            SHELL_EXEC_TOOL,
            &json!({ "command": command }),
        );
        assert_eq!(
            assessed.decision,
            Decision::Ask,
            "危险命令必须 ask: {command}"
        );
        assert_eq!(
            assessed.reason,
            format!("dangerous-command: {rule}"),
            "应由危险命令层命中: {command}"
        );
    }

    // 敏感动作正则表同样强制 ask（两套规则表都不可被 auto 绕过）。
    for command in ["rm -rf /tmp/exec-wire", "chmod -R 777 /"] {
        assert_eq!(check(&gate, command), Decision::Ask, "{command} 必须 ask");
    }
    assert!(
        !matches!(check(&gate, "reboot"), Decision::Deny(_)),
        "注册后的危险命令不得落到「未注册」deny"
    );

    // 对照组：无害命令仍可被显式放行（证明不是「一律 ask」）。
    assert_eq!(check(&gate, "echo hi"), Decision::Allow);

    // 反过来：同一命令在空注册表里是 deny「未注册」——证明注册条目确实生效。
    let unregistered = PermissionGate::new(ToolRegistry::new())
        .with_whitelist(Mode::Craft, BTreeSet::from([SHELL_EXEC_TOOL.to_string()]));
    let decision = check(&unregistered, "echo hi");
    assert!(
        matches!(&decision, Decision::Deny(reason) if reason.contains("未注册")),
        "空注册表必须 deny 未注册：{decision:?}"
    );
}

/// 4) 沙箱包装真的在管执行：Deny 不产生副作用，Allow 才落盘且结果带沙箱标签。
///
/// 标记文件是可观测副作用：若适配器绕过 `GuardedExec` 直跑，deny 用例会留下文件，
/// 且结果里不会有 `sandbox` 字段。
#[test]
fn guarded_exec_governs_the_execution_path() {
    if which("touch").is_none() {
        eprintln!("跳过：本机 PATH 中没有 touch");
        return;
    }

    let dir = tempfile::tempdir().expect("临时目录");
    let marker = dir.path().join("exec-wire-marker");
    let command = format!("touch {}", marker.display());

    // Deny：判定层先拒，不 spawn。
    let denied = shell_exec_adapter(guarded(policy_with(&[(&["touch"], PolicyDecision::Deny)])));
    let result = denied.execute(SHELL_EXEC_TOOL, &json!({ "command": command }));
    assert!(result.is_err(), "deny 必须返回错误: {result:?}");
    assert!(!marker.exists(), "被拒绝的命令不得产生副作用");

    // Allow：真实执行，结果带 `sandbox` 标签（`Forbid` 下为 "none"）。
    let allowed = shell_exec_adapter(guarded(policy_with(&[(&["touch"], PolicyDecision::Allow)])));
    let value: Value = allowed
        .execute(SHELL_EXEC_TOOL, &json!({ "command": command }))
        .expect("allow 必须真实执行");
    assert!(marker.exists(), "allow 应产生副作用: {value:?}");
    assert_eq!(value["sandbox"], json!("none"), "Forbid 偏好的沙箱标签");
}
