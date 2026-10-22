//! `fs.*` 与 `web.fetch` 的执行接线：把两族工具装进运行时的**工具执行器槽**。
//!
//! # 为什么需要本模块
//!
//! `fs.*`（六个）与 `web.fetch` 早已注册进 [`crate::tool::default_registry`]，
//! 也有各自的执行体（[`crate::fs_adapter::FsToolExecutor`]、
//! [`crate::web_adapter::WebFetchAdapter`]），但**没有任何执行器认领它们**——
//! 运行时派发时落到 [`crate::executor::DispatchToolExecutor::UNMATCHED`]
//! （`无可用执行器认领工具`）。本模块负责把「谁认领哪个名字」这件事补上。
//!
//! # 绑定契约
//!
//! 绑定键有两种形态（与 [`crate::executor::DispatchToolExecutor`] 的路由规则一致）：
//! - **前缀键**（以 `.` 结尾）：认领该前缀起首的**全部**工具名 ⇒ `fs.`；
//! - **精确名键**：只认领这一个名字 ⇒ `web.fetch`。
//!
//! 路由顺序：精确名优先于前缀；同形态内**先绑者胜**。四族之间名字空间不重叠
//! （`task.` / `shell.exec` / `fs.` / `web.fetch`），因此顺序只影响可复现性。
//!
//! 绑定顺序表的单一来源是 [`TOOL_BINDING_TABLE`]；装配**按表迭代**（顺序与集合
//! 都来自表），`tests/tool_exec_wiring.rs` 断言实际装配的绑定名序列与表逐字一致。
//!
//! # 权限门不得被绕过
//!
//! `fs.write` / `fs.delete` 与 `shell.exec` 走**同一条纪律**：判定 → 执行。
//! - **判定**：[`Runtime::check_tool`]——与运行时工具步用的是**同一道门**、
//!   **同一份活态会话**（模式/权限模式随时可变，不得取装配时的快照）。
//! - **执行**：[`crate::fs_adapter::FsToolExecutor`] 内的 `agent-tools` 路径守卫
//!   （工作区包含、符号链接、受保护分量）与 `expect_read` 防呆覆盖——与
//!   `shell.exec` 的沙箱层同位。
//!
//! 只挡 [`Decision::Deny`]：[`Decision::Ask`]（需人工审批）**放行**，因为审批流
//! 归运行时管——运行时在调本执行器之前就把 `Ask` 拦成挂起审批，批准后才回调
//! 执行器（`resume_approved_tool_call` → `invoke_pending_tool`，该路径**直接**
//! 调 `executor.execute`、不重过运行时门）。执行器层再拒一次只会把**已批准**的
//! 调用重复拒掉。
//!
//! # 已知代价：引用环
//!
//! 判定需要**活态**会话 ⇒ 执行器持有 [`Arc<Runtime>`] ⇒ `Runtime` → 执行器槽 →
//! 分派器 → 执行器 → `Runtime` 成环。环的代价是**一个 Runtime 图**（会话表/
//! 事件日志/任务看板）不随 `Runtime` 释放；[`Runtime::clear_tool_executor`] 清空
//! 执行器槽即断环。该形状与仓库既有的 [`crate::task_adapter::TaskAdapter`]
//! （同样持有 `Arc<Runtime>`）一致。
//!
//! # 装配面（两个入口 + 两个构造器）
//!
//! [`Runtime::set_tool_executor`] 是**单槽位热替换**（后写覆盖先写），因此：
//! - [`Runtime::install_domain_tools`]：只装 `fs.` + `web.fetch`（**替换**槽位）；
//! - [`Runtime::install_all_tools`]：一次装齐 `task.` + `shell.exec` + `fs.` +
//!   `web.fetch`（**替换**槽位）——四族共存必须走这个入口。
//!
//! 两个 `build_*_dispatcher` 是对应的**纯构造器**（不碰槽位）——装配自检与宿主
//! 自建分派器用。
//!
//! `fs.` 需要工作区根：取**会话**的 `workspace_root`（[`Runtime::get_session`]）。
//! 该字段为空 ⇒ 不装 `fs.` 族 ⇒ 调用时落 `UNMATCHED`（不静默给一个没有安全闸的
//! `fs.`）。
//!
//! ⛔ 工作区根的**真实来源**是 `CreateSession.workspace_root`（`Option<String>`），
//! 会话建立时落进 [`crate::schema::Session`]；`None` ⇒ 回落到运行时的
//! `workdir`（`runtime/sessions.rs` 的 `unwrap_or_else`）⇒ 回落后的工作区**是**
//! 一个已配置的安全边界，故 `fs.` 族照常装。
//! （卡片的 D5 写的是 `self.config_get().workspace_root`——`RuntimeConfig` 里
//! **没有**这个字段；`runtime.rs` 里那个 `workspace_root: Option<String>` 属于
//! `CreateSession`，不是 `RuntimeConfig`。此处按活态会话的
//! `Session.workspace_root` 取，语义等价且是运行时唯一可读的那一份。）
//!
//! `web.fetch` 需要抓取端口：本 crate **不依赖 tokio**，抓取是 `async` 的 ⇒ 宿主
//! 注入同步端口 [`WebFetchPort`]；`None` ⇒ 不装 `web.fetch`（不静默降级）。

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use agent_tools_exec::GuardedExec;

use crate::error::CoreError;
use crate::executor::{DispatchToolExecutor, ExecToolExecutor, EXEC_TOOL_NAME, TASK_TOOL_PREFIX};
use crate::fs_adapter::FsToolExecutor;
use crate::permission::Decision;
use crate::r#loop::ToolExecutor;
use crate::runtime::Runtime;
use crate::tool_web::WEB_FETCH_TOOL;
use crate::web_adapter::{WebFetchAdapter, WebFetchPort};

/// `fs.*` 工具族的**前缀绑定键**（以 `.` 结尾 ⇒ 只认领 `fs.` 起首的工具名）。
pub const FS_TOOL_PREFIX: &str = "fs.";

/// `web.fetch` 的**精确名**绑定键。
pub const WEB_FETCH_BINDING: &str = WEB_FETCH_TOOL;

/// 四族组合装配的绑定顺序表（装配契约的单一来源）。
///
/// 顺序即装配顺序（先绑者胜）；键为**精确名**或**前缀**。装配**按本表迭代**
/// ⇒ 顺序与集合都只有这一份事实来源。
///
/// ⛔ 本表描述的是**四族**装配（[`Runtime::install_all_tools`]）；旧的
/// [`crate::executor::builtin_tool_bindings`] 描述的是
/// [`crate::executor::Runtime::install_builtin_tools`] 的**两族**装配
/// （`task.` + `shell.exec`，行为不变），它从本表派生 ⇒ 描述文本只有一份。
pub const TOOL_BINDING_TABLE: [(&str, &str); 4] = [
    (TASK_TOOL_PREFIX, "task.* 工具族（会话作用域）"),
    (EXEC_TOOL_NAME, "exec 沙箱链"),
    (FS_TOOL_PREFIX, "fs.* 工具族（工作区锚定）"),
    (WEB_FETCH_BINDING, "web.fetch 抓取适配层"),
];

/// 执行器槽里的执行器类型（与 [`Runtime::set_tool_executor`] 的入参同型）。
type BoundExecutor = Arc<dyn ToolExecutor + Send + Sync>;

/// 一条绑定：`(绑定键, 执行器)`；键为精确名或前缀。
type Binding = (&'static str, BoundExecutor);

/// 执行前的权限门：与运行时工具步同一道门、同一份活态会话。
///
/// 只挡 [`Decision::Deny`]；[`Decision::Ask`] 放行（审批流归运行时管，
/// 理由见模块文档）。
struct GatedExecutor {
    runtime: Arc<Runtime>,
    session: Uuid,
    inner: BoundExecutor,
}

impl GatedExecutor {
    fn new(runtime: &Runtime, session: Uuid, inner: BoundExecutor) -> Self {
        Self {
            runtime: Arc::new(runtime.clone()),
            session,
            inner,
        }
    }
}

impl ToolExecutor for GatedExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        match self.runtime.check_tool(self.session, name, args) {
            Ok(Decision::Deny(reason)) => Err(format!("{name} 被权限门拒绝: {reason}")),
            Ok(_) => self.inner.execute(name, args),
            Err(error) => Err(format!("{name} 权限判定失败: {error}")),
        }
    }
}

impl Runtime {
    /// 组装**四族**分派器（`task.` + `shell.exec` + `fs.` + `web.fetch`）。
    ///
    /// 纯构造器：不碰执行器槽。绑定顺序 = [`TOOL_BINDING_TABLE`] 的顺序。
    /// `fs.` / `web.fetch` 的缺席条件见 [`Runtime::install_all_tools`]。
    pub fn build_dispatcher(
        &self,
        session: Uuid,
        guarded: Arc<GuardedExec>,
        web: Option<Arc<dyn WebFetchPort>>,
    ) -> Result<DispatchToolExecutor, CoreError> {
        let plan = binding_plan(self, session, Some(&guarded), web.as_ref())?;
        Ok(assemble_dispatcher(plan))
    }

    /// 组装**域族**分派器（`fs.` + `web.fetch`）——不含 `task.` 与 `shell.exec`。
    ///
    /// 纯构造器：不碰执行器槽。
    pub fn build_domain_dispatcher(
        &self,
        session: Uuid,
        web: Option<Arc<dyn WebFetchPort>>,
    ) -> Result<DispatchToolExecutor, CoreError> {
        let plan = binding_plan(self, session, None, web.as_ref())?;
        let domain = plan
            .into_iter()
            .filter(|(key, _)| *key == FS_TOOL_PREFIX || *key == WEB_FETCH_BINDING)
            .collect();
        Ok(assemble_dispatcher(domain))
    }

    /// 只装 `fs.` + `web.fetch` 两族（**替换**执行器槽）。
    ///
    /// ⛔ 单槽位热替换：本方法会**顶掉**槽里已有的执行器（含 `task.` 与
    /// `shell.exec`）。要四族共存用 [`Runtime::install_all_tools`]。
    ///
    /// - `fs.`：会话 `workspace_root` 为空 ⇒ **不装**（调用时落
    ///   `UNMATCHED`，不静默给一个没有路径安全闸的 `fs.`）；
    /// - `web.fetch`：`web` 为 `None` ⇒ **不装**（本 crate 不碰 tokio，抓取
    ///   必须由宿主注入同步端口）。
    pub fn install_domain_tools(
        &self,
        session: Uuid,
        web: Option<Arc<dyn WebFetchPort>>,
    ) -> Result<(), CoreError> {
        let dispatcher = self.build_domain_dispatcher(session, web)?;
        self.set_tool_executor(Arc::new(dispatcher));
        Ok(())
    }

    /// 一次装齐四族：`task.` + `shell.exec` + `fs.` + `web.fetch`（**替换**执行器槽）。
    ///
    /// 这是**四族共存**的入口：`Runtime::set_tool_executor` 是单槽位热替换，
    /// 分多次装会互相顶掉。
    ///
    /// 绑定顺序 = [`TOOL_BINDING_TABLE`] 的顺序（精确名优先于前缀，同形态先绑者胜）。
    /// `fs.` / `web.fetch` 的缺席条件与 [`Runtime::install_domain_tools`] 相同。
    pub fn install_all_tools(
        &self,
        session: Uuid,
        guarded: Arc<GuardedExec>,
        web: Option<Arc<dyn WebFetchPort>>,
    ) -> Result<(), CoreError> {
        let dispatcher = self.build_dispatcher(session, guarded, web)?;
        self.set_tool_executor(Arc::new(dispatcher));
        Ok(())
    }

    /// `fs.` 族的执行器：会话工作区为空 ⇒ 无（不装该族）。
    fn fs_executor(&self, session: Uuid) -> Result<Option<BoundExecutor>, CoreError> {
        let hot = self.get_session(session)?;
        if hot.workspace_root.trim().is_empty() {
            return Ok(None);
        }
        let root = PathBuf::from(hot.workspace_root);
        Ok(Some(Arc::new(GatedExecutor::new(
            self,
            session,
            Arc::new(FsToolExecutor::new(root)),
        ))))
    }
}

/// 装配计划：**按 [`TOOL_BINDING_TABLE`] 迭代**产出 `(绑定键, 执行器)`。
///
/// 缺席的族不产出条目（`guarded` 为 `None` ⇒ 无 `shell.exec`；`web` 为 `None` ⇒
/// 无 `web.fetch`；会话工作区为空 ⇒ 无 `fs.`）。
fn binding_plan(
    runtime: &Runtime,
    session: Uuid,
    guarded: Option<&Arc<GuardedExec>>,
    web: Option<&Arc<dyn WebFetchPort>>,
) -> Result<Vec<Binding>, CoreError> {
    let mut plan: Vec<Binding> = Vec::new();
    for (key, _) in TOOL_BINDING_TABLE {
        let executor: Option<BoundExecutor> = match key {
            TASK_TOOL_PREFIX => Some(Arc::new(runtime.task_tools_for_session(session))),
            EXEC_TOOL_NAME => guarded
                .map(|exec| Arc::new(ExecToolExecutor::new(Arc::clone(exec))) as BoundExecutor),
            FS_TOOL_PREFIX => runtime.fs_executor(session)?,
            WEB_FETCH_BINDING => web.map(|port| {
                Arc::new(GatedExecutor::new(
                    runtime,
                    session,
                    Arc::new(WebFetchAdapter::new(Arc::clone(port))),
                )) as BoundExecutor
            }),
            _ => None,
        };
        if let Some(executor) = executor {
            plan.push((key, executor));
        }
    }
    Ok(plan)
}

/// 绑定条目 → 分派器（顺序即优先级，先绑者胜）。
fn assemble_dispatcher(bindings: Vec<Binding>) -> DispatchToolExecutor {
    let mut dispatcher = DispatchToolExecutor::new();
    for (key, executor) in bindings {
        dispatcher = dispatcher.bind(key, executor);
    }
    dispatcher
}
