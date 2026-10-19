//! ACP 任务四方法类型（27 册 §2「ACP 任务」族）。
//!
//! `tasks/list` / `tasks/get` / `tasks/cancel` / `tasks/result`——ACP 自带
//! 任务生命周期。注意与 Agent 工具的 TaskOutput 分属两层：**协议层任务 ≠
//! 工具层任务**（27 册 §2 原文），本模块只建协议层。
//!
//! 27/19/18 册对四方法仅实证到方法名一级，字段层证据缺失；按「宁可缺、
//! 不许编」原则取最小字段集：id + 状态 + 分页 cursor + 产物 open 载荷，
//! 其余一律不建（缺口见任务报告偏离清单）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 任务 id：ACP 字符串形态（与会话 id 同风格）。
pub type TaskId = String;

/// 任务状态机（含终态；19 册 §4「回退到终态」实证任务态有终态语义，
/// 19 册 §7「Failed or cancelled … are not live」实证 failed/cancelled 终态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已创建未开始。
    Pending,
    /// 执行中。
    InProgress,
    /// 完成（终态）。
    Completed,
    /// 失败（终态）。
    Failed,
    /// 已取消（终态，tasks/cancel 生效）。
    Cancelled,
}

/// 协议层任务实体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// 任务 id。
    pub task_id: TaskId,
    /// 当前状态。
    pub status: TaskStatus,
}

/// `tasks/list` 请求参数（cursor 分页风格，与本仓分页类型命名习惯一致：
/// 入参 `cursor`、出参 `nextCursor`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksListParams {
    /// 分页锚点（首页缺省）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `tasks/list` 响应结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksListResult {
    /// 当前页任务目录。
    pub tasks: Vec<Task>,
    /// 下一页锚点（无则列表到底）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// `tasks/get` 请求参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksGetParams {
    /// 目标任务 id。
    pub task_id: TaskId,
}

/// `tasks/get` 响应结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksGetResult {
    /// 任务实体。
    pub task: Task,
}

/// `tasks/cancel` 请求参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksCancelParams {
    /// 目标任务 id。
    pub task_id: TaskId,
}

/// `tasks/cancel` 响应结果：取消受理即成功，结果体为空对象
/// （同 `logout` 的空参数占位风格）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TasksCancelResult {}

/// `tasks/result` 请求参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksResultParams {
    /// 目标任务 id。
    pub task_id: TaskId,
}

/// `tasks/result` 响应结果：任务产物/输出载荷（open 结构 + `_meta` 预留）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksResultResult {
    /// 产物/输出载荷（内部结构原样保留，前向兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    /// 厂商私有元数据（27 册 §1 `codebuddy.ai/*` 命名空间的载体；wire 键 `_meta`）。
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// 方法名集中表：与上面类型一一对应（27 册 §2 斜杠方法名）。
pub mod method {
    /// `tasks/list`
    pub const TASKS_LIST: &str = "tasks/list";
    /// `tasks/get`
    pub const TASKS_GET: &str = "tasks/get";
    /// `tasks/cancel`
    pub const TASKS_CANCEL: &str = "tasks/cancel";
    /// `tasks/result`
    pub const TASKS_RESULT: &str = "tasks/result";
}
