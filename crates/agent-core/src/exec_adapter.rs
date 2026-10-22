//! `shell.exec` 的 wire 适配层。
//!
//! 参数翻译（`command` 分词、`cwd` 校验）与「判定 → 沙箱执行」已经由既有
//! [`crate::executor::ExecToolExecutor`] 完成：它只做形状校验，随后一律调用
//! [`agent_tools_exec::GuardedExec::run`]，因此不存在绕过沙箱的执行路径。
//!
//! 本模块**不重写**那份实现，只提供本域对外的稳定名字与装配入口，供汇总卡/宿主持有
//! `Arc<GuardedExec>` 时一行接入；工具注册（写 `ToolRegistry`）仍由汇总卡负责。

use std::sync::Arc;

use agent_tools_exec::GuardedExec;

use crate::r#loop::ToolExecutor;

/// `shell.exec` 适配器：即既有执行体，参数翻译与沙箱链都复用它。
pub use crate::executor::ExecToolExecutor as ShellExecAdapter;

/// 以判定 → 沙箱执行器构造本域适配器。
pub fn shell_exec_adapter(guarded: Arc<GuardedExec>) -> ShellExecAdapter {
    ShellExecAdapter::new(guarded)
}

/// 以工具槽位需要的 trait object 形式返回（宿主装配 `set_tool_executor` 用）。
pub fn shell_exec_executor(guarded: Arc<GuardedExec>) -> Arc<dyn ToolExecutor + Send + Sync> {
    Arc::new(shell_exec_adapter(guarded))
}
