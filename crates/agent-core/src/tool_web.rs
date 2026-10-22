//! `web.fetch` 的 spec 清单（web 域）。
//!
//! 本模块只产出**注册规格**：工具名、描述、类别、敏感标记与超时。真正的注册动作
//! （写入 [`crate::tool::ToolRegistry`]）由汇总卡在 `tool.rs` 的 `default_registry()`
//! 里做——本域不碰共享文件，避免多卡并行时争同一文件。
//!
//! 规格与 [`crate::mode::READONLY_TOOLS`] 里的 `web.fetch` 逐字一致：名字漂移会被
//! 权限门第 2 门（白名单未命中）拒掉。
//!
//! 描述文本写清三件事：**做什么**（抓 http/https 网页正文）、**失败怎么表现**
//! （显式错误，不返回空串）、**能力边界**（只读、无脚本、字节上限、已脱敏）——
//! 模型据此判断何时该用、何时不该用。

use crate::tool::ToolKind;

/// 工具名：与 [`crate::mode::READONLY_TOOLS`] 中的同名项逐字一致。
pub const WEB_FETCH_TOOL: &str = "web.fetch";

/// 超时（毫秒）：网络请求预算。
pub const WEB_FETCH_TIMEOUT_MS: u64 = 30_000;

/// 返回给模型的正文字节上限（**上下文**上限）。
///
/// 取 256 KiB：既能容下常规文档页，又不至于把整页大文件塞进模型上下文。
/// 适配层（[`crate::web_adapter`]）按此截断，并**显式标注**截断——不静默丢内容。
pub const WEB_FETCH_MAX_BYTES: usize = 256 * 1024;

/// 传输层响应体字节上限（**下载**上限，= 上下文上限的 4 倍）。
///
/// 两级上限各有分工：
/// - 传输层（本值）挡住「下载 10 MB」这件事，内存与网络都有界；
/// - 上下文层（[`WEB_FETCH_MAX_BYTES`]）决定「多少字节进模型上下文」，由适配层执行并标注。
///
/// 传输层必须**宽于**上下文层：若两者同值，HTML 转纯文本后正文会短于上限，
/// 传输层的截断就变成**静默截断**（模型看不到「被截了」的提示）。留出 4 倍余量后，
/// 传输层截断发生时正文几乎必然仍超上下文上限 ⇒ 仍由适配层显式标注。
pub const WEB_FETCH_BODY_BYTES: usize = 4 * WEB_FETCH_MAX_BYTES;

/// 工具描述：做什么 + 失败怎么表现 + 能力边界。
pub const WEB_FETCH_DESCRIPTION: &str = concat!(
    "抓取指定 http/https 地址的网页正文并转成纯文本返回。",
    "参数 url（必填，http/https 完整地址）。",
    "失败时以显式错误返回（不返回空串、不静默吞掉）：",
    "地址非法或不是 http/https 协议、响应非 2xx、网络不可达、超过 30 秒超时，都会说明失败原因。",
    "能力边界：只读抓取——不提交表单、不带凭据登录、不执行页面脚本；",
    "响应体有 256 KiB 上限，超限会显式截断并注明；",
    "返回内容已脱敏，页面里的 token、私钥样式串不会以明文进入模型上下文。",
);

/// 本域工具的 spec 清单：`(名字, 描述, kind, sensitive_arguments, timeout_ms)`。
///
/// `sensitive_arguments = true`：URL 可能带 token/签名参数，让权限门的敏感动作扫描生效。
/// `kind = Read`：只读网络访问，不写任何状态，readonly 权限模式下仍可用。
pub fn tool_specs() -> Vec<(&'static str, &'static str, ToolKind, bool, u64)> {
    vec![(
        WEB_FETCH_TOOL,
        WEB_FETCH_DESCRIPTION,
        ToolKind::Read,
        true,
        WEB_FETCH_TIMEOUT_MS,
    )]
}
