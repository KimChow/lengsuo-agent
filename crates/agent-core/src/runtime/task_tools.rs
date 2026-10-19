//! 任务工具族执行器（W1 打通卡）：把逻辑工具名 `task.*` 适配到 Runtime 既有的
//! 任务能力面（[`super::tasks`]），使模型请求的 `task.create` 等**真实执行**。
//!
//! 产品行为定义（工具名、语义、参数意图）对齐参考件 21 册「工具宇宙」的
//! TaskCreate/TaskGet/TaskUpdate/TaskList 与 18 册 §7 任务管理纪律（建→进行中→
//! 完成，标记完成不得攒批）。**不新增工具名**：白名单（`crate::mode`）里已有
//! `task.list` / `task.get` / `task.create` / `task.update` 四个名字，本模块只补执行。
//!
//! 三层界限（本模块的全部职责就是这一层薄适配）：
//!
//! | 层 | 归属 | 本模块的态度 |
//! | --- | --- | --- |
//! | 数据类型与 JSON 形状 | `agent-tasks`（域层，冻结） | 直接用 `Task` 的 serde，不另造字段名 |
//! | 会话作用域门 | [`Runtime::create_task`] 等五个入口 | **必须复用**，不碰 `TaskBoard` |
//! | 工具名 → 入口的参数适配 | 本模块 | 唯一新增内容 |
//!
//! 会话 id 的传递：`ToolExecutor::execute` 的签名里**没有会话 id**（trait 体属
//! `loop` 模块，本卡不得改），因此执行器在**构造时绑定会话**
//! （[`TaskToolExecutor::for_session`]），运行时再补一道会话存在性校验
//! （[`Runtime::get_session`]）：伪造/陈旧 id ⇒ 可读错误，而不是静默作用到空气上。
//!
//! 错误形状：一切失败都返回 `Err(String)`（可读中文），由既有
//! `Runtime::execute_tool` 的 `Decision::Allow` 分支结算成
//! `{"type":"error","message":..}` + `isError=true`，回合继续、不 panic。

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use agent_tasks::TaskStatus;

use crate::error::CoreError;
use crate::r#loop::ToolExecutor;

use super::Task;

/// 四个任务工具名（与 `crate::mode` 白名单逐字一致，不与 `TaskEvent` 的
/// `task.created` / `task.updated` 事件 kind 混淆）。
pub const TASK_LIST: &str = "task.list";
pub const TASK_GET: &str = "task.get";
pub const TASK_CREATE: &str = "task.create";
pub const TASK_UPDATE: &str = "task.update";

/// 任务工具名全表（装配与文档的单一来源）。
pub const TASK_TOOLS: [&str; 4] = [TASK_LIST, TASK_GET, TASK_CREATE, TASK_UPDATE];

/// `task.create` 参数。`subject` 必填（域层要求去空白后非空），`description` 可选
/// （缺省空串，与 [`Runtime::create_task`] 的入参形状一致）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    subject: String,
    #[serde(default)]
    description: String,
}

/// `task.get` 参数：`id` 必填。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetArgs {
    id: Uuid,
}

/// `task.update` 参数：`id` 必填；`status` / `subject` / `description` 至少给一个。
/// `status` 用域层状态词（`pending|in_progress|done|cancelled`）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: Uuid,
    #[serde(default)]
    status: Option<TaskStatus>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// `task.*` 工具族的执行器：把工具名适配到 Runtime 的会话作用域任务入口。
///
/// 会话 id 在**构造时绑定**（`ToolExecutor::execute` 的签名里没有它）：宿主按
/// 「哪个会话在跑」取一个执行器，经 [`Runtime::install_task_tools`] 或
/// [`RuntimeBuilder::tool_executor`](super::RuntimeBuilder::tool_executor) 装上。
/// 会话 id 由宿主提供、执行器只负责带上它——认证属上层（同 `task-owned-access-status`
/// 的既有口径）。
#[derive(Clone)]
pub struct TaskToolExecutor {
    session: Uuid,
}

impl TaskToolExecutor {
    /// 为指定会话构造执行器（无状态：Runtime 由执行语境提供，见
    /// [`Runtime::with_task_runtime`]）。
    ///
    /// 供按会话存储执行器的宿主使用；只想「装上就完事」的宿主直接用
    /// [`Runtime::install_task_tools`]。
    pub fn for_session(session: Uuid) -> Self {
        Self { session }
    }

    /// 本执行器绑定的会话 id。
    pub fn session_id(&self) -> Uuid {
        self.session
    }

    /// 工具名是否由本执行器认领（装配自检与文档的单一来源）。
    pub fn claims(name: &str) -> bool {
        TASK_TOOLS.contains(&name)
    }
}

impl ToolExecutor for TaskToolExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        dispatch(self.session, name, args)
    }
}

/// 工具名 → Runtime 任务入口的唯一分派面。
///
/// **读类两步都读活态**：`task.list` 直读看板，`task.get` / `task.update` 的目标存在性
/// 校验由 [`Runtime::get_task`] 在调用时完成（`update` 的第一句就是它）——所以不缓存任务，
/// 也不信任上游给过的 id。
fn dispatch(session: Uuid, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        TASK_LIST => list(session, args),
        TASK_GET => get(session, args),
        TASK_CREATE => create(session, args),
        TASK_UPDATE => update(session, args),
        other => Err(unknown_tool(other)),
    }
}

/// 未知工具名错误文案（与 `Runtime::execute_memory_tool` 的既有措辞同构）。
fn unknown_tool(name: &str) -> String {
    format!("未知任务工具: {name}（本执行器认领 {TASK_TOOLS:?}）")
}

/// 参数解析：JSON 形状错误 ⇒ 可读错误（不 panic，不静默取默认值）。
fn parse<T: for<'de> Deserialize<'de>>(tool: &str, args: &Value) -> Result<T, String> {
    serde_json::from_value(args.clone()).map_err(|e| format!("{tool} 参数非法: {e}"))
}

/// 任务 → 结果值：直接用域层 `Task` 的 serde 形状（字段名不与 `agent-tasks` 分叉）。
fn to_value(task: &Task) -> Result<Value, String> {
    serde_json::to_value(task).map_err(|e| format!("任务序列化失败: {e}"))
}

/// `task.list`：会话作用域列表。无参数；结果 `{"tasks":[Task,..], "count":n}`。
fn list(session: Uuid, args: &Value) -> Result<Value, String> {
    // 兼容面：接受省略、`null` 或空对象；任何具名参数都报错（不静默忽略）。
    match args {
        Value::Null => {}
        Value::Object(map) if map.is_empty() => {}
        _ => return Err(format!("{TASK_LIST} 不接受参数: {args}")),
    }
    let tasks: Vec<Value> = super::Runtime::list_tasks_global(session)
        .iter()
        .map(to_value)
        .collect::<Result<_, _>>()?;
    Ok(json!({ "tasks": tasks, "count": tasks.len() }))
}

/// `task.get`：会话作用域读取单个任务（越权/不存在同报 `任务不存在`）。
fn get(session: Uuid, args: &Value) -> Result<Value, String> {
    let GetArgs { id } = parse(TASK_GET, args)?;
    let task = super::Runtime::get_task_global(session, id).map_err(core_err)?;
    to_value(&task)
}

/// `task.create`：建任务（初始 `pending`），落 `task.created` 事件。
fn create(session: Uuid, args: &Value) -> Result<Value, String> {
    let CreateArgs {
        subject,
        description,
    } = parse(TASK_CREATE, args)?;
    let task =
        super::Runtime::create_task_global(session, &subject, &description).map_err(core_err)?;
    to_value(&task)
}

/// `task.update`：状态或文本更新。
///
/// - 只给 `status` ⇒ 走 [`Runtime::update_task_status`]（状态机 + 幂等 + 时间戳语义）；
/// - 给了 `subject` / `description` ⇒ 走 [`Runtime::update_task_text`]；
/// - 两者同时给 ⇒ **先状态后文本**（两次独立结算，失败即返回 `Err`，已成功的那次
///   保留——与模型分两次调用的结果一致）；文本变更只作审计标记，见
///   `docs/runtime-task-events-status.md` 的已知缺口。
/// - 三者都没给 ⇒ 可读错误（不静默成功）。
fn update(session: Uuid, args: &Value) -> Result<Value, String> {
    let UpdateArgs {
        id,
        status,
        subject,
        description,
    } = parse(TASK_UPDATE, args)?;
    let text_given = subject.is_some() || description.is_some();
    if status.is_none() && !text_given {
        return Err(format!(
            "{TASK_UPDATE} 至少需要 status / subject / description 之一"
        ));
    }
    let mut task = super::Runtime::get_task_global(session, id).map_err(core_err)?;
    if let Some(to) = status {
        task = super::Runtime::update_task_status_global(session, id, to).map_err(core_err)?;
    }
    if text_given {
        task = super::Runtime::update_task_text_global(
            session,
            id,
            subject.as_deref(),
            description.as_deref(),
        )
        .map_err(core_err)?;
    }
    to_value(&task)
}

/// `CoreError` → 可读错误字符串：透传域层文案（`任务不存在` / `非法状态迁移: ..` /
/// `subject 不能为空`），不吞原因、不改写形状。
fn core_err(e: CoreError) -> String {
    e.to_string()
}
