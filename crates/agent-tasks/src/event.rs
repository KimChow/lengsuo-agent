//! 任务事件：任务域的 durable 事件形状（对照 参考客户端 session/event 的 durable 事件定义）。
//!
//! 事件是唯一真相：状态变更以事件落盘，读侧看板由事件投影而来，投影可重放且幂等。
//! 变体标签（`task.created` / `task.updated`）与字段名冻结；时间戳一律由调用方给出，
//! 事件路径不读时钟。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::task::{Task, TaskStatus};

/// 任务域 durable 事件；snake_case 点分标签，字段名复用 [`Task`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaskEvent {
    /// 任务创建：快照即创建时刻的完整任务（对照 session.created 携带全部初始字段）。
    #[serde(rename = "task.created")]
    Created {
        task_id: Uuid,
        occurred_at_ms: u64,
        snapshot: Task,
    },
    /// 任务更新：只携带变化的字段，可缺省（对照 session.renamed 等单字段更新事件）。
    #[serde(rename = "task.updated")]
    Updated {
        task_id: Uuid,
        occurred_at_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<TaskStatus>,
    },
}

impl TaskEvent {
    /// 事件发生时间（Unix 毫秒），由发射方给定。
    pub fn occurred_at_ms(&self) -> u64 {
        match self {
            TaskEvent::Created { occurred_at_ms, .. }
            | TaskEvent::Updated { occurred_at_ms, .. } => *occurred_at_ms,
        }
    }

    /// 事件所属任务。
    pub fn task_id(&self) -> Uuid {
        match self {
            TaskEvent::Created { task_id, .. } | TaskEvent::Updated { task_id, .. } => *task_id,
        }
    }

    /// 重放排序键：先 `occurred_at_ms` 升序，同刻按 `task_id` 升序；同任务的
    /// `created` 先于 `updated`（否则 updated 会因目标不存在被丢弃）。
    /// 对照参考件由 durable.seq 全序化；本卡事件形状无 seq，以 (时间, 任务) 近似。
    pub(crate) fn replay_order(&self) -> (u64, Uuid, u8) {
        let variant = match self {
            TaskEvent::Created { .. } => 0,
            TaskEvent::Updated { .. } => 1,
        };
        (self.occurred_at_ms(), self.task_id(), variant)
    }
}
