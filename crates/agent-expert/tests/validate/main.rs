//! 专家包校验器测试（h4-expert-validate 卡）：24 项校验器逐条断言。
//!
//! 覆盖矩阵：
//! - **正例**：两份真实样本（单人专家 / 团队专家）通过校验；
//! - **负例**：逐条 §17 编号的违规项被报出，严重级别符合规格；
//! - **契约**：解析失败 fail-closed（不降级放行）、错误码稳定、team 类型识别。
//!
//! 原单文件 1067 行（超 700 硬上限），按主题机械拆分为五组
//! （断言与用例顺序零变化）：
//! `positive.rs` = 正例，`negative.rs` = §17 编号 01–13，
//! `negative_team.rs` = §17 编号 15–25，
//! `contract.rs` = fail-closed 与错误码契约，
//! 共享夹具（PNG 构造 / 样本成员表 / 包构造 / 断言助手）在 `support.rs`。
mod contract;
mod negative;
mod negative_team;
mod positive;
mod support;

pub use support::*;
