//! 宿主能力服务（技能目录 / MCP 客户端 / 凭据解析）——C1 接线骨架。
//!
//! ## 为什么有这个模块
//!
//! `agent-skills` / `agent-mcp` / `credential-ref` 三个库此前是**孤儿**：它们各自
//! 写完即冻住，没有任何其他 crate 依赖，也没有任何生产调用点。本模块是它们的
//! **组装口**：宿主（网关）在构造 [`crate::runtime::Runtime`] 时把「技能目录、
//! MCP 客户端、凭据解析」三域服务一次性注入，运行期由 [`crate::runtime`] 的工具
//! 步按需取用。C2–C13 各卡产出的库，注入点都收在这里。
//!
//! 参考实现：wb 10 册 §2.2/§2.3（MCP 配置、Trust 机制与连接状态机）与 09 册 §2.1
//! （技能两级存储；本卡只做注入口，根路径解析归 C3）。
//!
//! ## 三字段都可选（`None` ⇒ 既有行为）
//!
//! 每个字段缺省时不 panic、也不静默降级成别的语义：
//! - [`HostServices::skills`] 为 `None` ⇒ 运行时不提供技能面（技能注入属 C5）；
//! - [`HostServices::mcp`] / [`HostServices::mcp_dispatch`] 为 `None` ⇒ `mcp:` 前缀
//!   工具走既有执行器路径（零回归）；
//! - [`HostServices::credentials`] 为 `None` ⇒ 凭据解析保持 [`crate::blobs`] 的
//!   fail-closed 桩语义（`Err`，不返回明文）。
//!
//! ## 与 tokio 的边界（为什么有 `mcp_dispatch`）
//!
//! `AGENT-KICKOFF.md` §3 硬约束：**agent-core 不得依赖 tokio**。而既有的
//! [`TrustedClient::call_tool`] 是 `async fn` 且取 `&mut self`——同步的
//! [`crate::runtime::Runtime::step`] 路径既无法 `.await` 它，也无法从一个共享的
//! `Arc<TrustedClient>` 取得 `&mut`。因此本卡把「MCP 工具的真实调用」抽象成一个
//! **同步端口** [`McpToolDispatcher`]：它由持有 tokio 的宿主（网关）实现，内部
//! 负责把调用投递到 [`TrustedClient`] 所在的运行时。详细偏离见
//! `docs/host-services-status.md` 的「偏离清单」。这一分层正是 kickoff 所说的
//! 「用 async-trait 或回调抽象，网关层才碰运行时」。

use std::sync::Arc;

use agent_mcp::TrustedClient;
use agent_skills::SkillCatalog;

/// 宿主能力服务（技能目录 / MCP 客户端 / MCP 调用端口 / 凭据解析）。
///
/// 全部字段可选：未注入时对应能力保持既有行为（不 panic、不静默降级成别的语义）。
///
/// 克隆共享：内部一律 `Arc`，注入后由 `Runtime` 长期持有。
#[derive(Clone, Default)]
pub struct HostServices {
    /// 技能目录（wb 09 册 §2.1 的两级存储；根路径解析属 C3）。
    pub skills: Option<Arc<SkillCatalog>>,

    /// MCP 受控客户端（wb 10 册 §2.2 的 Trust 机制）。
    ///
    /// 门面独占持有连接与 [`agent_mcp::TrustGate`]，是 MCP 能力的**持有者**；
    /// 真实调用经 [`HostServices::mcp_dispatch`] 投递（见模块文档的 tokio 边界）。
    pub mcp: Option<Arc<TrustedClient>>,

    /// 凭据解析口（[`crate::blobs::CredentialResolver`] 的类型化版本）。
    pub credentials: Option<Arc<dyn CredentialResolver>>,

    /// MCP 工具调用的同步端口：同步工具步经它把 `mcp:<server>:<tool>` 调用投递到
    /// 宿主持有的 [`TrustedClient`]。`None` ⇒ `mcp:` 工具不被本运行时接管。
    ///
    /// 这一字段是相对任务书「先只做三个字段」的**必要偏离**：任务书设想的
    /// `Arc<TrustedClient>` 在 tokio-free 的同步 `execute.rs` 里不可调用，
    /// 故把「能力持有」（[`HostServices::mcp`]）与「同步调用」（本字段）拆开，
    /// 理由与证据见 `docs/host-services-status.md`。
    pub mcp_dispatch: Option<Arc<dyn McpToolDispatcher>>,
}

impl std::fmt::Debug for HostServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // MCP/凭据的 Debug 可能牵连连接诊断；只暴露「是否注入」，不 dump 内部。
        f.debug_struct("HostServices")
            .field("skills", &self.skills.is_some())
            .field("mcp", &self.mcp.is_some())
            .field("credentials", &self.credentials.is_some())
            .field("mcp_dispatch", &self.mcp_dispatch.is_some())
            .finish()
    }
}

impl HostServices {
    /// 是否注入了 MCP 调用能力（持有 + 端口齐备才可路由）。
    #[must_use]
    pub fn has_mcp_dispatch(&self) -> bool {
        self.mcp_dispatch.is_some()
    }

    /// 把 `mcp:<server>:<tool>` 形状的工具名解析为 `(server, tool)`。
    ///
    /// 规则（wb 24 册 规则 A24-1）：id 格式 `mcp:<server>:<tool>`。解析要求三段
    /// 非空、且只在**前两个** `:` 处切分——`tool` 段允许含 `:`（不吞掉后续冒号）。
    /// 不匹配（前缀不是 `mcp:`、段为空、缺省 `tool` 段）⇒ `None`，调用方保持既有路径。
    #[must_use]
    pub fn parse_mcp_tool_id(name: &str) -> Option<(&str, &str)> {
        let rest = name.strip_prefix("mcp:")?;
        let (server, tool) = rest.split_once(':')?;
        if server.is_empty() || tool.is_empty() {
            return None;
        }
        Some((server, tool))
    }
}

/// 凭据解析口（`blobs.rs` 既有桩 [`crate::blobs::CredentialResolver`] 的类型化版本）。
///
/// 与 `blobs.rs` 的版本相比，返回 `String`（明文串，工具参数的常见形态）而非
/// `Vec<u8>`，并把错误类型从 [`crate::error::CoreError`] 收窄为 `String`：本口是
/// 宿主实现的**边界 trait**，不应把核心错误类型强加给外部实现者。`blobs.rs` 的
/// 既有 trait 保持不变（本卡不改其签名）。
pub trait CredentialResolver: Send + Sync {
    /// 把 `cred_ref` 解析为明文；未接入 ⇒ `Err`（fail-closed）。
    fn resolve(&self, reference: &str) -> Result<String, String>;
}

/// 未注入凭据服务时的 fail-closed 桩：一律 `Err`，不返回明文、不误报能力。
///
/// 语义与 [`crate::blobs::NoopCredentialResolver`] 一致（都是「凭据库未接入」），
/// 只是错误类型随本 trait 收窄为 `String`。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCredentialResolver;

impl CredentialResolver for NoopCredentialResolver {
    fn resolve(&self, _reference: &str) -> Result<String, String> {
        Err("凭据库未接入（cred_ref 需由宿主注入）".to_string())
    }
}

/// MCP 工具调用的同步端口。
///
/// 宿主（网关）实现本 trait，内部把调用投递给持有的 [`TrustedClient`]（其
/// `call_tool` 是 `async`，需宿主在有 tokio 的语境里驱动）。运行时的工具步是
/// 同步的，故只经本端口发起 MCP 调用。
///
/// 契约：
/// - 返回 `Ok(value)` ⇒ 调用成功，`value` 为工具结果（进历史与事件）；
/// - 返回 `Err(message)` ⇒ 调用失败（门拒绝 / 传输错误 / 工具自身 `isError`），
///   运行时会走既有 `Err(message)` 结算路径（回合继续）。
pub trait McpToolDispatcher: Send + Sync {
    /// 调用 `server` 上的 `tool`，参数 `args`。
    fn call_tool(
        &self,
        server: &str,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}
