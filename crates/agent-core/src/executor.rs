//! `exec` 工具的真实执行体：把 `ToolExecutor::execute` 接到已闭环的 exec 链。
//!
//! 本模块只做**形状校验与结果转译**，判定与沙箱一律交给
//! [`agent_tools_exec::GuardedExec::run`]：本层不解析命令、不挑程序、不起进程，
//! 因此不存在绕过策略的路径。
//!
//! 调用形状固定为：
//!
//! ```json
//! { "argv": ["echo", "hi"], "cwd": "/tmp" }
//! ```
//!
//! - `argv` 必填、非空字符串数组；`cwd` 可选、必须是字符串。
//! - 形状不合法 → `Err`（静态说明，不回显参数值，避免把用户输入带进日志）。
//! - 策略拒绝 / 待审批 → `Err`，说明文本原样透传，供宿主转成审批交互。
//! - 执行成功（含超时、截断、非零退出码）→ `Ok`，结果里的 `exit_code` 可为 `null`
//!   （被信号终止时没有退出码），`timed_out` / `truncated` 由沙箱层填充。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::r#loop::ToolExecutor;

/// 本执行体认领的工具名；其余工具名一律拒绝。
pub const EXEC_TOOL_NAME: &str = "exec";

/// 静态拒绝说明：形状不合法时返回，不回显任何参数值。
const BAD_ARGS: &str = "exec 参数不合法：需要 { \"argv\": [非空字符串数组], \"cwd\": 可选字符串 }";

/// 把 `exec` 工具调用接到 policy → sandbox 执行链。
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

        let argv = object
            .get("argv")
            .and_then(Value::as_array)
            .ok_or(BAD_ARGS)?
            .iter()
            .map(|token| token.as_str().map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .filter(|argv| !argv.is_empty())
            .ok_or(BAD_ARGS)?;

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
