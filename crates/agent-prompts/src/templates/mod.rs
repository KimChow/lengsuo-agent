//! 分模式的模板段集（按模式拆文件，对应参考端 `interactionmode/*/fragments/`）。
//!
//! - `common.rs`：跨模式公共段（对应参考端 `prompt-common` 共享片段层）；
//! - `craft.rs`：craft 段集（plan 与 expert 复用，21 册 §3）；
//! - `ask.rs` / `plan.rs` / `quick.rs` / `expert.rs`：各模式的差异段；
//! - `identity.rs`：身份注入段（`<identity_context>` 两分支，18 册 §13）。
//!
//! 每个模式文件只放该模式的段，不重排公共段序；`template.rs` 只做门面。

pub(super) mod ask;
pub(super) mod common;
pub(super) mod craft;
pub(super) mod expert;
pub(super) mod identity;
pub(super) mod plan;
pub(super) mod quick;
