//! agent-prompts：提示词引擎（模式模板 × 变量注入）。
//!
//! 纯域逻辑：不读写文件、不访问网络、不取系统时钟；输出完全由
//! [`PromptMode`] + [`PromptVars`] 决定，同输入同输出（无时间戳注入）。
//! 规格源：worktree 根 `PROMPTS-SPEC.md`（v0 冻结要点）。
//!
//! 结构：[`PromptTemplate`] 由 [`Segment`] 序列组成（文本段 / 变量位 / 门禁段），
//! [`Composer::build`] 按模式取模板并渲染；`plan` 复用 `craft` 模板再追加
//! [`Composer::plan_gate`] 门禁片段，`expert` 在 `craft` 之上追加专家协作段。
//!
//! 另有 参考端（参考客户端 调研资料）提示词与工具描述资产：`assets/*.md` 忠实转写件
//! 经 `include_str!` 编译期嵌入，由 [`ASSETS`] / [`asset`] 只读访问，
//! 每项带来源册名标注；不参与渲染，不引入 IO。
//!
//! ```
//! use agent_prompts::{Composer, PromptMode, PromptVars};
//!
//! let mut vars = PromptVars {
//!     ai_name: "小澜".into(),
//!     user_name: "Kim".into(),
//!     soul: "冷静、直接。".into(),
//!     workspace: "/work/demo".into(),
//!     date: "2026-05-06".into(),
//!     ..PromptVars::default() // 其余变量留空 → 整段省略
//! };
//! vars.memory_project = "决定：plan 复用 craft 模板".into();
//! let prompt = Composer::build(PromptMode::Plan, &vars);
//! assert!(prompt.contains("## 工具能力（craft · 全工具）")); // craft 段完整复用
//! assert!(prompt.contains(Composer::plan_gate()));           // 追加规划门禁
//! assert!(prompt.contains("## 项目记忆\n决定："));             // 非空记忆段成段
//! assert!(!prompt.contains("## 用户记忆"));                    // 空槽位不留标题
//! ```

mod assets;
mod compose;
mod model;
mod template;
mod templates;

pub use assets::{asset, PromptAsset, ASSETS};
pub use compose::Composer;
pub use model::{
    Cond, PromptMode, PromptTemplate, PromptVars, Segment, PROMPT_VAR_KEYS, WORK_MODE_KEY,
};
