//! ACP **信封级判别键面**（顶层 `type` 字段）的结构化登记断言（W2 · lane proto）。
//!
//! # 这条断言治什么病
//!
//! ACP 的 control 面在 JSON-RPC 三分类**之外**还有一层信封，顶层靠 `type` 字段
//! 判别（`control_request` / `control_notification` / `control_response`）。
//! 这三个字面量有**双来源派生**——**两处独立手写**，缺机器断言：
//!
//! - **来源 A（权威）**：`crates/agent-acp/src/control.rs` 里三个
//!   `serde(rename = "…")`（`ControlRequestTag` / `ControlNotificationTag` /
//!   `ControlResponseTag`）；
//! - **来源 B（消费方）**：同文件 `TopLevel::classify` 的 `match` 分支字面量。
//!
//! # 为什么它是**承重**的
//!
//! `classify` 是 `agent-acp` 编解码的**唯一入口**（`codec.rs` 的 `decode_frame`
//! 调它）。任一来源的字面量漂移 ⇒ 该信封**不被识别** ⇒ 落兜底臂 ⇒
//! 抛 `MalformedMessage` ⇒ **整帧被拒**（**硬失败**，不是静默降级）；且 serde
//! 侧用同一拼写去反序列化 ⇒ **编解码两端会一起错**（对端可能认得、我方不认）。
//!
//! # 为什么是**源码扫描**而不是运行时调用
//!
//! `agent-acp` 不是本 crate 的依赖（`Cargo.toml` 属共享区、本卡不得改），而
//! `agent-gateway` 虽依赖它却**没有 `pub use` 重新导出** `TopLevel` ⇒ 运行时
//! **够不着**。这**不是降级**：本卡的核心断言就是「两处独立手写的字面量是否
//! 一致」，**源码扫描正是对症手段**（运行时调用反而看不到「手写 vs 派生」这个
//! 双来源问题）。
//!
//! # 口径
//!
//! **只守关系，不锁数量**：实现正当新增一个信封类型时，把新标签登记进
//! `contracts_check::ACP_ENVELOPE_TAGS` 即可；任何「恰好 3 个」的断言都会在正当
//! 扩充时必红（教训：`tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。
//!
//! 台账：`docs/contracts/acp-host.md` §12。
//!
//! # 本目录分组说明
//!
//! - `surface.rs`：登记面与两个来源面的结构比对（`serde rename` / `classify` 字面量臂）；
//! - `scanner.rs`：源码扫描基元与单臂语义（含扫描器自证与漂移检测）。
//!
//! 共享的 `include_str!` 常量与两个判别结果类型留在本文件（target 根）。

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
///
/// `agent-acp` 不是本 crate 的依赖（dev-dependencies 里没有它，`Cargo.toml` 属
/// 共享区、本卡不得改），故按 `tests/acp_control_surface.rs` 的先例用
/// `include_str!` 绑定源文件**文本**，在测试里做源码扫描。
const CONTROL_SOURCE: &str = include_str!("../../../agent-acp/src/control.rs");

/// **来源 B** 的一个 `match` 臂（从 `classify` 扫出）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifyArm {
    /// 字面量臂的 wire 值；兜底臂为 `None`。
    wire: Option<String>,
    /// 字面量臂映射到的变体名（如 `ControlRequest`）；兜底臂为 `None`。
    variant: Option<String>,
    /// 臂的正文（多行拼接）；兜底臂里含错误类型与拒绝理由。
    body: String,
}

/// 一个顶层 `type` 取值在 `classify` 里的归类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Classify {
    /// 命中某个字面量臂。
    Recognized {
        /// 命中的变体名。
        variant: String,
    },
    /// 落兜底臂 ⇒ **整帧被拒**。
    Rejected {
        /// 兜底臂正文（含错误类型与拒绝理由）。
        reason: String,
    },
    /// **没有兜底臂** ⇒ 未登记的 `type` 不再被拒（契约破了）。
    Unbounded,
}

mod scanner;
mod surface;
