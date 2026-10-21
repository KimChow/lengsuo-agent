//! OpenAI 兼容模型网关适配器。
//!
//! 职责：把 `agent_core::provider::TurnContext` 翻译成 OpenAI Chat Completions 请求，
//! 把响应翻译回 `agent_core::provider::ProviderStep` 序列，并按 OpenAI 规范解析 `usage`。
//!
//! ⛔ 不自创协议：请求体与响应解析严格按 OpenAI Chat Completions 规范。
//! ⛔ 不做流式（SSE 解析是独立一张卡）。
//! ⛔ 不做工具调用往返（`tools` 字段不送出）——传了但解析不了 = 模型调了但接不住。
//!
//! ## 同步、无 tokio
//!
//! HTTP 走 `ureq`（同步、纯 Rust）。`agent-core` 不依赖 tokio，
//! `Provider` trait 的方法必须保持同步 ⇒ 本适配器全程同步。
//!
//! ## 错误映射
//!
//! 本仓 `CoreError` 没有「外部服务调用失败」这一档，而 `agent-core/src/error.rs`
//! 不在本卡可改范围内 ⇒ 统一映射到 `CoreError::Io`：
//! - `path` = 请求 URL（诊断时看得出是打哪个端点）；
//! - `source` = `io::Error`，其文案带**状态码 + 响应体前 200 字符**（脱敏后）
//!   或传输层错误分类。
//!
//! 该变体在网关侧落在**兜底分支 ⇒ 信封 500**（`agent-gateway/src/rpc.rs:81-85`），
//! 与「上游模型挂了」语义相符（不是 400「配置非法」，也不是 404）。
//!
//! ⛔ 网络错误 / 超时**显式报错**，绝不静默回退到 `EchoProvider` 之类。

pub mod config;
pub mod request;
pub mod response;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, StepOptions, TokenUsage, TurnContext, Uuid};
use agent_core::redact::Redactor;
use agent_core::schema::Mode;
use serde_json::Value;

pub use config::OpenAiConfig;
pub use response::ParseError;

/// 错误响应体最多读取的字节数（够看清错误，又不整包吞下）。
const ERROR_BODY_CAP: usize = 4096;

/// 错误文案里带出的响应体字符数上限。
const ERROR_BODY_PREVIEW_CHARS: usize = 200;

/// 真实模型 provider：打 OpenAI 兼容网关。
///
/// 用法：`RuntimeBuilder::provider(Arc::new(OpenAiProvider::new(config)))`。
#[derive(Debug)]
pub struct OpenAiProvider {
    config: OpenAiConfig,
    agent: ureq::Agent,
    /// 最近一次成功调用的 usage（`take_usage` 取走即清）。
    usage: Mutex<Option<TokenUsage>>,
}

impl OpenAiProvider {
    /// 按配置构造（不建连、不校验端点可达性——那是调用时的事）。
    pub fn new(config: OpenAiConfig) -> Self {
        // 0 秒无意义（每次请求立刻超时）⇒ 至少 1 秒。
        let timeout = Duration::from_secs(config.timeout_secs.max(1));
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        Self {
            config,
            agent,
            usage: Mutex::new(None),
        }
    }

    /// 最近一次成功调用的 usage（**只读**，不消费）。
    ///
    /// 上游响应缺 `usage` ⇒ `None`（不估算、不报错）。
    pub fn last_usage(&self) -> Option<TokenUsage> {
        self.usage.lock().ok().and_then(|slot| *slot)
    }

    /// 取走最近一次成功调用的 usage（取走即清）。
    pub fn take_usage(&self) -> Option<TokenUsage> {
        let mut slot = self.usage.lock().ok()?;
        slot.take()
    }

    /// 当前配置（只读）。
    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// 打一次网关，产出步骤序列。
    fn call(&self, context: &TurnContext) -> Result<Vec<ProviderStep>, CoreError> {
        let body = request::build_request(context, &self.config.model);
        let url = self.config.chat_completions_url();
        let mut req = self.agent.post(&url);
        if let Some(key) = self.config.api_key.as_deref() {
            if !key.trim().is_empty() {
                req = req.set("Authorization", &format!("Bearer {}", key.trim()));
            }
        }
        let response = req
            .send_json(body)
            .map_err(|error| self.map_error(&url, error))?;
        let text = response
            .into_string()
            .map_err(|error| self.io_error(&url, format!("读取响应体失败: {error}")))?;
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            CoreError::Json(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("响应体不是合法 JSON: {error}"),
            )))
        })?;
        let (steps, usage) = response::parse_response(&value).map_err(|error| {
            CoreError::Json(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )))
        })?;
        if let Some(usage) = usage {
            if let Ok(mut slot) = self.usage.lock() {
                *slot = Some(usage);
            }
        }
        Ok(steps)
    }

    /// `ureq` 错误 → `CoreError`。
    fn map_error(&self, url: &str, error: ureq::Error) -> CoreError {
        match error {
            ureq::Error::Status(code, response) => {
                let preview = error_body_preview(response);
                self.io_error(url, format!("HTTP {code}: {preview}"))
            }
            ureq::Error::Transport(transport) => {
                let detail = transport.message().unwrap_or("无附加信息");
                self.io_error(url, format!("{}: {detail}", transport.kind()))
            }
        }
    }

    /// 造一个 `CoreError::Io`（`path` = 请求 URL）。
    fn io_error(&self, url: &str, message: String) -> CoreError {
        CoreError::Io {
            path: PathBuf::from(url),
            source: std::io::Error::other(message),
        }
    }
}

/// 错误响应体的可读片段（**脱敏后**截断）。
///
/// 先脱敏再截断：反向顺序会把密钥截成半截而躲过脱敏规则。
fn error_body_preview(response: ureq::Response) -> String {
    let mut raw = Vec::new();
    let _ = response
        .into_reader()
        .take(ERROR_BODY_CAP as u64)
        .read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let redacted = Redactor::default_v0().redact_str(&text);
    let mut preview: String = redacted.chars().take(ERROR_BODY_PREVIEW_CHARS).collect();
    if redacted.chars().count() > ERROR_BODY_PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

impl Provider for OpenAiProvider {
    /// 只送本轮输入（无 system prompt、无历史）——`TurnContext` 缺的字段用空值补。
    ///
    /// ⚠️ trait 的 `next_steps` **没有默认实现**（必须实现），故本适配器
    /// 显式实现它，而不是走 trait 默认。
    fn next_steps(
        &self,
        session_id: Uuid,
        user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let context = TurnContext {
            session_id,
            mode: Mode::Quick,
            system_prompt: String::new(),
            memory_digest: None,
            history: Vec::new(),
            user_text: user_text.to_string(),
        };
        self.call(&context)
    }

    /// 全量上下文（含 system prompt / 历史）⇒ 真实实现，转调 options 版本。
    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.next_steps_with_options(context, &StepOptions::default())
    }

    /// 全量上下文 + 请求选项 ⇒ 真实实现。
    ///
    /// ⚠️ `options.tool_choice` **不送出**：本卡不送 `tools` 字段
    /// （见模块文档），没有可禁用的工具 ⇒ 两种取值对上游等价。
    fn next_steps_with_options(
        &self,
        context: &TurnContext,
        _options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.call(context)
    }
}
