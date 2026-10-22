//! `shell.exec` 的 spec 清单（exec 域）。
//!
//! 本模块只产出**注册规格**：工具名、描述、类别、敏感标记与超时。真正的注册动作
//! （写入 [`crate::tool::ToolRegistry`]）由汇总卡在 `tool.rs` 的 `default_registry()`
//! 里做——本域不碰共享文件，避免多卡并行时争同一文件。
//!
//! 工具名直接复用执行体里的 [`EXEC_TOOL_NAME`]，与 [`crate::mode::WRITE_TOOLS`]
//! 中的 `shell.exec` 逐字一致：名字漂移会被权限门拒掉。
//!
//! 描述文本写清三件事：**做什么**（执行命令并回传退出码/输出）、**失败怎么表现**
//! （显式错误，不静默成功）、**能力边界**（必经沙箱与危险命令审批）。

use crate::tool::ToolKind;

pub use crate::executor::EXEC_TOOL_NAME as SHELL_EXEC_TOOL;

/// 超时（毫秒）：命令比文件读写慢，与冻结规格一致。
pub const SHELL_EXEC_TIMEOUT_MS: u64 = 30_000;

/// 工具描述：做什么 + 失败怎么表现 + 能力边界。
pub const SHELL_EXEC_DESCRIPTION: &str = concat!(
    "在受控沙箱中执行一条命令，返回 exit_code、stdout、stderr、timed_out、truncated 与 sandbox 标签。",
    "参数 command（必填，非空字符串，按 shell 语义分词为 argv）；cwd（可选，字符串，工作目录）。",
    "能力边界：命令一律经 GuardedExec 判定 → 沙箱层执行，没有绕过沙箱的路径；",
    "危险命令（rm -rf、git reset --hard、关机/重启、管道执行脚本等）强制人工确认，",
    "即使该工具已被显式自动放行、且沙箱可用。",
    "失败时显式报错而不静默成功：参数非法、策略拒绝、需人工审批、沙箱不可用都会说明原因；",
    "非零退出码、超时与输出截断作为结果字段如实标注，不伪装成成功。",
);

/// 本域工具的 spec 清单：`(名字, 描述, kind, sensitive_arguments, timeout_ms)`。
///
/// `kind = Exec`：命令执行类。权限门的危险命令强制审批层只对 [`ToolKind::Exec`]
/// 生效（`permission/mod.rs` 的危险命令层），用 `Write` 会静默跳过该层。
/// `sensitive_arguments = true`：命令串可能含凭据，也让敏感动作正则表生效。
pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)> {
    vec![(
        SHELL_EXEC_TOOL,
        SHELL_EXEC_DESCRIPTION,
        ToolKind::Exec,
        true,
        SHELL_EXEC_TIMEOUT_MS,
    )]
}
