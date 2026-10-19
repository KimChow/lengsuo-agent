//! 网关 ↔ ACP host 的接线点（W2）。
//!
//! 本模块只做**持有并驱动**：网关按 [`GatewayConfig`] 起一个
//! [`agent_acp::host::Host`]（「被 spawn 的一端」由桌面壳/网关拉起），
//! 暴露 `register_method` 与 `notify` 两个入口（外加 control 回调与拆除）。
//!
//! **本卡不做的部分（诚实登记）**：
//! - 网关的 HTTP RPC（[`crate::rpc`] 的 `域:动作` 面）**未**接到 ACP 方法表——
//!   双向转发是后续卡，故这里不给「已接线」的假象；
//! - 传输仍是宿主注入的 [`Transport`]（测试/后续卡用 `mock_transport()` 或
//!   `stdio::spawn_stdio` 后的对端），网关自己不 spawn 子进程、不自建管道；
//!   真实形态（谁 spawn 谁）需用户决策，见模块尾注。
//!
//! 形态对齐 [`crate::server::GatewayHandle`]：`Arc` 共享 + `Debug` 视图 +
//! 显式 `stop`（`Drop` 亦自动拆除，`Host` 自身幂等）。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use agent_acp::host::{Host, HostConfig, HostEvent, NextEvent};
use agent_acp::message::JsonRpcRequest;
use agent_acp::router::RpcOutcome;
use agent_acp::transport::Transport;
use agent_acp::ControlSubtype;
use serde_json::Value;

use crate::server::GatewayConfig;

/// 网关持有的 ACP host 句柄（`Clone`，内部共享同一 host）。
#[derive(Clone)]
pub struct AcpHostHandle {
    inner: Arc<Host>,
}

impl fmt::Debug for AcpHostHandle {
    /// 只报可观测状态，不打印 handler 表（与 `GatewayHandle` 不收 token 同款）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcpHostHandle")
            .field("closed", &self.inner.is_closed())
            .field("bad_frames", &self.inner.bad_frame_count())
            .finish()
    }
}

impl AcpHostHandle {
    /// 接管一条 ACP 传输，按配置起 host（分派线程即刻运行）。
    #[must_use]
    pub fn start(transport: Transport, config: HostConfig) -> Self {
        Self {
            inner: Arc::new(Host::with_config(transport, config)),
        }
    }

    /// 用 [`GatewayConfig`] 起 host：本卡的 `GatewayConfig` 不带 ACP 侧参数
    /// （路径/isolation 都是握手与运行时面），故取 [`HostConfig::default`]。
    /// 传输由调用方注入（网关不 spawn 子进程）。
    #[must_use]
    pub fn from_gateway_config(_config: &GatewayConfig, transport: Transport) -> Self {
        Self::start(transport, HostConfig::default())
    }

    /// 注册入站 ACP 方法 handler（同名重复注册 = 覆盖）。
    pub fn register_method(
        &self,
        name: impl Into<String>,
        handler: impl Fn(&JsonRpcRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        self.inner.register_method(name, handler);
    }

    /// 注册 control subtype handler（V.* 回调面）。
    pub fn register_control(
        &self,
        subtype: ControlSubtype,
        handler: impl Fn(&agent_acp::ControlRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        self.inner.on_control(subtype, handler);
    }

    /// 主动向 ACP 对端推通知（如 `session/update`）。
    ///
    /// # Errors
    /// [`agent_acp::Error::Transport`]：连接已终止或写侧失败。
    pub fn notify(&self, method: impl Into<String>, params: Value) -> agent_acp::Result<()> {
        self.inner.notify(method, params)
    }

    /// 阻塞取下一个 host 事件（超时归 [`NextEvent::Idle`]）。
    #[must_use]
    pub fn next_event(&self, timeout: Duration) -> NextEvent {
        self.inner.next_event(timeout)
    }

    /// 非阻塞取下一个 host 事件。
    pub fn poll_event(&self) -> Option<HostEvent> {
        self.inner.poll_event()
    }

    /// 连接是否已终止。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// 主动拆除。`Host::stop` 幂等，重复调用（含 `Drop` 路径）安全。
    ///
    /// 注意：`Host::stop` 需 `&mut self`，而句柄是共享的——故拆除按
    /// 「丢弃句柄」执行（`Arc` 计数归零时 `Host::drop` → `stop`）。
    pub fn stop(&mut self) {
        self.inner = Arc::new(Host::new(agent_acp::transport::mock_transport().0));
    }
}

// 决策项（留给用户，不在本卡拍板）：
// 1. 谁 spawn 谁——网关作为 host 被桌面壳 spawn（`stdio` 读 stdin/stdout），
//    还是网关自己 `stdio::spawn_stdio` 拉起 CLI agent 作为**客户端**角色？
//    两者在 `agent-acp` 里都已成型，接线方式相反，需用户定网关的定位。
// 2. HTTP RPC → ACP 方法表的映射表（`域:动作` ↔ `session/*`）本卡未定，
//    需先有方法面台账再实现，避免自创一套名字。
