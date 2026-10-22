//! 工具注册总装（x-tool-registry）：12 个域工具进默认注册表，且**不放宽**权限门。
//!
//! 四条不变量：
//! 1. `default_registry()` 含 12 个新工具名（fs.* / shell.exec / task.* / web.fetch）；
//! 2. 既有 7 个记忆工具仍在（回归保护）；
//! 3. 每个新工具的 kind / sensitive_arguments / timeout_ms 与卡面 D2 表逐字一致；
//! 4. 未注册的工具名即使进了白名单并 auto_allowed，权限门仍 `Deny("工具未注册")`。

use std::collections::BTreeSet;

use agent_core::mode::mode_tools;
use agent_core::permission::{Decision, PermissionGate};
use agent_core::schema::{Mode, PermissionMode};
use agent_core::tool::{default_registry, ToolKind};
use serde_json::json;

/// 卡面 D2 表的 12 个新工具：(名字, kind, sensitive_arguments, timeout_ms)。
const DOMAIN_TOOLS: [(&str, ToolKind, bool, u64); 12] = [
    ("fs.read", ToolKind::Read, false, 5_000),
    ("fs.list", ToolKind::Read, false, 5_000),
    ("fs.glob", ToolKind::Read, false, 5_000),
    ("fs.search", ToolKind::Read, false, 5_000),
    ("fs.write", ToolKind::Write, true, 10_000),
    ("fs.delete", ToolKind::Write, true, 10_000),
    ("shell.exec", ToolKind::Write, true, 30_000),
    ("task.list", ToolKind::Read, false, 5_000),
    ("task.get", ToolKind::Read, false, 5_000),
    ("task.create", ToolKind::Write, false, 5_000),
    ("task.update", ToolKind::Write, false, 5_000),
    ("web.fetch", ToolKind::Read, true, 30_000),
];

/// 既有记忆工具（4 写 + 3 检索），本卡不得删改。
const MEMORY_WRITE_TOOLS: [&str; 4] = [
    "memory.set_user",
    "memory.set_project",
    "memory.append_daily",
    "identity.set_soul",
];
const MEMORY_READONLY_TOOLS: [&str; 3] = ["memory.list", "memory.search", "memory.read"];

#[test]
fn default_registry_contains_twelve_domain_tools() {
    let registry = default_registry();
    let names: BTreeSet<&str> = registry
        .list()
        .iter()
        .map(|spec| spec.name.as_str())
        .collect();
    for (name, _, _, _) in DOMAIN_TOOLS {
        assert!(names.contains(name), "{name} 必须注册进默认注册表");
    }
    // 7（记忆）+ 12（域工具）＝ 19：注册总装的完整口径。
    assert_eq!(registry.len(), 19, "默认注册表应恰好 19 个工具");
}

#[test]
fn default_registry_keeps_memory_tools() {
    let registry = default_registry();
    for name in MEMORY_WRITE_TOOLS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 不得从默认注册表消失"));
        assert_eq!(spec.kind, ToolKind::Write, "{name} 应为写类");
        assert!(spec.sensitive_arguments, "{name} 必须过敏感动作扫描");
        assert_eq!(spec.timeout_ms, 5_000, "{name} 超时不得漂移");
    }
    for name in MEMORY_READONLY_TOOLS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 不得从默认注册表消失"));
        assert_eq!(spec.kind, ToolKind::Read, "{name} 应为只读类");
        assert!(!spec.sensitive_arguments, "{name} 是只读检索，不做强 ask");
        assert_eq!(spec.timeout_ms, 5_000, "{name} 超时不得漂移");
    }
}

#[test]
fn domain_tool_metadata_matches_card_table() {
    let registry = default_registry();
    for (name, kind, sensitive, timeout_ms) in DOMAIN_TOOLS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 未注册"));
        assert_eq!(spec.kind, kind, "{name} kind 与 D2 表不一致");
        assert_eq!(
            spec.sensitive_arguments, sensitive,
            "{name} sensitive_arguments 与 D2 表不一致"
        );
        assert_eq!(
            spec.timeout_ms, timeout_ms,
            "{name} timeout_ms 与 D2 表不一致"
        );
        assert!(
            spec.description.len() > 10 && spec.description.contains('⇒'),
            "{name} 描述必须写清「做什么 + 失败怎么表现」: {}",
            spec.description
        );
    }
}

#[test]
fn unregistered_tool_is_still_denied() {
    let mut allowed = mode_tools(&Mode::Craft);
    allowed.insert("fs.nuke".to_string());
    let gate = PermissionGate::new(default_registry())
        .with_whitelist(Mode::Craft, allowed)
        .with_auto_allowed("fs.nuke");
    let decision = gate.check(
        Mode::Craft,
        PermissionMode::Full,
        "fs.nuke",
        &json!({ "path": "a.txt" }),
    );
    match decision {
        Decision::Deny(reason) => assert!(
            reason.contains("工具未注册"),
            "拒绝理由必须指明未注册，实际: {reason}"
        ),
        other => panic!("未注册工具不得被白名单/auto_allowed 绕过，实际: {other:?}"),
    }

    // 对照：已注册工具不会被「未注册」门拦下（证明上一条断言不是空转）。
    let registered = gate.check(
        Mode::Craft,
        PermissionMode::Full,
        "fs.read",
        &json!({ "path": "a.txt" }),
    );
    if let Decision::Deny(reason) = registered {
        assert!(
            !reason.contains("工具未注册"),
            "已注册的 fs.read 不应落到未注册门: {reason}"
        );
    }
}
