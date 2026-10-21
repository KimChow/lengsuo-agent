//! OpenAI 兼容模型网关适配器。
//!
//! 职责：把 `agent_core::provider::TurnContext` 翻译成 OpenAI Chat Completions 请求，
//! 把响应翻译回 `agent_core::provider::ProviderStep` 序列，并按 OpenAI 规范解析 `usage`。
//!
//! ⛔ 不自创协议：请求体与响应解析严格按 OpenAI Chat Completions 规范。
//! ⛔ 不做流式（SSE 解析是独立一张卡）。

pub mod config;
pub mod request;
pub mod response;

pub use config::OpenAiConfig;
