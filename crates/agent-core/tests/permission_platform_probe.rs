//! 沙箱优先门的**真实平台探测**接线（A22-1 第二步）：`with_platform_sandbox_probe`。
//!
//! 与 `permission_execpolicy.rs` 的注入式门（`with_sandbox_probe`）分开：
//! 这里断言的是「宿主探测 → 后端标签 → 门」这条真实链路，
//! 判定顺序（工具级 → 资源级 → 沙箱门）与注入式门共用一个实现。

use std::collections::BTreeSet;

use agent_core::permission::{Decision, PermissionGate};
use agent_core::schema::{Mode, PermissionMode};
use agent_core::tool::{ToolRegistry, ToolSpec};
use agent_execpolicy::{AccessKind, Decision as PolicyDecision, Matcher, Policy, Rule};
use serde_json::json;

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    for (name, kind, sensitive) in [
        ("fs.read", agent_core::tool::ToolKind::Read, false),
        ("shell.exec", agent_core::tool::ToolKind::Exec, true),
    ] {
        r.register(ToolSpec {
            name: name.into(),
            description: format!("{name} 测试用"),
            kind,
            sensitive_arguments: sensitive,
            timeout_ms: 5_000,
        });
    }
    r
}

fn base_gate() -> PermissionGate {
    PermissionGate::new(registry()).with_whitelist(
        Mode::Craft,
        BTreeSet::from(["fs.read".to_string(), "shell.exec".to_string()]),
    )
}

fn rule(pattern: &str, decision: PolicyDecision) -> Rule {
    Rule {
        pattern: pattern.into(),
        matcher: Matcher::Glob,
        decision,
        reason: None,
    }
}

/// 策略层装进单段规则的辅助。
fn policy_segment(kind: AccessKind, rules: Vec<Rule>) -> Policy {
    let mut p = Policy::empty();
    match kind {
        AccessKind::File => p.file = rules,
        AccessKind::Process => p.process = rules,
        AccessKind::Network => p.network = rules,
    }
    p
}

// ---------- 真实平台探测接线（with_platform_sandbox_probe） ----------

#[test]
fn platform_probe_keeps_empty_gate_default() {
    // PermissionGate::new() 的既有默认不得被本次接线改动：不探测即沙箱门不生效。
    let g = base_gate();
    let d = g.check(
        Mode::Craft,
        PermissionMode::Full,
        "shell.exec",
        &json!({"command": "ls -la"}),
    );
    assert_eq!(d, Decision::Ask, "默认构造不得启用沙箱优先门");
}

#[test]
fn platform_probe_maps_backend_to_metric_tag_and_gates_exec() {
    // 真实平台探测：宿主有后端 → 命令类 ask 升 allow；无后端 → 维持 ask。
    let g = base_gate().with_platform_sandbox_probe();
    let detected = agent_sandbox::get_platform_sandbox()
        .map(agent_sandbox::SandboxType::as_metric_tag)
        .filter(|tag| !tag.is_empty());
    let d = g.check(
        Mode::Craft,
        PermissionMode::Full,
        "shell.exec",
        &json!({"command": "ls -la"}),
    );
    match detected {
        Some(tag) => assert_eq!(d, Decision::Allow, "探测到后端 {tag} → 沙箱优先门生效"),
        None => assert_eq!(d, Decision::Ask, "本机无沙箱后端 → 维持 ask"),
    }
}

#[test]
fn platform_probe_leaves_deny_and_non_exec_untouched() {
    // 沙箱门只放宽「ask 的 Exec 类」：deny 与非执行类别一律不受影响。
    let project = policy_segment(
        AccessKind::Process,
        vec![rule("shutdown**", PolicyDecision::Deny)],
    );
    let g = base_gate()
        .with_policy(project)
        .with_platform_sandbox_probe();
    let denied = g.check(
        Mode::Craft,
        PermissionMode::Full,
        "shell.exec",
        &json!({"command": "shutdown now"}),
    );
    assert!(
        matches!(denied, Decision::Deny(_)),
        "deny 不被沙箱门放宽：{denied:?}"
    );
    let non_exec = g.check(
        Mode::Craft,
        PermissionMode::Full,
        "fs.read",
        &json!({"path": "docs/readme.md"}),
    );
    assert_eq!(non_exec, Decision::Ask, "非 Exec 类别维持原结论");
}
