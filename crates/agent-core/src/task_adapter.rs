//! 任务工具族的宿主侧适配：把一次工具调用接到 Runtime 既有的任务入口。
//!
//! 本层**不复制**参数解析、状态机或事件落盘——全部复用
//! [`crate::runtime::task_tools::TaskToolExecutor`]（它再经 `runtime/tasks.rs`
//! 走 `agent-tasks` 的会话作用域门）。适配只做两件事：
//!
//! 1. 认领校验：非 `task.*` 名字显式报错，不静默转交；
//! 2. 执行语境：经 [`Runtime::with_task_runtime`] 把「正在服务的 Runtime」带入
//!    线程语境，执行体的 `*_global` 入口据此取回同一份热态。
//!
//! 会话 id 由宿主在调用时绑定（与 `TaskToolExecutor::for_session` 同源），
//! 不从工具参数读取——模型无法伪造会话归属。

use serde_json::Value;
use uuid::Uuid;

use crate::r#loop::ToolExecutor;
use crate::runtime::TaskToolExecutor;
use crate::tool_task::TASK_TOOLS;
use crate::Runtime;

/// 工具名是否由任务适配器认领。
pub fn claims(name: &str) -> bool {
    TaskToolExecutor::claims(name)
}

/// 在 `rt` 的任务执行语境里执行一次 `task.*` 调用。
///
/// 未知工具名与所有执行失败都以可读 `Err(String)` 返回，由调用方结算成
/// `isError` 工具结果；本函数不 panic、不吞原因。
pub fn execute(rt: &Runtime, session: Uuid, name: &str, args: &Value) -> Result<Value, String> {
    if !claims(name) {
        return Err(format!(
            "未知任务工具: {name}（本适配器认领 {TASK_TOOLS:?}）"
        ));
    }
    let executor = TaskToolExecutor::for_session(session);
    rt.with_task_runtime(|| executor.execute(name, args))
        .map_err(|e| e.to_string())?
}
