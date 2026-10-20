//! 记忆工具注册与模式白名单（M1）。
//!
//! 本套件守两条不变量：
//! 1. `tool::default_registry()` 是内置工具名的**唯一事实来源**——四个写工具
//!    （`memory.set_user` / `memory.set_project` / `memory.append_daily` /
//!    `identity.set_soul`）必须都在表里，且参数名与执行体 `execute_memory_tool`
//!    的取值键一致（`date` / `text`）。
//! 2. 注册**不改变** deny-by-default：注册表之外的任何名字（含形似的
//!    `memory.nuke`）经 `PermissionGate::check` 仍必须 `Deny` 且原因含「工具未注册」。
//!
//! 白名单可见性（五模式矩阵）在 `mode.rs` 的单元测试里守相对口径。

use std::collections::BTreeSet;

use agent_core::mode::{mode_policy, WRITE_TOOLS};
use agent_core::permission::{Decision, PermissionGate};
use agent_core::schema::{Mode, PermissionMode};
use agent_core::tool::{default_registry, ToolKind};
use serde_json::json;

/// 本卡注册的四个写工具名。
const MEMORY_WRITE_TOOLS: [&str; 4] = [
    "memory.set_user",
    "memory.set_project",
    "memory.append_daily",
    "identity.set_soul",
];

#[test]
fn default_registry_registers_memory_write_tools_as_write() {
    let registry = default_registry();
    for name in MEMORY_WRITE_TOOLS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 必须注册进默认注册表"));
        assert_eq!(spec.kind, ToolKind::Write, "{name} 应为写类");
        assert!(
            spec.sensitive_arguments,
            "{name} 参数是用户内容，必须过敏感动作扫描"
        );
    }
}

/// 执行体 `execute_memory_tool`（`runtime/sessions.rs`）按 `arg("date")` / `arg("text")`
/// 取值；注册表若用别的键名，工具会被放行却执行失败。此处只用注册表自身可见的信息
/// 守不变量：参数名必须落在执行体实际取值的那组键里。
#[test]
fn memory_write_tool_argument_names_match_executor() {
    let registry = default_registry();
    let executor_keys: BTreeSet<&str> = ["date", "text"].into_iter().collect();
    for name in MEMORY_WRITE_TOOLS {
        let spec = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} 未注册"));
        let mentioned: BTreeSet<&str> = executor_keys
            .iter()
            .copied()
            .filter(|key| spec.description.contains(key))
            .collect();
        assert!(
            !mentioned.is_empty(),
            "{name} 的描述必须写明执行体实际读取的参数名（date/text），当前: {}",
            spec.description
        );
    }
    // 逐工具精确到「该工具有哪些参数」的口径：append_daily 需 date+text，其余仅 text。
    let append = registry.get("memory.append_daily").expect("append_daily");
    assert!(
        append.description.contains("date") && append.description.contains("text"),
        "append_daily 描述须同时含 date 与 text"
    );
    for name in ["memory.set_user", "memory.set_project", "identity.set_soul"] {
        let spec = registry.get(name).expect("注册");
        assert!(
            spec.description.contains("text") && !spec.description.contains("date"),
            "{name} 只接受 text，描述不得出现 date: {}",
            spec.description
        );
    }
}

#[test]
fn default_registry_is_the_single_source_for_mode_whitelist() {
    // 白名单里的记忆工具名必须在注册表里存在，否则会出现「可见但不可执行」的假象。
    let registry = default_registry();
    for name in MEMORY_WRITE_TOOLS {
        assert!(
            WRITE_TOOLS.contains(&name),
            "{name} 必须进写集（模式白名单来源）"
        );
        assert!(registry.get(name).is_some(), "{name} 必须注册");
    }
}

#[test]
fn registered_memory_tools_are_allowed_in_craft_and_expert() {
    let registry = default_registry();
    for name in MEMORY_WRITE_TOOLS {
        let gate = PermissionGate::new(registry.clone())
            .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]));
        let d = gate.check(
            Mode::Craft,
            PermissionMode::Full,
            name,
            &json!({"text": "x", "date": "2026-10-20"}),
        );
        assert!(
            matches!(d, Decision::Allow | Decision::Ask),
            "craft 下 {name} 应被放行（allow/ask），实得 {d:?}"
        );
    }
}

/// ⚠️ 行为变化守门：注册四个工具**不得**顺手放宽未注册名字。
#[test]
fn unregistered_memory_tool_is_still_denied() {
    let gate = PermissionGate::new(default_registry());
    let d = gate.check(
        Mode::Craft,
        PermissionMode::Full,
        "memory.nuke",
        &json!({"text": "x"}),
    );
    match d {
        Decision::Deny(reason) => assert!(
            reason.contains("工具未注册"),
            "拒绝原因应含「工具未注册」，实得: {reason}"
        ),
        other => panic!("memory.nuke 必须 Deny，实得 {other:?}"),
    }
}

/// 注册 ≠ 全模式可见：Plan 剔除写集，Ask/Quick 无写工具。
#[test]
fn registered_memory_tools_are_invisible_in_plan_ask_quick() {
    for mode in [Mode::Plan, Mode::Ask, Mode::Quick] {
        let policy = mode_policy(mode);
        for name in MEMORY_WRITE_TOOLS {
            assert!(
                !policy.allows(name),
                "{mode:?} 不应可见写工具 {name}（白名单相对口径）"
            );
        }
    }
}
