//! 任务工具族的 Runtime 域层能力面（对照 参考客户端 TaskCreate/TaskGet/TaskUpdate/TaskList）。
//!
//! 只做接线：`TaskBoard` 挂在 Runtime 上，读写全部经 `agent-tasks` 的
//! `*_for_session` 会话作用域门（`docs/task-owned-access-status.md` 末段冻结约束），
//! 状态机、幂等与时间戳语义完全复用域层，本模块不复制任何任务逻辑。
//! 越权与不存在同报 `TaskError::NotFound`（映射为 `CoreError::Task`，文案不变），
//! 不泄露他人任务的存在性；拒绝路径零副作用。
//! 工具注册与执行器属 W3，本模块不碰 `tool.rs`。
//!
//! 事件接线（W1，对照 opencode「状态变更先成为 durable 事件、读侧由事件投影」）：
//! 写侧入口（create / update_status / update_text）在状态确实改变时落一条
//! `task.created` / `task.updated` 事件，payload 直接复用 `agent_tasks::TaskEvent`
//! 的 serde（字段名与 agent-tasks 完全一致，不另造第二套）；读侧入口不改变状态、
//! 不落事件。原子序与失败语义见 `docs/runtime-task-events-status.md`。

use std::cell::RefCell;
use std::sync::Arc;

use uuid::Uuid;

pub use agent_tasks::{Task, TaskStatus};

use agent_tasks::TaskEvent;

use crate::error::CoreError;

use super::Runtime;

/// 任务事件 kind 前缀；启动回放按它过滤事件日志（`builder.rs` 复用）。
pub(super) const TASK_EVENT_KIND_PREFIX: &str = "task.";

thread_local! {
    /// 当前线程正在服务的 Runtime（**任务工具步的语境栈**）。
    ///
    /// 来由：`ToolExecutor::execute(&self, name, args)` 的签名里既没有会话 id、也没有
    /// Runtime（trait 体属 `loop` 模块，跨 crate 公共契约，本卡不得改）。适配层于是
    /// 在 Runtime 内发起工具执行时把「正在服务的 Runtime」压栈，参数解析（纯函数）
    /// 再经 [`Runtime::*_global`] 取回它——这样参数解析与执行器**共用同一份实现**，
    /// 不出现「工具名在一条路径能跑、在另一条路径不能跑」的分叉。
    ///
    /// 约束（全部由本模块的守卫 [`Runtime::with_task_runtime`] 保证）：
    /// - 只在**一次工具执行**的期间入栈，出栈即恢复；
    /// - 只对本 Runtime 的**克隆**（同一热态）入栈，故即使压栈期间执行器又调
    ///   [`Runtime`] 方法，落到的是同一个看板/会话表；热态归属用 [`Arc`] 指针比较
    ///   断言，杜绝「张冠李戴」；
    /// - 缺语境直接返回可读错误，不 panic（纯函数形态的执行器仅供宿主按需调用）。
    static TASK_RUNTIME: RefCell<Vec<Runtime>> = const { RefCell::new(Vec::new()) };
}

impl From<agent_tasks::TaskError> for CoreError {
    /// 透传 `TaskError` 原始文案（NotFound/InvalidTransition/EmptySubject/Json），
    /// 不吞原因；网关层按 `CoreError::Task` 变体归一映射信封 code。
    fn from(e: agent_tasks::TaskError) -> Self {
        CoreError::Task(e.to_string())
    }
}

/// 任务事件的 kind：与事件体 serde 标签逐字一致（`task.created` / `task.updated`）。
fn task_event_kind(event: &TaskEvent) -> &'static str {
    match event {
        TaskEvent::Created { .. } => "task.created",
        TaskEvent::Updated { .. } => "task.updated",
    }
}

/// 当前线程的服务 Runtime：栈顶即「正在执行工具的那个 Runtime」。
fn current_task_runtime() -> Result<Runtime, CoreError> {
    TASK_RUNTIME
        .with(|stack| stack.borrow().last().cloned())
        .ok_or_else(|| {
            CoreError::InvalidConfig(
                "任务工具无执行语境（Runtime 未发起本次工具执行；宿主请改用 Runtime 的任务入口）"
                    .into(),
            )
        })
}

impl Runtime {
    // ---- 任务工具步的语境（适配层 `task_tools.rs` 的取 Runtime 通道）----

    /// 在「本 Runtime 正在服务」的语境里跑一次工具执行。
    ///
    /// 每次入栈的是**本 Runtime 的克隆**（共享同一热态），且入栈前用
    /// [`is_serving_this_runtime`](Self::is_serving_this_runtime) 的反面判归属：
    /// 若宿主把 A 的运行嵌到 B 的语境里，直接返回可读错误而不是悄悄用 B 的热态。
    /// 出栈在 `Drop` 时机，panic 展开也不会把语境留在栈上。
    pub fn with_task_runtime<T>(&self, f: impl FnOnce() -> T) -> Result<T, CoreError> {
        let mismatched = self
            .context_runtime()
            .is_some_and(|_| !self.is_serving_this_runtime());
        if mismatched {
            return Err(CoreError::InvalidConfig(
                "任务工具语境归属冲突：不同 Runtime 嵌套执行（拒绝跨热态执行）".into(),
            ));
        }
        TASK_RUNTIME.with(|stack| stack.borrow_mut().push(self.clone()));
        struct Pop;
        impl Drop for Pop {
            fn drop(&mut self) {
                TASK_RUNTIME.with(|stack| {
                    stack.borrow_mut().pop();
                });
            }
        }
        let _pop = Pop;
        Ok(f())
    }

    /// 调用线程当前是否有**任何** Runtime 在服务（装配自检用：无语境时工具步
    /// 只会落 pending 占位，宿主可据此提示「忘了 `install_task_tools`」）。
    pub fn has_task_runtime_context() -> bool {
        TASK_RUNTIME.with(|stack| !stack.borrow().is_empty())
    }

    /// 当前线程正在服务的 Runtime 是否就是 `self`（装配自检用）。
    ///
    /// 两个使用点：① [`with_task_runtime`](Self::with_task_runtime) 入栈前的
    /// 归属断言（防跨热态嵌套）；② 宿主/测试判定「本 Runtime 此刻是否在语境栈顶」。
    pub fn is_serving_this_runtime(&self) -> bool {
        self.context_runtime()
            .is_some_and(|outer| Arc::ptr_eq(&outer.sessions, &self.sessions))
    }

    /// 语境栈顶 Runtime 的克隆（无语境则 `None`）；归属比较的唯一取数点。
    fn context_runtime(&self) -> Option<Runtime> {
        TASK_RUNTIME.with(|stack| stack.borrow().last().cloned())
    }

    /// 参数解析用的**纯函数形态**任务入口：Runtime 取自线程语境（见 [`TASK_RUNTIME`]）。
    ///
    /// 与适配层 [`super::task_tools::TaskToolExecutor`] 共用同一份参数解析与错误文案，
    /// 语义与对应的 [`Runtime::list_tasks`] 等方法逐字一致（本模块是唯一实现）。
    pub(crate) fn list_tasks_global(session_id: Uuid) -> Vec<Task> {
        match current_task_runtime() {
            Ok(runtime) => runtime.list_tasks(session_id),
            // 无语境：没有 Runtime 就没有任务，返回空列表（调用方是 `task.list`，
            // 空结果比 panic 更真实——「看不到任务」）。
            Err(_) => Vec::new(),
        }
    }

    /// 见 [`Runtime::list_tasks_global`]。
    pub(crate) fn get_task_global(session_id: Uuid, task_id: Uuid) -> Result<Task, CoreError> {
        current_task_runtime()?.get_task(session_id, task_id)
    }

    /// 见 [`Runtime::list_tasks_global`]。
    pub(crate) fn create_task_global(
        session_id: Uuid,
        subject: &str,
        description: &str,
    ) -> Result<Task, CoreError> {
        current_task_runtime()?.create_task(session_id, subject, description)
    }

    /// 见 [`Runtime::list_tasks_global`]。
    pub(crate) fn update_task_status_global(
        session_id: Uuid,
        task_id: Uuid,
        to: TaskStatus,
    ) -> Result<Task, CoreError> {
        current_task_runtime()?.update_task_status(session_id, task_id, to)
    }

    /// 见 [`Runtime::list_tasks_global`]。
    pub(crate) fn update_task_text_global(
        session_id: Uuid,
        task_id: Uuid,
        subject: Option<&str>,
        description: Option<&str>,
    ) -> Result<Task, CoreError> {
        current_task_runtime()?.update_task_text(session_id, task_id, subject, description)
    }
    /// 建任务（TaskCreate 行为面）：subject 去空白后非空，初始 `pending`。
    ///
    /// 原子序：在克隆看板上变更并生成 `task.created`（快照即创建时刻的完整任务），
    /// 事件落盘成功才用克隆替换原看板；append 失败则原看板保持不变并返回错误。
    pub fn create_task(
        &self,
        session_id: Uuid,
        subject: &str,
        description: &str,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        let mut next = board.clone();
        let task = next.create(session_id, subject, description)?;
        let event = TaskEvent::Created {
            task_id: task.id,
            occurred_at_ms: task.created_at_ms,
            snapshot: task.clone(),
        };
        self.append_task_event(session_id, &event)?;
        *board = next;
        Ok(task)
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
    ///
    /// 原子序同 [`create_task`](Self::create_task)：同值幂等（状态未变）不落事件、
    /// 不替换看板；非法迁移在克隆上报错，原看板零副作用。
    pub fn update_task_status(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        to: TaskStatus,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        let current = board.get_for_session(session_id, task_id)?.clone();
        let mut next = board.clone();
        let updated = next.update_status_for_session(session_id, task_id, to)?;
        if updated == current {
            return Ok(updated);
        }
        let event = TaskEvent::Updated {
            task_id,
            occurred_at_ms: updated.updated_at_ms,
            subject: None,
            status: Some(updated.status),
        };
        self.append_task_event(session_id, &event)?;
        *board = next;
        Ok(updated)
    }

    /// 会话作用域文本更新：域层未冻结 `update_text_for_session` 门，此处按
    /// `update_status_for_session` 的同构模式组合——同一锁内先过 `get_for_session`
    /// owner 门，再复用 `update_text`；越权/不存在同报 NotFound 且零副作用。
    ///
    /// 事件只承载 `TaskEvent` 冻结形状内的字段：subject 变化落 `task.updated`
    /// （只带 subject）；description-only 变更落一条不带可选字段的裸 `task.updated`
    /// 作审计标记（重启回放不可恢复 description，已知缺口见交付文档）。
    pub fn update_task_text(
        &self,
        session_id: Uuid,
        task_id: Uuid,
        subject: Option<&str>,
        description: Option<&str>,
    ) -> Result<Task, CoreError> {
        let mut board = self.tasks.lock().expect("任务看板锁不得毒化");
        let current = board.get_for_session(session_id, task_id)?.clone();
        let mut next = board.clone();
        let updated = next.update_text(task_id, subject, description)?;
        if updated == current {
            return Ok(updated);
        }
        let subject_changed = subject.is_some_and(|value| value != current.subject);
        let event = TaskEvent::Updated {
            task_id,
            occurred_at_ms: updated.updated_at_ms,
            subject: subject_changed.then(|| updated.subject.clone()),
            status: None,
        };
        self.append_task_event(session_id, &event)?;
        *board = next;
        Ok(updated)
    }

    /// 落一条任务事件：payload 复用 `TaskEvent` serde，kind 与事件体 `type` 标签
    /// 逐字一致；事件归属该任务的 `session_id`（事件日志按会话分片）。
    ///
    /// 与 `emit` 同步热会话 `last_seq` 记账（seq 同流），但不脱敏——载荷必须与
    /// 看板逐字节等价，启动回放才不失真；也不广播到 EventHub（订阅面归 W3）。
    /// 调用方持有看板锁期间调用（锁序 tasks → sessions，全仓无反向取锁）。
    fn append_task_event(&self, session_id: Uuid, event: &TaskEvent) -> Result<(), CoreError> {
        let payload = serde_json::to_value(event)?;
        let kind = task_event_kind(event);
        let appended = self.log.append(session_id, kind, payload)?;
        if let Ok(mut guard) = self.sessions.lock() {
            if let Some(hot) = guard.get_mut(&session_id) {
                hot.session.last_seq = hot.session.last_seq.max(appended.seq);
            }
        }
        Ok(())
    }
}
