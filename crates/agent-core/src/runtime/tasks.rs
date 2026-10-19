//! 任务工具族的 Runtime 域层能力面（对照 参考客户端 TaskCreate/TaskGet/TaskUpdate/TaskList）。
//!
//! 只做接线：`TaskBoard` 挂在 Runtime 上，读写全部经 `agent-tasks` 的
//! `*_for_session` 会话作用域门（`docs/task-owned-access-status.md` 末段冻结约束），
//! 状态机、幂等与时间戳语义完全复用域层，本模块不复制任何任务逻辑。
//! 越权与不存在同报 `TaskError::NotFound`（映射为 `CoreError::Task`，文案不变），
//! 不泄露他人任务的存在性；拒绝路径零副作用。
//! 工具注册与执行器属 W3，本模块不碰 `tool.rs`。

use uuid::Uuid;

pub use agent_tasks::{Task, TaskStatus};

use crate::error::CoreError;

use super::Runtime;

impl From<agent_tasks::TaskError> for CoreError {
    /// 透传 `TaskError` 原始文案（NotFound/InvalidTransition/EmptySubject/Json），
    /// 不吞原因；网关层按 `CoreError::Task` 变体归一映射信封 code。
    fn from(e: agent_tasks::TaskError) -> Self {
        CoreError::Task(e.to_string())
    }
}

impl Runtime {
    /// 建任务（TaskCreate 行为面）：subject 去空白后非空，初始 `pending`。
    pub fn create_task(
        &self,
        session_id: Uuid,
        subject: &str,
        description: &str,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        board
            .create(session_id, subject, description)
            .map_err(Into::into)
    }

    /// 会话内任务列表（TaskList 行为面）：按 created_at_ms 升序、同毫秒按 id 升序。
    pub fn list_tasks(&self, session_id: Uuid) -> Vec<Task> {
        let board = self.tasks.lock().expect("任务看板锁不得毒化");
        board
            .list_by_session(session_id)
            .into_iter()
            .cloned()
            .collect()
    }

    /// 会话作用域读取（TaskGet 行为面）：越权与不存在同报 NotFound。
    pub fn get_task(&self, session_id: Uuid, task_id: Uuid) -> Result<Task, CoreError> {
        let board = self.tasks.lock().expect("任务看板锁不得毒化");
        Ok(board.get_for_session(session_id, task_id)?.clone())
    }

    /// 会话作用域状态更新（TaskUpdate 行为面）：owner 校验 + 状态机 + 时间戳只前进。
    pub fn update_task_status(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        to: TaskStatus,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        board
            .update_status_for_session(session_id, task_id, to)
            .map_err(Into::into)
    }

    /// 会话作用域文本更新：域层未冻结 `update_text_for_session` 门，此处按
    /// `update_status_for_session` 的同构模式组合——同一锁内先过 `get_for_session`
    /// owner 门，再复用 `update_text`；越权/不存在同报 NotFound 且零副作用。
    pub fn update_task_text(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        subject: Option<&str>,
        description: Option<&str>,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        board.get_for_session(session_id, task_id)?;
        board
            .update_text(task_id, subject, description)
            .map_err(Into::into)
    }
}
