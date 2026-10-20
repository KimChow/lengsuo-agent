//! 摘要式上下文压缩：决策、提示词构造与产物落历史（h2-compaction-summary 卡）。
//!
//! 验收对象是 [`agent_core::runtime::compaction`] 的纯函数面：给定历史与固定的
//! 估算口 / 摘要文本，断言「该不该压 / 切在哪 / 提示词长什么样 / 产物长什么样」。
//! **不碰网络、不读时钟**（本卡不接线真实 Provider，见卡面 E）。
//!
//! 原单文件 750 行，2026-10-20 按主题拆分为五组（断言与用例顺序零变化）：
//! `ceiling.rs` = 上限公式（`calculate_ceiling`）；
//! `due.rs` = 压缩决策（`due()` 的三条否定条件与触发）；
//! `split.rs` = 切分与压平（`split_conversation` / `flatten_and_drop_oldest` / 截断）；
//! `apply.rs` = 产物与运行时接线（`apply` / 历史边界 / 落库 / 回放）；
//! `prompt.rs` = 提示词与常量契约（`build_prompt` / 模板 / 规则 / 常量 / 错误类型）。
//! 共享 helper（常量镜像、估算口、消息构造器）见 `support.rs`。
mod apply;
mod ceiling;
mod due;
mod prompt;
mod split;
mod support;
