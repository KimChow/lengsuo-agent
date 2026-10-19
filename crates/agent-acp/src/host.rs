//! host 侧角色（被 spawn 的一端）：与 [`crate::client`] 共用同一套
//! [`crate::codec`] / [`crate::message`] / [`crate::control`] / [`crate::transport`]
//! 类型，角色相反——外部（桌面壳 / 网关）拉起本端后，本端从帧源读帧、按
//! JSON-RPC 2.0 语义分派请求、把响应写回帧汇，并可**主动推**通知。
//!
//! **公开名面**：入口是 [`Host`]；事件是 [`HostEvent`]；出队结果是 [`NextEvent`]；
//! 配置是 [`HostConfig`]。`register_method` / `on_control` / `notify` /
//! `respond_control` / `next_event` 等方法见 [`Host`] 的逐条注释。
//!
//! 结构：`Host` 持有共享路由状态（写侧 sink + handler 表 + 出站编号），
//! 一条分派线程从帧源拉帧并按顶层归类（[`crate::control::TopLevel`]）。与
//! [`crate::client`] 的差异只有三处：
//! - **无 pending 表**：host 不发起请求，故没有「等回执」这一半；
//! - **主动推送**：`notify` 把 `session/update` 一类通知写回对端（host 侧存在的理由）；
//!   出站帧与分派线程的应答共用 [`host_router::HostRouteState::write_frame`] 的同一把
//!   写锁，帧字节序即写序，不会交错半帧；
//! - **重建路由状态**：`Client` 的路由状态在 `client.rs` 私有（`state` 字段 + 私有
//!   `RouteState`），本角色按同样语义**重建**一份（`HostRouteState`），不触碰
//!   `router.rs` 既有语义。
//!
//! 入站帧语义（与 [`crate::router`] 的 `RouteState` 逐条一致）：
//! - `Rpc::Request` → handler 表命中则同步执行回响应（**id 逐字回显**）；未命中
//!   自动回 `-32601`（**不得静默丢弃**，对端会悬挂）；
//! - `Rpc::Notification`（如 `session/cancel`）→ 入有界事件队列；
//! - `Rpc::Response` → host 不发起请求，计 `unexpected_responses`（不 panic）；
//! - `ControlRequest` → subtype handler 命中则回 `control_response`，未命中入队待
//!   [`Host::respond_control`] 延后应答；
//! - `ControlNotification` / `ControlResponse` → 入事件队列 / 计数；
//! - 畸形帧 → `bad_frames` 计数，**不断连**（字节流仍对齐）。
//!
//! 拆除：`stop() → sink 关闭 → 分派线程退出 → 一次性执行 Transport 的拆除钩子`，
//! [`Drop`] 自动执行且幂等（对齐 [`crate::client::Client`] 的拆除语义）。
//!
//! 文件切分（单文件 400 行软目标）：本文件是角色门面与文档；读侧状态机在
//! [`crate::host_router`]（归类语义 + 计数），写侧与生命周期在
//! [`crate::host_send`]（`Host` 的 `impl` 与 `Drop`）。三者的语义是同一张表。

pub use crate::host_router::{
    HostEvent, NextEvent, ACP_ERROR_HOST_CONNECTION_CLOSED, DEFAULT_MAX_EVENTS,
};
pub use crate::host_send::Host;

use std::time::Duration;

use crate::codec::DEFAULT_MAX_LINE_BYTES;

/// 分派线程轮询间隔（同时决定 [`Host::stop`] 的响应延迟上界）。
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// host 侧配置。
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// 事件队列上限（满丢最旧并计数）。
    pub max_events: usize,
    /// 单行帧上限（真实字节源形态用；见 [`Host::from_bytes`]）。
    pub max_line_bytes: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_MAX_EVENTS,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
        }
    }
}
