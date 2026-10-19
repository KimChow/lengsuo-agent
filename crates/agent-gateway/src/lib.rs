//! Authenticated loopback gateway. Protocol: AGENT-KICKOFF.md §2.
#![cfg_attr(not(windows), forbid(unsafe_code))]
// 唯一 unsafe 例外：Windows 握手 ACL（server.rs 内声明的 handshake_windows 模块）的
// windows-sys FFI（GetNamedSecurityInfoW/SetEntriesInAclW/SetNamedSecurityInfoW），
// 仅 windows target 编译；非 Windows 平台 forbid 照旧（与 agent-sandbox lib.rs 同款处理）。
mod rpc;
mod server;
pub use server::{GatewayConfig, GatewayHandle};
