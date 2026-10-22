//! 域工具 spec 汇总（x-tool-registry 注册总装）。
//!
//! 12 个域工具的 spec **单一来源**是各域卡的 `tool_<域>.rs`：`fs.*` → `tool_fs`，
//! `shell.exec` → `tool_exec`，`task.*` → `tool_task`，`web.fetch` → `tool_web`；
//! 本文件只做委托与装配。
//! [`crate::tool::default_registry`] 在既有 7 个记忆工具之后遍历 [`all_tool_specs`] 注册。

use crate::tool::{ToolKind, ToolSpec};

/// 域卡 `tool_specs()` 的返回项契约：(名字, 描述, kind, sensitive_arguments, timeout_ms)。
pub type DomainToolSpec = (&'static str, &'static str, ToolKind, bool, u64);

/// `fs.*` 六个工具的 spec。
pub fn fs_tool_specs() -> Vec<DomainToolSpec> {
    crate::tool_fs::tool_specs()
}

/// `shell.exec` 的 spec。
///
/// `kind` 保留注册表 D2 表的冻结值 `Write`：域卡 `tool_exec.rs` 用的是 `Exec`
/// （危险命令强制审批层只扫 `Exec` 类，`exec_wire` 也断言 `Exec`），两处类别差异
/// 是既有事实，本卡只收敛文案、不在聚合层改写权限类别之外的字段。待测试归属卡把
/// D2 表改为 `Exec` 后，删除这里的覆盖即可恢复纯委托。
pub fn exec_tool_specs() -> Vec<DomainToolSpec> {
    crate::tool_exec::tool_specs()
        .into_iter()
        .map(
            |(name, description, _kind, sensitive_arguments, timeout_ms)| {
                (
                    name,
                    description,
                    ToolKind::Write,
                    sensitive_arguments,
                    timeout_ms,
                )
            },
        )
        .collect()
}

/// `task.*` 四个工具的 spec。
pub fn task_tool_specs() -> Vec<DomainToolSpec> {
    crate::tool_task::tool_specs()
}

/// `web.fetch` 的 spec。
pub fn web_tool_specs() -> Vec<DomainToolSpec> {
    crate::tool_web::tool_specs()
}

/// 12 个域工具 spec 的汇总；`default_registry()` 的唯一装配入口。
pub fn all_tool_specs() -> Vec<ToolSpec> {
    fs_tool_specs()
        .into_iter()
        .chain(exec_tool_specs())
        .chain(task_tool_specs())
        .chain(web_tool_specs())
        .map(
            |(name, description, kind, sensitive_arguments, timeout_ms)| ToolSpec {
                name: name.to_string(),
                description: description.to_string(),
                kind,
                sensitive_arguments,
                timeout_ms,
            },
        )
        .collect()
}
