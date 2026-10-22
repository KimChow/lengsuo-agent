//! 任务工具族的 spec 清单（`x-task-wire`：只描述，不注册）。
//!
//! 本文件是四个任务工具 spec 的**单一来源**；注册进 `default_registry()`、挂到
//! `DispatchToolExecutor` 由汇总卡 `x-tool-registry` 负责，执行体在
//! `runtime/task_tools.rs`，宿主侧调用适配见 [`crate::task_adapter`]。
//!
//! 工具名与 `crate::mode` 白名单逐字一致（`task.list` / `task.get` 只读，
//! `task.create` / `task.update` 写）；参数是任务元数据，不含凭据，
//! 因此 `sensitive_arguments = false`。

use crate::tool::ToolKind;

/// `task.list`：列本会话任务。
pub const TASK_LIST: &str = "task.list";
/// `task.get`：按 id 取本会话任务。
pub const TASK_GET: &str = "task.get";
/// `task.create`：建任务。
pub const TASK_CREATE: &str = "task.create";
/// `task.update`：改任务状态或文本。
pub const TASK_UPDATE: &str = "task.update";

/// 任务工具名全表（顺序即 spec 表顺序）。
pub const TASK_TOOLS: [&str; 4] = [TASK_LIST, TASK_GET, TASK_CREATE, TASK_UPDATE];

/// 本域工具的 spec 清单：(名字, 描述, kind, sensitive_arguments, timeout_ms)。
///
/// 描述写清「做什么 + 失败怎么表现 + 能力边界」；汇总卡按本表逐条构造
/// `ToolSpec` 注册即可，不得另行改写文案。
pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)> {
    vec![
        (
            TASK_LIST,
            "列出当前会话的任务看板（按创建时间升序）。无参数；没有任务时返回空列表 \
             {\"tasks\":[],\"count\":0}，这是正常结果而不是错误。只能看到本会话的任务，\
             不跨会话；会话不存在或读取任务失败 ⇒ 显式错误，不返回其他会话的数据；\
             本工具不改变任何状态、不落事件。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            TASK_GET,
            "按 id 读取当前会话的一个任务，返回任务完整字段（id/subject/description/status/\
             时间戳）。参数 id（UUID 字符串，必填）。id 不存在或不属于本会话时 ⇒ 显式报错 \
             「任务不存在」，不返回空对象；只读，不改变状态、不落事件。",
            ToolKind::Read,
            false,
            5_000,
        ),
        (
            TASK_CREATE,
            "在当前会话新建任务，初始状态 pending。参数 subject（字符串，必填，去空白后\
             不得为空）、description（字符串，可选，缺省空串）。subject 为空或参数形状\
             非法时 ⇒ 显式报错、不建任务；成功时落一条 task.created 事件并返回完整任务。\
             不能指定其他会话、状态或时间戳。",
            ToolKind::Write,
            false,
            5_000,
        ),
        (
            TASK_UPDATE,
            "更新当前会话的一个任务。参数 id（UUID 字符串，必填），以及 status / subject / \
             description 至少一项：status 只能是 pending|in_progress|done|cancelled，且须\
             遵守状态机 pending→in_progress→done|cancelled；非法迁移 ⇒ 显式报错且状态不变。\
             成功变更时落一条 task.updated 事件并返回更新后的完整任务；重复设置为同值不落事件。",
            ToolKind::Write,
            false,
            5_000,
        ),
    ]
}
