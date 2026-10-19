//! Runtime：会话/任务热态与回合执行（开工说明 §3/§4/§5）。
//!
//! - 热态是进程内并发表（§5：UI 读取走内存，不做同步磁盘 IO）；
//! - 事件一律经 `EventLog` 落 JSONL 并广播到 `EventHub`（发送端不阻塞）；
//! - 大结果落 `BlobStore`，消息 `content` 只留收窄摘要；
//! - 工具输出与审批摘要进事件流前必须过脱敏器（§4）。
//!
//! 本模块是同步域逻辑（不依赖 tokio）；网关层在自己的运行时里调度。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use uuid::Uuid;

use agent_tasks::TaskBoard;

use crate::blobs::BlobStore;
use crate::bus::EventHub;
use crate::config::RuntimeConfig;
use crate::error::CoreError;
use crate::event_log::{EventLog, JsonlEventLog};
use crate::permission::PermissionGate;
use crate::provider::{Provider, TurnContext};
use crate::r#loop::ToolExecutor;
use crate::redact::Redactor;
use crate::schema::{Event, Isolation, Message, Mode, PermissionMode, Session, SessionStatus};
use crate::tool::ToolRegistry;

/// `content` 里保留的最大字符数（大结果走 `result_ref`）。
pub const CONTENT_PREVIEW_CHARS: usize = 400;

/// 新建会话入参。
#[derive(Debug, Clone)]
pub struct CreateSession {
    pub title: String,
    pub workspace_root: Option<String>,
    pub mode: Mode,
    pub permission_mode: PermissionMode,
    pub env_binding: Option<String>,
    pub isolation: Isolation,
}

impl CreateSession {
    /// 最简构造（测试与 `session:create` 默认值用）。
    pub fn plain(title: impl Into<String>, isolation: Isolation) -> Self {
        Self {
            title: title.into(),
            workspace_root: None,
            mode: Mode::Ask,
            permission_mode: PermissionMode::ReadOnly,
            env_binding: isolation.env_id.clone(),
            isolation,
        }
    }
}

/// 一个回合的结果（网关 `session:send` 的 data）。
#[derive(Debug, Clone)]
pub struct TurnReceipt {
    pub session_id: Uuid,
    /// 本回合产生的事件（已落盘并广播）。
    pub events: Vec<Event>,
    pub last_seq: u64,
}

/// 会话热态记录。
struct HotSession {
    session: Session,
    /// 单队列投递锁：同一会话的回合串行执行，后到的输入不插入进行中的步。
    drain: Arc<Mutex<()>>,
}

/// 运行时句柄（克隆共享）。
#[derive(Clone)]
pub struct Runtime {
    workdir: PathBuf,
    sessions: Arc<Mutex<BTreeMap<Uuid, HotSession>>>,
    log: Arc<dyn EventLog>,
    blobs: Arc<BlobStore>,
    hub: Arc<EventHub>,
    gate: Arc<PermissionGate>,
    provider: Arc<dyn Provider>,
    redactor: Redactor,
    config: Arc<Mutex<RuntimeConfig>>,
    /// 会话内任务看板（task 工具族域层能力面；克隆共享，走 `*_for_session` 门）。
    tasks: Arc<Mutex<TaskBoard>>,
    /// 工具执行器（W1 接线）：注入后工具步真实执行并结算结果；`None` 保持
    /// pending 占位（执行由宿主承担）。`Send + Sync` 为使用点约束（trait 本体
    /// 属 `loop` 模块，本卡不改），保证 `Runtime` 仍可跨线程共享（网关 tokio 域）。
    ///
    /// 句柄热替换面（`set_tool_executor`）：执行器要绑定会话 id，而会话是在
    /// `build()` 之后才创建的，构造期注入无法覆盖「按会话装配」；若改成绑定一次的
    /// 独立 `Arc`，宿主还得额外持有一份会话表快照。这里改用**同一把锁下的热替换**，
    /// 不新增第二条注入路径（装配仍只有 `RuntimeBuilder::tool_executor` 与
    /// [`Runtime::set_tool_executor`] 两个入口，后者是前者的运行期对应物）。
    executor: Arc<Mutex<Option<Arc<dyn ToolExecutor + Send + Sync>>>>,
}

impl Runtime {
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    /// 工作目录（`<workdir>/events`、`<workdir>/blobs`）。
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// 权限门（网关 `tool:list` / 审批 RPC 用）。
    pub fn gate(&self) -> &PermissionGate {
        &self.gate
    }

    /// 事件日志（网关回放 RPC 用）。
    pub fn event_log(&self) -> &dyn EventLog {
        self.log.as_ref()
    }

    /// 当前配置快照。
    pub fn config_get(&self) -> RuntimeConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// 应用配置补丁；失败不改现值。
    pub fn config_set(&self, patch: &Value) -> Result<RuntimeConfig, CoreError> {
        let mut guard = self
            .config
            .lock()
            .map_err(|_| CoreError::InvalidConfig("配置锁毒化".into()))?;
        let next = guard.apply_patch(patch)?;
        *guard = next.clone();
        Ok(next)
    }

    /// 指定会话的 `task.*` 工具执行器句柄（W1 打通卡）。
    ///
    /// 无状态：会话 id 绑进执行器，Runtime 由执行语境提供（见
    /// [`Runtime::with_task_runtime`]）。宿主自建装配时用它，或直接用
    /// [`Runtime::install_task_tools`] 一行装上。
    pub fn task_tools_for_session(&self, session: Uuid) -> TaskToolExecutor {
        TaskToolExecutor::for_session(session)
    }

    /// 运行期装配任务工具：把**本会话**的 `task.*` 执行器接进去（一行）。
    ///
    /// 与 [`RuntimeBuilder::tool_executor`] 是同一注入面的运行期对应物：构造期
    /// 注入（`build()` 前拿不到会话 id）与运行期注入（会话建好后）都写同一个槽位，
    /// 没有第二套注入路径。已注入其他执行器时本次调用替换之（后写胜出）。
    pub fn install_task_tools(&self, session: Uuid) {
        self.set_tool_executor(Arc::new(self.task_tools_for_session(session)));
    }

    /// 热替换工具执行器（`None` 恢复 pending 占位）。见字段处的来由说明。
    pub fn set_tool_executor(&self, executor: Arc<dyn ToolExecutor + Send + Sync>) {
        self.set_executor_slot(Some(executor));
    }

    /// 清空执行器槽位，恢复「未注入执行器」的 pending 占位语义。
    pub fn clear_tool_executor(&self) {
        self.set_executor_slot(None);
    }

    /// 当前执行器（测试与宿主自检的观测面）。
    pub fn tool_executor(&self) -> Option<Arc<dyn ToolExecutor + Send + Sync>> {
        self.executor
            .lock()
            .expect("执行器锁不得毒化")
            .as_ref()
            .map(Arc::clone)
    }

    /// 写入执行器槽位。锁在 `Arc::clone` 量级，不跨 `step` 持有；执行器内部只锁
    /// 任务看板/会话表，不做重入（无锁序倒置）。
    fn set_executor_slot(&self, executor: Option<Arc<dyn ToolExecutor + Send + Sync>>) {
        *self.executor.lock().expect("执行器锁不得毒化") = executor;
    }
}

mod approvals;
mod bus;
mod execute;
mod internals;
mod sessions;
mod task_tools;
mod tasks;
mod tool_result;
mod turns;

pub use task_tools::TaskToolExecutor;
pub use tasks::{Task, TaskStatus};
pub use turns::StepOutcome;
// 交付物提醒的单一来源（缺口 B3）：`loop` 层经 `crate::runtime::…` 取用关键词表、
// 文案与记账位；三者皆为 `pub(crate)`，故 re-export 也必须是 `pub(crate) use`
// （`pub use` 暴露 crate 外项会报 E0364）。
pub(crate) use turns::{deliverable_reminder, WriteLedger, DELIVERABLE_REMINDER_NOTE};

pub(super) fn narrow(text: &str, chars: usize) -> String {
    let mut out: String = text.chars().take(chars).collect();
    if out.chars().count() < text.chars().count() {
        out.push_str("…（截断，完整结果见 result_ref）");
    }
    out
}

mod builder;
pub use builder::RuntimeBuilder;

mod context;

/// 字符级预览截断（≤max 字符，超限加省略标记）。
pub(super) fn preview(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let cut: String = text.chars().take(max).collect();
        format!("{cut}…")
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

impl Runtime {
    /// 一条消息的产生时刻（毫秒）。**一次调用 = 一次时钟读数**，调用方必须把这个值
    /// **同时**用于消息的 `ts_ms` 与随后事件 payload 的 `"tsMs"`——这样「重启无损」
    /// 才有语义：重建时直接取回消息时间真值，而不是拿「事件落盘时刻」冒充
    /// 「消息产生时刻」（两者是相邻的两次读数，落在相邻毫秒是常态）。
    ///
    /// 对齐 opencode 的投影语义（`session/projector.ts` + `session/message-updater.ts`：
    /// 消息行的时间一律来自事件自身携带的时刻，投影器从不读运行期内存）。
    pub(super) fn turn_ts(&self) -> u64 {
        now_ms()
    }
}
