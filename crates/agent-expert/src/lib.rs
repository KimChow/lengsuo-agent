//! 专家包（expert package）数据结构层。
//!
//! 专家不是第四套机制，而是「expert 模式的用户自定义实例」：本 crate 只承载
//! **数据**（manifest + 人格 + 技能挂载），运行时复用既有的 expert 模式链路。
//!
//! 真相来源：调研 `harness-expert.md` §3（plugin.json 逐字段判定表）。
#![forbid(unsafe_code)]

// 具体结构体与错误类型由 h4-expert-crate 卡填充。
