//! `web.fetch` 的执行适配：把工具调用参数翻译成一次真实的网页抓取。
//!
//! ## 分层与 tokio 边界
//!
//! `agent-tools-exec` 的 [`agent_tools_exec::WebFetcher::fetch_to_markdown`] 是
//! `async fn`，而本 crate 的工具步是**同步**的（[`crate::r#loop::ToolExecutor`]），
//! 且本 crate **不得依赖 tokio**。因此抓取能力经**同步端口** [`WebFetchPort`] 接入：
//! 持 tokio 的宿主（网关）实现该端口，内部驱动 [`agent_tools_exec::WebFetcher`]。
//! 这与既有的 [`crate::host::McpToolDispatcher`] 是同一套分层（能力持有与调用分离）。
//!
//! [`FetcherPort`] 是端口的**现成实现**：它持有真的 [`agent_tools_exec::WebFetcher`]，
//! 把「await 抓取」这一步交给宿主提供的驱动闭包（宿主有 tokio，可自行决定
//! `block_on` / `spawn_blocking` / 线程池投递）。
//!
//! ## 返回内容的三道处理
//!
//! 顺序固定：
//! 1. **大小上限**：正文按 [`WEB_FETCH_MAX_BYTES`]（与抓取侧的字节上限同值）截断，
//!    **显式标注** `truncated` 并在 `note` 里说明——不静默丢内容；
//! 2. **脱敏**：整个结果过 [`crate::redact::Redactor`]——正文、最终 URL、标题里的
//!    token/私钥样式串都不会以明文进入模型上下文；
//! 3. **失败显式化**：抓取失败、URL 形状不合法、协议不是 http/https，一律 `Err`
//!    并说明原因——不返回空串、不 panic。
//!
//! 失败文案**不回显 URL 原文**（URL 可能带签名参数），只说协议与形状。

use std::sync::Arc;
use std::time::Duration;

use agent_tools_exec::{FetchResult, WebFetcher};
use serde_json::{json, Value};

use crate::r#loop::ToolExecutor;
use crate::redact::Redactor;
use crate::tool_web::{
    WEB_FETCH_BODY_BYTES, WEB_FETCH_MAX_BYTES, WEB_FETCH_TIMEOUT_MS, WEB_FETCH_TOOL,
};

/// 一次抓取的结果（已转成纯文本的页面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFetchPage {
    /// 跟随重定向后的最终地址。
    pub final_url: String,
    /// `<title>` 文本；页面没有标题时为 `None`。
    pub title: Option<String>,
    /// 纯文本正文。
    pub text: String,
}

/// 抓取端口：同步端口，宿主实现。
///
/// 与 [`crate::host::McpToolDispatcher`] 同构——本 crate 不碰 tokio，异步抓取由
/// 持运行时的宿主在此端口内部驱动。
pub trait WebFetchPort: Send + Sync {
    /// 抓取 `url` 的正文；失败须给出**可读原因**（不含凭据）。
    fn fetch(&self, url: &str) -> Result<WebFetchPage, String>;
}

/// 抓取端口的现成实现：驱动真的 [`agent_tools_exec::WebFetcher`]。
///
/// 驱动闭包负责「把 async 抓取跑起来」——它在宿主（有 tokio）里创建，因此闭包可以
/// 自行 `block_on` 或投递到工作线程；本 crate 不替宿主做这个决定。
pub struct FetcherPort<F> {
    fetcher: WebFetcher,
    driver: F,
}

impl<F> FetcherPort<F>
where
    F: Fn(&WebFetcher, &str) -> Result<FetchResult, String> + Send + Sync,
{
    /// 用默认抓取器构造：超时取冻结规格（[`WEB_FETCH_TIMEOUT_MS`]），
    /// 传输层字节上限取 [`WEB_FETCH_BODY_BYTES`]（宽于上下文上限，理由见其文档）。
    pub fn new(driver: F) -> Self {
        Self::with_fetcher(
            WebFetcher::new(
                Duration::from_millis(WEB_FETCH_TIMEOUT_MS),
                WEB_FETCH_BODY_BYTES,
            ),
            driver,
        )
    }

    /// 用宿主自备的抓取器构造（例如换 User-Agent）。
    ///
    /// 抓取器的字节上限应不宽于 [`WEB_FETCH_MAX_BYTES`]：超宽的部分在适配层仍会被
    /// 截断（返回给模型的正文以适配层上限为准）。
    pub fn with_fetcher(fetcher: WebFetcher, driver: F) -> Self {
        FetcherPort { fetcher, driver }
    }

    /// 生效的抓取器（供宿主查超时/字节上限）。
    pub fn fetcher(&self) -> &WebFetcher {
        &self.fetcher
    }
}

impl<F> WebFetchPort for FetcherPort<F>
where
    F: Fn(&WebFetcher, &str) -> Result<FetchResult, String> + Send + Sync,
{
    fn fetch(&self, url: &str) -> Result<WebFetchPage, String> {
        let result = (self.driver)(&self.fetcher, url)?;
        Ok(WebFetchPage {
            final_url: result.final_url,
            title: result.title,
            text: result.text_markdown,
        })
    }
}

/// `web.fetch` 的执行适配器。
pub struct WebFetchAdapter {
    port: Arc<dyn WebFetchPort>,
    /// 返回给模型的正文字节上限。
    max_bytes: usize,
    redactor: Redactor,
}

impl std::fmt::Debug for WebFetchAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebFetchAdapter")
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl WebFetchAdapter {
    /// 以抓取端口构造（字节上限取冻结规格）。
    pub fn new(port: Arc<dyn WebFetchPort>) -> Self {
        Self {
            port,
            max_bytes: WEB_FETCH_MAX_BYTES,
            redactor: Redactor::default_v0(),
        }
    }

    /// 改写返回正文的字节上限（抓取侧仍按端口自己的上限）。
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// 生效的字节上限。
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl ToolExecutor for WebFetchAdapter {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        if name != WEB_FETCH_TOOL {
            return Err(format!("不支持的工具: {name}"));
        }
        let url = parse_url_arg(args)?;
        check_http_scheme(&url)?;

        let page = self
            .port
            .fetch(&url)
            .map_err(|e| format!("{WEB_FETCH_TOOL} 抓取失败: {e}"))?;

        // 第一级：原始正文按上限截断（不静默丢内容，先记账）。
        let (text, truncated_raw) = cap_text(&page.text, self.max_bytes);
        let value = json!({
            "url": page.final_url,
            "title": page.title,
            "text": text,
            "truncated": truncated_raw,
            "note": truncated_raw.then(|| truncation_note(self.max_bytes, text.len())),
        });
        // 脱敏覆盖整个结果：正文之外，最终 URL 与标题也可能带凭据样式串。
        let mut redacted = self.redactor.redact_json(&value);
        // 第二级：脱敏后的正文再夹一次上限（私钥掩码比原文略长，夹一次才是硬上界）。
        if let Some(text) = redacted
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            let (text, truncated_final) = cap_text(&text, self.max_bytes);
            if truncated_final {
                redacted["text"] = Value::String(text.clone());
                redacted["truncated"] = Value::Bool(true);
                redacted["note"] = Value::String(truncation_note(self.max_bytes, text.len()));
            }
        }
        Ok(redacted)
    }
}

/// 截断说明：显式告诉模型「内容被截了」，并给出实际保留的字节数。
fn truncation_note(max_bytes: usize, kept: usize) -> String {
    format!("内容超过 {max_bytes} 字节上限，已显式截断（只保留前 {kept} 字节）")
}

/// 取 `url` 参数：必填、非空字符串。
fn parse_url_arg(args: &Value) -> Result<String, String> {
    let raw = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(bad_args)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_args());
    }
    Ok(trimmed.to_string())
}

fn bad_args() -> String {
    format!("{WEB_FETCH_TOOL} 参数不合法: 需要 {{ \"url\": 非空字符串 }}")
}

/// 只允许 http/https——与 [`agent_tools_exec::WebFetcher`] 的口径一致。
///
/// 失败文案只说协议形状，**不回显 URL 原文**（URL 可能带签名参数）。
fn check_http_scheme(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let Some(rest) = rest else {
        return Err(format!(
            "{WEB_FETCH_TOOL} 参数不合法: 只允许 http/https 地址"
        ));
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err(format!("{WEB_FETCH_TOOL} 参数不合法: 地址缺少主机名"));
    }
    Ok(())
}

/// 按字节上限截断正文（不切碎多字节字符）。
///
/// 返回 `(截断后的文本, 是否截断)`；未超限时原样返回。
fn cap_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}
