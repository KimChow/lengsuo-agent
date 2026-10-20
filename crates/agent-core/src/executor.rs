//! `shell.exec` 工具的真实执行体：把 `ToolExecutor::execute` 接到已闭环的 exec 链。
//!
//! 本模块只做**形状校验与结果转译**，判定与沙箱一律交给
//! [`agent_tools_exec::GuardedExec::run`]：本层不解析命令、不挑程序、不起进程，
//! 因此不存在绕过策略的路径。对齐参考实现「判定（`execpolicy::Decision`）与执行
//! （`sandboxing` 分发层）分离、`Allow` 之后必经沙箱分发层」的分层
//! （`sandboxing/src/manager.rs` 的 `SandboxManager::transform` 位于判定之后、
//! spawn 之前）。
//!
//! 调用形状固定为：
//!
//! ```json
//! { "command": "echo hi", "cwd": "/tmp" }
//! ```
//!
//! - `command` 必填、非空字符串，按 shell 语义分词为 argv（`shell-words`）；
//!   `cwd` 可选、必须是字符串。
//! - 形状不合法 / 分词失败 → `Err`（静态说明，不回显参数值，避免把用户输入带进日志）。
//! - 策略拒绝 / 待审批 → `Err`，说明文本原样透传，供宿主转成审批交互。
//! - 执行成功（含超时、截断、非零退出码）→ `Ok`，结果里的 `exit_code` 可为 `null`
//!   （被信号终止时没有退出码），`timed_out` / `truncated` 由沙箱层填充。
//!
//! 工具名收敛：`shell.exec` 是白名单（[`crate::mode::WRITE_TOOLS`]）里的**唯一事实来源**，
//! 执行体服从白名单，不再另立 `exec` 别名（对齐参考端「execpolicy 规则键 = 逻辑命令名」）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::r#loop::ToolExecutor;

/// 本执行体认领的工具名；与 [`crate::mode::WRITE_TOOLS`] 中 `shell.exec` 逐字一致。
pub const EXEC_TOOL_NAME: &str = "shell.exec";

/// 静态拒绝说明：形状不合法时返回，不回显任何参数值。
const BAD_ARGS: &str =
    "shell.exec 参数不合法：需要 { \"command\": 非空字符串, \"cwd\": 可选字符串 }";

/// 把 `shell.exec` 工具调用接到 policy → sandbox 执行链。
pub struct ExecToolExecutor {
    guarded: Arc<agent_tools_exec::GuardedExec>,
    /// 工作目录；`None` 表示由沙箱层决定（通常是子进程继承的当前目录）。
    cwd: Option<PathBuf>,
}

impl ExecToolExecutor {
    /// 以给定的判定 → 执行器构造；工作目录留给沙箱层决定。
    pub fn new(guarded: Arc<agent_tools_exec::GuardedExec>) -> Self {
        Self { guarded, cwd: None }
    }

    /// 指定工作目录。`cwd` 优先级：调用参数 > 本设置 > 沙箱层默认。
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// 解析后的调用参数。
    fn parse_args(args: &Value) -> Result<(Vec<String>, Option<PathBuf>), String> {
        let object = args.as_object().ok_or(BAD_ARGS)?;

        // `command` 必填、必须是字符串，且分词后不得为空。
        let command = object
            .get("command")
            .and_then(Value::as_str)
            .ok_or(BAD_ARGS)?;
        let argv = split_shell_command(command).ok_or(BAD_ARGS)?;
        if argv.is_empty() {
            return Err(BAD_ARGS.to_string());
        }

        // `cwd` 缺省合法；显式给出且不是字符串则拒绝（不静默忽略）。
        let cwd = match object.get("cwd") {
            None | Some(Value::Null) => None,
            Some(value) => Some(PathBuf::from(value.as_str().ok_or(BAD_ARGS)?)),
        };

        Ok((argv, cwd))
    }

    /// 执行结果的 JSON 投影。
    fn outcome_json(outcome: agent_sandbox::ExecOutcome) -> Value {
        json!({
            "exit_code": outcome.status.code,
            // 非法 UTF-8 用 lossy 转换：工具结果必须是可序列化文本，且不得 panic。
            "stdout": String::from_utf8_lossy(&outcome.stdout),
            "stderr": String::from_utf8_lossy(&outcome.stderr),
            "timed_out": outcome.timed_out,
            "truncated": outcome.truncated,
            "sandbox": outcome.sandbox.as_metric_tag(),
        })
    }
}

impl ToolExecutor for ExecToolExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        if name != EXEC_TOOL_NAME {
            return Err(format!("不支持的工具: {name}"));
        }

        let (argv, cwd) = Self::parse_args(args)?;
        // 参数级 `cwd` 覆盖构造时配置；两者都没有则交给沙箱层。
        let cwd: Option<&Path> = cwd.as_deref().or(self.cwd.as_deref());

        self.guarded
            .run(&argv, cwd)
            .map(Self::outcome_json)
            .map_err(render_error)
    }
}

/// 链接错误 → 工具错误说明。
///
/// [`agent_tools_exec::ExecError`] 的 `Display` 已把「策略拒绝」与「需人工审批」
/// 渲染成稳定前缀，这里原样透传，宿主可据此前缀路由到审批交互。
fn render_error(error: agent_tools_exec::ExecError) -> String {
    error.to_string()
}

/// 把 `shell.exec` 的 `command` 字符串按 shell 语义分词为 argv。
///
/// 只在 `agent-core` 内实现（**不新增依赖**：`Cargo.toml` 属共享区分册，本卡不得改）。
/// 语义与 `agent-pty` 的 `shell_words::split` 一致，以便「判定对象 == 被执行 argv」：
/// - 单引号内一律字面量（不可转义，含 `\`）；
/// - 双引号内 `\` 只对 `"` `\` `$` `` ` `` 换行有转义意义（POSIX 语义）；
/// - 引号外的 `\` 转义下一个字符；空白（空格 / 制表）分隔 token；
/// - 引号未闭合 / 尾随裸 `\` ⇒ `None`（调用方 fail-closed 拒绝，不猜意图）。
///
/// 明确**只做分词与去引号**：不做变量展开、不做管道/重定向解析、不执行任何东西——
/// 那些语义若需要由沙箱层与判定层各自负责，本层不得越过。
fn split_shell_command(command: &str) -> Option<Vec<String>> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    // 是否有「未结算的 token」——区分 `""` 产出的空 token 与纯分隔空白。
    let mut in_token = false;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(literal) => current.push(literal),
                        None => return None, // 引号未闭合
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            // 双引号内只有这几个字符可被转义；其余保反斜杠字面量。
                            Some(escaped @ ('"' | '\\' | '$' | '`')) => current.push(escaped),
                            Some('\n') => {}
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return None, // 尾随裸 `\`
                        },
                        Some(literal) => current.push(literal),
                        None => return None, // 引号未闭合
                    }
                }
            }
            '\\' => {
                in_token = true;
                // 尾随裸 `\` ⇒ `None`（fail-closed，不猜意图）。
                current.push(chars.next()?);
            }
            ch if ch.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            literal => {
                in_token = true;
                current.push(literal);
            }
        }
    }

    if in_token {
        tokens.push(current);
    }
    // 纯空白命令行 ⇒ 空 argv（调用方转成静态错误）。
    Some(tokens)
}

// ---------------------------------------------------------------------------
// 组合执行器：按工具名分派（同槽互斥的缓解）
// ---------------------------------------------------------------------------

/// 按**工具名**把调用分派给子执行器。
///
/// 来由：`Runtime::set_tool_executor` 是**整槽替换**（`runtime.rs:157`），
/// `task.*` 与 `shell.exec` 无法共存于同一步。参考实现里 execpolicy 判定与沙箱执行
/// 被显式分段（`core/src/exec_policy.rs` 产出 `ExecApprovalRequirement` 后，命令仍统一
/// 经 `sandboxing` 分发层），本仓对应的槽位是**单一入口**，因此把「多来源执行器」
/// 收拢成一个组合体，槽位仍只装一个 `ToolExecutor`。
///
/// 绑定规则（对齐 `agent-pty` 的策略门纪律：判定对象就是被执行的 argv）：
/// - **精确名**优先于前缀；前缀以 `.` 结尾（如 `task.`），只匹配同族其余工具；
/// - **首个命中的绑定生效**（装配顺序即优先级），不合并、不静默丢来源；
/// - 无匹配 ⇒ [`DispatchToolExecutor::UNMATCHED`] 可读错误（含工具名，**不 panic**）；
/// - 子执行器自身返回的错误**原样透传**（委派对象唯一：执行体仍然是受沙箱保护的那一个）。
///
/// 本类型**不**内含沙箱或判定：它只做名字路由，真正执行仍由被绑定的
/// [`ExecToolExecutor`]（内部即 `GuardedExec`）承担，故不引入绕过沙箱的路径。
pub struct DispatchToolExecutor {
    /// 绑定表。`Vec` 保序（装配顺序即优先级，可复现），键为**精确名或前缀**。
    bindings: Vec<(String, Arc<dyn ToolExecutor + Send + Sync>)>,
}

impl DispatchToolExecutor {
    /// 空分派器：任何工具名都落到「无匹配」错误。
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// 追加一条绑定；后写不覆盖先写，命中优先级 = 装配顺序。
    ///
    /// `name` 为精确工具名（`shell.exec`）或以 `.` 结尾的前缀（`task.`）。
    #[must_use]
    pub fn bind(
        mut self,
        name: impl Into<String>,
        executor: Arc<dyn ToolExecutor + Send + Sync>,
    ) -> Self {
        self.bindings.push((name.into(), executor));
        self
    }

    /// 已绑定条目数（装配自检用）。
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// 是否没有任何绑定（`clippy::len_without_is_empty` 的对偶）。
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// 已绑定的名字序列（装配自检与文档的单一来源）。
    pub fn names(&self) -> Vec<&str> {
        self.bindings.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// 无匹配工具名时的错误前缀（供测试与宿主识别）。
    pub const UNMATCHED: &'static str = "无可用执行器认领工具";

    /// 找首个认领 `name` 的子执行器（精确名优先于前缀）。
    fn route(&self, name: &str) -> Option<&Arc<dyn ToolExecutor + Send + Sync>> {
        // 精确名优先：两个循环而非一次遍历，顺序语义显式。
        for (bound, executor) in &self.bindings {
            if bound == name {
                return Some(executor);
            }
        }
        for (bound, executor) in &self.bindings {
            if bound.ends_with('.') && name.starts_with(bound.as_str()) {
                return Some(executor);
            }
        }
        None
    }
}

impl Default for DispatchToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolExecutor for DispatchToolExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        match self.route(name) {
            Some(executor) => executor.execute(name, args),
            None => Err(format!(
                "{}: {name}（本分派器认领 {:?}）",
                Self::UNMATCHED,
                self.names()
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime 侧的生产装配入口
// ---------------------------------------------------------------------------
//
// 本 `impl Runtime` 块刻意放在 `executor.rs` 内（Rust 允许同 crate 跨文件 impl）：
// `runtime.rs` 是 `h1-host-services` 卡的独占文件，并行改会冲突。契约不变——
// 仍旧只写 `Runtime::set_tool_executor` 那**唯一一个槽位**，不新增第二条注入路径。

impl super::Runtime {
    /// 只装 exec 工具：`shell.exec` → policy → sandbox 链（生产装配入口）。
    ///
    /// 与 [`super::Runtime::install_task_tools`] 共用同一槽位（`runtime.rs:157` 的
    /// `set_tool_executor`），因此**同一步只能装一种执行器**。要两者共存用
    /// [`super::Runtime::install_builtin_tools`]（组合分派），不要连续调用本方法。
    pub fn install_exec_tools(&self, guarded: Arc<agent_tools_exec::GuardedExec>) {
        self.set_tool_executor(Arc::new(ExecToolExecutor::new(guarded)));
    }

    /// 只装 exec 工具的**分派**形态：`shell.exec` 走 [`ExecToolExecutor`]，
    /// 其余名字（含 `task.*`）落入无匹配错误。
    ///
    /// 用于「本步只允许 exec」的宿主；全量装配用
    /// [`super::Runtime::install_builtin_tools`]。
    pub fn install_exec_dispatch(&self, guarded: Arc<agent_tools_exec::GuardedExec>) {
        let dispatcher = DispatchToolExecutor::new()
            .bind(EXEC_TOOL_NAME, Arc::new(ExecToolExecutor::new(guarded)));
        self.set_tool_executor(Arc::new(dispatcher));
    }

    /// 一次装好「任务执行器 + exec 执行器」的组合体（推荐入口）。
    ///
    /// 组合体是 [`DispatchToolExecutor`]：`task.*` 路由到**本会话**的任务执行器，
    /// `shell.exec` 路由到 **judge → sandbox** 的 [`ExecToolExecutor`]。两者各自保持
    /// 原有语义（任务执行器绑会话；exec 执行器不解析命令、不起进程，一律交给
    /// [`agent_tools_exec::GuardedExec`]），分派层只做名字路由。
    ///
    /// 未绑定任何 `ToolSpec`：本卡不注册工具（属 M1 范围），只保证执行端**可被装上**。
    pub fn install_builtin_tools(
        &self,
        session: Uuid,
        guarded: Arc<agent_tools_exec::GuardedExec>,
    ) {
        // 绑定顺序即优先级：先任务族（前缀），再 exec（精确名）；两者名字空间不重叠。
        let task_executor: Arc<dyn ToolExecutor + Send + Sync> =
            Arc::new(self.task_tools_for_session(session));
        let exec_executor: Arc<dyn ToolExecutor + Send + Sync> =
            Arc::new(ExecToolExecutor::new(guarded));
        let dispatcher = DispatchToolExecutor::new()
            .bind(TASK_TOOL_PREFIX, task_executor)
            .bind(EXEC_TOOL_NAME, exec_executor);
        self.set_tool_executor(Arc::new(dispatcher));
    }
}

/// `task.*` 工具族的**前缀绑定键**（以 `.` 结尾 ⇒ 只匹配 `task.` 起首的工具名）。
///
/// 与 `runtime::task_tools::TASK_TOOLS` 的关系：那四个名字是**具体**工具名，
/// 这里是装配期**前缀**。用前缀而非逐个精确名，是为了不把工具清单复制成第二份
/// 事实来源（新增 `task.*` 工具时装配无须同步改）。
pub const TASK_TOOL_PREFIX: &str = "task.";

/// 组合装配的绑定顺序表（装配契约的单一来源，供文档与测试引用）。
///
/// 顺序即优先级：精确名 `shell.exec` 与 `task.` 前缀互不重叠，顺序只影响未来扩展。
pub fn builtin_tool_bindings() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("task.", "task.* 工具族（会话作用域）"),
        (EXEC_TOOL_NAME, "exec 沙箱链"),
    ])
}
