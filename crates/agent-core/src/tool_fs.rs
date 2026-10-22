//! `fs.*` 工具的 spec 清单（域卡 `x-fs-wire` 产出，汇总卡 `x-tool-registry` 消费）。
//!
//! 契约（汇总卡按此核对，**逐字**）：
//!
//! ```ignore
//! pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)>
//! ```
//!
//! 五元组依次是 `(名字, 描述, kind, sensitive_arguments, timeout_ms)`。
//! 描述文本**必须**写清「做什么 + 失败怎么表现 + 能力边界」——模型据此判断
//! 何时调用、失败后是否重试；只写「执行某操作」这类空话会让模型无从判断结果。

use crate::tool::ToolKind;

/// 本域工具的 spec 清单：`(名字, 描述, kind, sensitive_arguments, timeout_ms)`。
///
/// 名字沿用**现有逻辑名**（`fs.read` / `fs.write` / `fs.list` / `fs.glob` /
/// `fs.search` / `fs.delete`），与 [`crate::mode`] 的白名单逐字一致；
/// 命名规范统一化（`namespace_tool` 化）是另一张卡的事，本域不改。
///
/// - 读类四项 ⇒ `ToolKind::Read` + `sensitive_arguments: false`（只读，不触发敏感动作扫描）；
/// - 写/删两项 ⇒ `ToolKind::Write` + `sensitive_arguments: true`（参数是用户内容，
///   可能含凭据样式串 ⇒ 让权限门的敏感动作扫描生效）；
/// - 超时：读类 5s，写/删类 10s（写盘可能更慢）。
pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)> {
    vec![
        (
            "fs.read",
            "读工作区内的文件内容。参数 path（相对工作区的路径，必填）、max_bytes（可选，读取字节上限，默认 8 MiB）。\
成功返回文件文本内容；路径越界、文件不存在、不是普通文件或超过上限 ⇒ 一律返回显式错误（不返回空内容、不静默截断）。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.list",
            "列出工作区内某个目录的直接子项（不递归）。参数 path（相对工作区的路径，可选，默认为工作区根）、\
offset（可选，默认 0）、limit（可选，默认 200，上限 1000）。成功返回条目列表与总条数；\
路径越界、目标不存在或不是目录 ⇒ 一律返回显式错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.glob",
            "按 glob **路径**模式匹配工作区内的文件（`**` 跨目录，`*` 与 `?` 不跨目录）。\
参数 pattern（必填）、root（可选，默认为工作区根）、limit（可选，默认 200，上限 1000）。\
成功返回匹配的相对路径数组；路径越界、目录不存在或模式非法 ⇒ 一律返回显式错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.search",
            "在工作区内按 **文件名** glob 模式递归搜索（匹配的是文件名，不是文件内容）。\
参数 pattern（必填）、root（可选，默认为工作区根）、limit（可选，默认 200，上限 1000）。\
成功返回命中项与命中总数；路径越界、目录不存在或模式非法 ⇒ 一律返回显式错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.write",
            "写入工作区内的文件（原子落盘：同目录临时文件 → rename 覆盖）。\
参数 path（相对工作区的路径，必填）、content（文件内容，必填）。\
覆盖**已存在**的文件前必须先读取过该文件（防盲覆盖），未读先覆盖、路径越界 ⇒ 一律返回显式错误。",
            ToolKind::Write,
            true,
            10_000,
        ),
        (
            "fs.delete",
            "删除工作区内的文件或目录（移入系统回收站，不直接抹除）。参数 path（相对工作区的路径，必填）。\
删除工作区根目录、路径越界或目标不存在 ⇒ 一律返回显式错误（避免一次调用清掉整个工作区）。",
            ToolKind::Write,
            true,
            10_000,
        ),
    ]
}
