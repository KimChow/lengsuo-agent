//! 域工具 spec 汇总（x-tool-registry 注册总装）。
//!
//! 本文件是 12 个域工具的**唯一 spec 来源**：`fs.*` / `shell.exec` / `task.*` / `web.fetch`。
//! [`crate::tool::default_registry`] 在既有 7 个记忆工具之后遍历 [`all_tool_specs`] 注册。
//!
//! # 域卡契约（D3）
//!
//! 每张域卡在自己的 `tool_<域>.rs` 里导出：
//!
//! ```ignore
//! /// 本域工具的 spec 清单：(名字, 描述, kind, sensitive, timeout_ms)
//! pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)>;
//! ```
//!
//! 描述必须写清「做什么 + 失败怎么表现」（含路径越界 ⇒ 显式错误、不回显凭据），
//! 不得写成「执行某操作」这类空话。域卡文件与 `lib.rs` 的模块声明合入后，
//! 把下面对应的 `*_tool_specs()` 实现换成对该 `tool_specs()` 的直接调用即可——
//! [`all_tool_specs`] 的签名与调用方都不用动。
//!
//! 本卡合入时四个域模块（`tool_fs` / `tool_exec` / `tool_task` / `tool_web`）**均未存在**，
//! 因此这里按卡面 D2 表集中定义 spec；执行体接入情况见 `docs/tool-registry-status.md`。

use crate::tool::{ToolKind, ToolSpec};

/// 域卡 `tool_specs()` 的返回项契约：(名字, 描述, kind, sensitive_arguments, timeout_ms)。
pub type DomainToolSpec = (&'static str, &'static str, ToolKind, bool, u64);

/// `fs.*` 六个工具的 spec（`x-fs-wire` 合入后由其 `tool_specs()` 替换）。
pub fn fs_tool_specs() -> Vec<DomainToolSpec> {
    vec![
        (
            "fs.read",
            "读取工作区内一个 UTF-8 文本文件；参数 path（必填，相对工作区）。\
             路径越出工作区 ⇒ 明确报错且不读取；文件不存在、过大或非 UTF-8 ⇒ 可读错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.list",
            "列出工作区内一个目录的直接子项（不递归）；参数 path（可选，默认工作区根）、\
             offset / limit（可选，分页）。路径越界 ⇒ 明确报错；目标不是目录 ⇒ 可读错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.glob",
            "按路径模式匹配工作区文件（`**` 跨目录，`*` / `?` 不跨 `/`）；\
             参数 pattern（必填）、root（可选）、limit（可选）。\
             路径越界或模式非法 ⇒ 明确报错。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.search",
            "按文件名 glob 递归查找工作区文件（只看文件名，不读文件内容）；\
             参数 pattern（必填）、root（可选）、limit（可选）。\
             路径越界或模式非法 ⇒ 明确报错。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "fs.write",
            "写入或覆盖工作区文件；参数 path（必填）、content（必填）。\
             路径越界 ⇒ 明确报错；错误只带路径与原因，不回显文件内容或凭据。",
            ToolKind::Write,
            true,
            10_000,
        ),
        (
            "fs.delete",
            "删除工作区文件或目录（进回收站，不直删）；参数 path（必填）。\
             路径越界或目标是工作区根 ⇒ 拒绝并明确报错；目标不存在 ⇒ 可读错误。",
            ToolKind::Write,
            true,
            10_000,
        ),
    ]
}

/// `shell.exec` 的 spec（`x-exec-wire` 合入后由其 `tool_specs()` 替换）。
pub fn exec_tool_specs() -> Vec<DomainToolSpec> {
    vec![(
        "shell.exec",
        "在工作区内执行一条 shell 命令；参数 command（必填，按 shell 语义分词）、cwd（可选）。\
         策略拒绝或待审批 ⇒ 明确错误；沙箱执行超时或非零退出 ⇒ 如实返回 exit_code 与 timed_out；\
         错误不回显命令中的凭据。",
        ToolKind::Write,
        true,
        30_000,
    )]
}

/// `task.*` 四个工具的 spec（`x-task-wire` 合入后由其 `tool_specs()` 替换）。
pub fn task_tool_specs() -> Vec<DomainToolSpec> {
    vec![
        (
            "task.list",
            "列出当前会话的任务；无参数。会话不存在或读取任务失败 ⇒ 可读错误，\
             不返回其他会话的数据。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "task.get",
            "读取当前会话的单个任务；参数 id（必填，UUID）。\
             id 非法、任务不存在或不属于本会话 ⇒ 可读错误。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            "task.create",
            "为当前会话创建任务；参数 subject（必填，去空白后非空）、description（可选）。\
             subject 为空或写入失败 ⇒ 可读错误，不创建半成品任务。",
            ToolKind::Write,
            false,
            5_000,
        ),
        (
            "task.update",
            "更新当前会话的任务；参数 id（必填）、\
             status（可选：pending | in_progress | done | cancelled）、subject / description（可选）。\
             id 非法、任务不存在或未给任何变更字段 ⇒ 可读错误。",
            ToolKind::Write,
            false,
            5_000,
        ),
    ]
}

/// `web.fetch` 的 spec（`x-web-wire` 合入后由其 `tool_specs()` 替换）。
pub fn web_tool_specs() -> Vec<DomainToolSpec> {
    vec![(
        "web.fetch",
        "抓取一个 URL 的文本内容；参数 url（必填，仅 http / https）。\
         URL 非法、协议不支持或请求失败 ⇒ 可读错误；只读取响应正文，不落盘，\
         错误不回显请求头中的凭据。",
        ToolKind::Read,
        true,
        30_000,
    )]
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
