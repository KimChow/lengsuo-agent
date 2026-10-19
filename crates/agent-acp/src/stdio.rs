//! 真实 stdio 传输。两个方向相反、但共用同一套线程/拆除设施的形态：
//!
//! - [`spawn_stdio`]：**客户端侧**（本端 spawn agent 子进程），持其 stdin/stdout；
//!   帧汇 = 子进程 stdin，帧源 = 子进程 stdout。
//! - [`host_stdio_transport`]：**host 侧**（本端被外部桌面壳 spawn），托管**当前
//!   进程**的 stdin/stdout；帧汇 = 本进程 stdout，帧源 = 本进程 stdin。方向与
//!   客户端侧相反，IO 实现（`read_loop`/`spawn_writer_to`/有界拆除）完全复用。
//!
//! 本文件是**模块门面**：三个子模块各自负责一个方向/一组规则，这里只做 `pub use`
//! 重导出（`mod` 声明在 crate 根，见 `lib.rs`），使 `agent_acp::stdio::*` 下的既有
//! 路径逐字不变。
//!
//! - [`crate::stdio_client`]：客户端侧——[`SpawnConfig`]、[`spawn_stdio`]、写/读
//!   线程与共用设施（有界读循环 `read_loop` 与有界 join 由 host 侧复用，不复制两份）；
//! - [`crate::stdio_host`]：host 侧——[`host_stdio_transport`] 与 host 形态的装配/
//!   拆除；
//! - [`crate::stdio_env`]：spawn 前的环境 scrub 规则（纯函数 + 白名单常量）。
//!
//! 无论哪一侧，字节流 → reader 线程（`LineDecoder` + [`crate::codec::decode_frame`]
//! 归类），writer 线程 ← 帧汇通道。同步线程模型。两侧语义与安全基线详见各自模块头。

pub use crate::stdio_client::{spawn_stdio, SpawnConfig, DEFAULT_SHUTDOWN_TIMEOUT};
pub use crate::stdio_env::{env_pair_allowed, is_credential_env, SYSTEM_ENV_KEEP};
pub use crate::stdio_host::host_stdio_transport;
