//! 任务看板：内存任务表 + 会话索引（规格冻结点：方法签名与 JSON 字段名）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use uuid::Uuid;

use crate::error::TaskError;
use crate::event::TaskEvent;
use crate::task::{Task, TaskStatus};

/// 可注入时钟（Unix 毫秒）；确定性测试用固定值，避免时间抖动。
pub type NowMs = fn() -> u64;

/// 会话内任务看板。纯内存，不落盘；持久化由上层会话存储负责（对照 参考客户端
/// 「任务存内存 + 会话存储」）。
#[derive(Debug, Clone)]
pub struct TaskBoard {
    tasks: HashMap<Uuid, Task>,
    /// 会话索引：session_id → 该会话的任务 id 列表。
    by_session: HashMap<Uuid, Vec<Uuid>>,
    now: NowMs,
}

impl Default for TaskBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskBoard {
    /// 使用系统时钟新建看板。
    pub fn new() -> Self {
        Self::with_clock(system_now_ms)
    }

    /// 使用注入时钟新建看板（测试用，保证时间戳确定）。
    pub fn with_clock(now: NowMs) -> Self {
        Self {
            tasks: HashMap::new(),
            by_session: HashMap::new(),
            now,
        }
    }

    /// 创建任务：subject 去空白后非空，初始状态 `pending`。
    pub fn create(
        &mut self,
        session_id: Uuid,
        subject: &str,
        description: &str,
    ) -> Result<Task, TaskError> {
        if subject.trim().is_empty() {
            return Err(TaskError::EmptySubject);
        }
        let now_ms = (self.now)();
        let task = Task {
            id: Uuid::new_v4(),
            session_id,
            subject: subject.to_string(),
            description: description.to_string(),
            status: TaskStatus::default(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        self.tasks.insert(task.id, task.clone());
        self.by_session.entry(session_id).or_default().push(task.id);
        Ok(task)
    }

    pub fn get(&self, id: Uuid) -> Result<&Task, TaskError> {
        self.tasks.get(&id).ok_or(TaskError::NotFound)
    }

    /// 会话作用域读取门：仅当任务存在且 `task.session_id == session_id` 时复用 [`get`](Self::get)。
    /// owner 不匹配与任务不存在都返回同一个 `TaskError::NotFound`，调用方无法区分，
    /// 因此不泄露他人任务的存在性与内容。
    /// 上层 Runtime 工具（未来的 task 工具族）必须走本门，而不是 [`get`](Self::get)。
    pub fn get_for_session(&self, session_id: Uuid, id: Uuid) -> Result<&Task, TaskError> {
        match self.tasks.get(&id) {
            Some(task) if task.session_id == session_id => Ok(task),
            _ => Err(TaskError::NotFound),
        }
    }

    /// 会话作用域状态更新门：owner 校验通过才复用 [`update_status`](Self::update_status)
    /// （状态机、幂等与时间戳单调语义完全一致）；owner 不匹配或任务不存在返回
    /// `TaskError::NotFound` 且不修改状态与 `updated_at_ms`。
    pub fn update_status_for_session(
        &mut self,
        session_id: Uuid,
        id: Uuid,
        to: TaskStatus,
    ) -> Result<Task, TaskError> {
        if self.get_for_session(session_id, id).is_err() {
            return Err(TaskError::NotFound);
        }
        self.update_status(id, to)
    }

    /// 状态迁移：同状态幂等返回当前任务；非法迁移报 `InvalidTransition`；
    /// `done`/`cancelled` 为终态，不可再出。
    pub fn update_status(&mut self, id: Uuid, to: TaskStatus) -> Result<Task, TaskError> {
        let task = self.tasks.get_mut(&id).ok_or(TaskError::NotFound)?;
        if task.status == to {
            return Ok(task.clone());
        }
        if !task.status.can_transition(to) {
            return Err(TaskError::InvalidTransition {
                from: task.status,
                to,
            });
        }
        task.status = to;
        Self::bump(&mut task.updated_at_ms, (self.now)());
        Ok(task.clone())
    }

    /// 更新文本：给出的 subject 不得为空白；字段未变化则不刷新 updated_at_ms。
    pub fn update_text(
        &mut self,
        id: Uuid,
        subject: Option<&str>,
        description: Option<&str>,
    ) -> Result<Task, TaskError> {
        if let Some(subject) = subject {
            if subject.trim().is_empty() {
                return Err(TaskError::EmptySubject);
            }
        }
        let task = self.tasks.get_mut(&id).ok_or(TaskError::NotFound)?;
        let mut changed = false;
        if let Some(subject) = subject {
            if task.subject != subject {
                task.subject = subject.to_string();
                changed = true;
            }
        }
        if let Some(description) = description {
            if task.description != description {
                task.description = description.to_string();
                changed = true;
            }
        }
        if changed {
            Self::bump(&mut task.updated_at_ms, (self.now)());
        }
        Ok(task.clone())
    }

    /// 单个事件投影到看板（对照 参考客户端 projector 逐条应用 durable 事件）。
    /// 返回 `true` 表示事件被接受；`false` 表示事件是重复/未知目标/非法载荷，被
    /// 幂等丢弃，看板状态不变：
    /// - `task.created`：id 不存在 ⇒ 插入快照；已存在 ⇒ 保留首个快照不覆盖
    ///   （对照 insert `onConflictDoNothing` + 重复即缺陷不重写行）。
    /// - `task.updated`：id 存在 ⇒ 只改给出字段；id 不存在 ⇒ 静默丢弃，不凭空插入
    ///   （对照 SQL `update ... where id = ?` 空匹配的 no-op）。
    ///
    /// 状态机与 subject 校验不被事件路径绕过：任一字段非法即整事件拒绝，无部分更新。
    pub fn apply(&mut self, event: &TaskEvent) -> bool {
        match event {
            TaskEvent::Created {
                task_id, snapshot, ..
            } => self.apply_created(*task_id, snapshot),
            TaskEvent::Updated {
                task_id,
                occurred_at_ms,
                subject,
                status,
            } => self.apply_updated(*task_id, *occurred_at_ms, subject.as_deref(), *status),
        }
    }

    /// created 的投影：`task_id` 与快照 id 不一致视为畸形事件拒绝；重复投递保留
    /// 既有行（含 `created_at_ms`）。插入同时维护会话索引，`(created_at, id)`
    /// 排序不变式由 `list_by_session` 读侧排序保证，任何路径都不改既有 created_at。
    fn apply_created(&mut self, task_id: Uuid, snapshot: &Task) -> bool {
        if task_id != snapshot.id || self.tasks.contains_key(&task_id) {
            return false;
        }
        self.tasks.insert(task_id, snapshot.clone());
        self.by_session
            .entry(snapshot.session_id)
            .or_default()
            .push(task_id);
        true
    }

    /// updated 的投影：校验先于写入（与 `update_text`/`update_status` 同一规则），
    /// `updated_at_ms` 只在有字段实际变化时以事件时间单调前移。
    fn apply_updated(
        &mut self,
        task_id: Uuid,
        occurred_at_ms: u64,
        subject: Option<&str>,
        status: Option<TaskStatus>,
    ) -> bool {
        let Some(task) = self.tasks.get_mut(&task_id) else {
            return false;
        };
        if let Some(subject) = subject {
            if subject.trim().is_empty() {
                return false;
            }
        }
        if let Some(to) = status {
            if task.status != to && !task.status.can_transition(to) {
                return false;
            }
        }
        let mut changed = false;
        if let Some(subject) = subject {
            if task.subject != subject {
                task.subject = subject.to_string();
                changed = true;
            }
        }
        if let Some(to) = status {
            if task.status != to {
                task.status = to;
                changed = true;
            }
        }
        if changed {
            Self::bump(&mut task.updated_at_ms, occurred_at_ms);
        }
        true
    }

    /// 由事件序列重建看板：按 `occurred_at_ms` 升序、同刻按 `task_id` 升序应用，
    /// 与输入顺序无关（确定性）。对照参考件「读侧从 durable 事件日志投影、可重放」。
    pub fn replay(events: &[TaskEvent]) -> TaskBoard {
        let mut ordered: Vec<&TaskEvent> = events.iter().collect();
        ordered.sort_by_key(|event| event.replay_order());
        let mut board = TaskBoard::new();
        for event in ordered {
            board.apply(event);
        }
        board
    }

    /// 会话内任务列表：按 created_at_ms 升序；同毫秒按 id 升序，保证顺序可复现。
    pub fn list_by_session(&self, session_id: Uuid) -> Vec<&Task> {
        let Some(ids) = self.by_session.get(&session_id) else {
            return Vec::new();
        };
        let mut items: Vec<&Task> = ids.iter().filter_map(|id| self.tasks.get(id)).collect();
        items.sort_by(|a, b| {
            a.created_at_ms
                .cmp(&b.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        items
    }

    /// 任务的 JSON 表示；字段名与状态字符串冻结（轨道 E 直接依赖）。
    pub fn to_json(&self, id: Uuid) -> Result<Value, TaskError> {
        let task = self.get(id)?;
        serde_json::to_value(task).map_err(|e| TaskError::Json(e.to_string()))
    }

    /// 刷新更新时间：只前进不回拨（对照 PC26 advanceActivity 的「只前进」原则）。
    /// 注入时钟回退时取历史最大值，保证 `updated_at_ms` 对该任务单调非递减。
    fn bump(slot: &mut u64, now_ms: u64) {
        *slot = (*slot).max(now_ms);
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
