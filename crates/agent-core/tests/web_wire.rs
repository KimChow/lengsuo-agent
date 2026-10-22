//! `web.fetch` 接线验收：spec 清单、抓取失败显式报错、成功路径内容、脱敏、大小上限。
//!
//! 本测试**不经过注册表**（注册由汇总卡在 `tool.rs` 负责），直接调本卡产出的
//! `tool_specs()` 与适配函数；抓取端口用本地 mock，**不依赖外网**（CI 无网仍可跑）。
//!
//! 挂载方式：`tool_web.rs` 与 `web_adapter.rs` **均已在 crate 本体**
//! （`lib.rs` 的 `pub mod tool_web;` / `pub mod web_adapter;`）⇒ 这里直接引
//! `agent_core::…`，验的是**本体**而不是影子副本。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use agent_core::r#loop::ToolExecutor;
use agent_core::tool::ToolKind;
use agent_core::tool_web::tool_specs;
use agent_core::web_adapter::{FetcherPort, WebFetchAdapter, WebFetchPage, WebFetchPort};
use agent_tools_exec::{FetchResult, WebFetcher};
use serde_json::json;

/// 本地 mock 端口：按 URL 返回预置页面或预置错误（**不触网**）。
#[derive(Default)]
struct MockPort {
    pages: BTreeMap<String, WebFetchPage>,
    failures: BTreeMap<String, String>,
}

impl MockPort {
    fn with_page(mut self, url: &str, page: WebFetchPage) -> Self {
        self.pages.insert(url.to_string(), page);
        self
    }

    fn with_failure(mut self, url: &str, message: &str) -> Self {
        self.failures.insert(url.to_string(), message.to_string());
        self
    }
}

impl WebFetchPort for MockPort {
    fn fetch(&self, url: &str) -> Result<WebFetchPage, String> {
        if let Some(message) = self.failures.get(url) {
            return Err(message.clone());
        }
        self.pages
            .get(url)
            .cloned()
            .ok_or_else(|| format!("IO 失败: 域名解析失败: {url}"))
    }
}

/// 造一个只有一页的适配器。
fn adapter_with(page: WebFetchPage) -> WebFetchAdapter {
    let url = "https://example.test/page";
    WebFetchAdapter::new(Arc::new(MockPort::default().with_page(url, page)))
}

/// 造一页正文。
fn page(text: &str) -> WebFetchPage {
    WebFetchPage {
        final_url: "https://example.test/page".to_string(),
        title: Some("本地测试页".to_string()),
        text: text.to_string(),
    }
}

/// 1) spec 清单含 `web.fetch`，且类别/敏感标记/超时与冻结规格逐字一致。
#[test]
fn specs_declare_web_fetch_with_frozen_attributes() {
    let specs = tool_specs();
    let (name, description, kind, sensitive, timeout_ms) = specs
        .iter()
        .find(|(n, ..)| *n == "web.fetch")
        .copied()
        .expect("spec 清单应含 web.fetch");

    assert_eq!(name, "web.fetch");
    assert_eq!(kind, ToolKind::Read, "只读网络访问");
    assert!(
        sensitive,
        "URL 可能带 token/签名参数 ⇒ 参数须过敏感动作扫描"
    );
    assert_eq!(timeout_ms, 30_000, "网络请求超时");

    // 描述必须写清「做什么 + 失败怎么表现 + 能力边界」。
    for needle in ["http", "https", "失败", "上限", "脱敏"] {
        assert!(
            description.contains(needle),
            "描述缺「{needle}」：{description}"
        );
    }
    // 名字与模式白名单逐字一致（白名单已含 web.fetch，名字漂移会被权限门拒掉）。
    assert!(
        agent_core::mode::READONLY_TOOLS.contains(&name),
        "web.fetch 应在只读白名单里"
    );
}

/// 2) 抓取失败 ⇒ 显式错误（不是空串、不是 panic）。
#[test]
fn fetch_failures_surface_as_explicit_errors() {
    let adapter = WebFetchAdapter::new(Arc::new(
        MockPort::default().with_failure("https://nope.invalid/x", "IO 失败: 域名解析失败"),
    ));

    // 端口失败（域名不可达）⇒ 显式错误。
    let err = adapter
        .execute("web.fetch", &json!({ "url": "https://nope.invalid/x" }))
        .expect_err("域名不可达必须失败");
    assert!(!err.is_empty(), "错误不得是空串");
    assert!(err.contains("web.fetch"), "错误要点明工具：{err}");

    // URL 形状不合法 ⇒ 显式错误。
    for bad in ["", "not a url", "file:///etc/passwd", "gopher://x/y"] {
        let err = adapter
            .execute("web.fetch", &json!({ "url": bad }))
            .expect_err(&format!("{bad:?} 必须失败"));
        assert!(!err.is_empty(), "{bad:?} 的错误不得是空串");
    }

    // 缺 `url` 参数 ⇒ 显式错误。
    let err = adapter
        .execute("web.fetch", &json!({}))
        .expect_err("缺 url 必须失败");
    assert!(!err.is_empty());

    // 别的工具名 ⇒ 拒绝（不越界认领）。
    let err = adapter
        .execute("web.other", &json!({ "url": "https://example.test/" }))
        .expect_err("只认 web.fetch");
    assert!(err.contains("web.other"), "错误要点明工具名：{err}");
}

/// 3) 抓取成功路径：内容正确（本地 mock，不触网）。
#[test]
fn successful_fetch_returns_page_content() {
    let adapter = adapter_with(page("# 标题一\n\n正文内容"));
    let value = adapter
        .execute("web.fetch", &json!({ "url": "https://example.test/page" }))
        .expect("抓取应成功");

    assert_eq!(value["url"], json!("https://example.test/page"));
    assert_eq!(value["title"], json!("本地测试页"));
    assert_eq!(value["text"], json!("# 标题一\n\n正文内容"));
    assert_eq!(value["truncated"], json!(false));
    assert!(value["note"].is_null(), "未截断时不应有截断说明");
}

/// 4) 返回内容含凭据样式串 ⇒ 明文不在返回里（脱敏生效）。
#[test]
fn credential_shaped_content_is_redacted() {
    let token = "ghp_AbCdEf1234567890AbCd";
    let api_key = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let body = format!(
        "token: {token}\napi_key={api_key}\n\
         -----BEGIN PRIVATE KEY-----\nabcdef\n-----END PRIVATE KEY-----"
    );
    let adapter = adapter_with(page(&body));
    let value = adapter
        .execute("web.fetch", &json!({ "url": "https://example.test/page" }))
        .expect("抓取应成功");
    let rendered = value.to_string();

    assert!(!rendered.contains(token), "明文 token 不应出现在返回里");
    assert!(!rendered.contains(api_key), "明文 api_key 不应出现在返回里");
    assert!(
        !rendered.contains("BEGIN PRIVATE KEY"),
        "私钥块不应出现在返回里"
    );
    assert!(rendered.contains("***"), "应有脱敏标记：{rendered}");
}

/// 5) 大小上限：超限显式截断并注明，不静默丢内容。
#[test]
fn oversized_content_is_capped_and_marked() {
    let adapter = adapter_with(page(&"x".repeat(10_000))).with_max_bytes(1_000);
    let value = adapter
        .execute("web.fetch", &json!({ "url": "https://example.test/page" }))
        .expect("抓取应成功");

    assert_eq!(value["truncated"], json!(true), "超限必须标注截断");
    let text = value["text"].as_str().expect("text 必须是字符串");
    assert_eq!(text.chars().count(), 1_000, "正文应被截到字节上限内");
    assert!(
        value["note"].as_str().is_some_and(|n| !n.is_empty()),
        "截断必须显式说明：{value}"
    );
}

/// 6) 多字节字符不被切碎（截断落在字符边界上）。
#[test]
fn truncation_never_splits_a_multibyte_char() {
    // 每个字符 3 字节 ⇒ 上限 10 落在第 4 个字符中间。
    let adapter = adapter_with(page(&"中".repeat(20))).with_max_bytes(10);
    let value = adapter
        .execute("web.fetch", &json!({ "url": "https://example.test/page" }))
        .expect("抓取应成功");
    let text = value["text"].as_str().expect("text 必须是字符串");
    assert!(text.chars().all(|c| c == '中'), "不得出现半个字符");
    assert!(text.len() <= 10, "字节数必须在上限内，实得 {}", text.len());
    assert_eq!(value["truncated"], json!(true));
}

/// 7) 端口实现：`agent-tools-exec` 的抓取结果映射成页面，失败原样显式报错。
#[test]
fn fetcher_port_maps_fetch_results_and_errors() {
    let port = FetcherPort::new(|_fetcher: &WebFetcher, url: &str| {
        if url.contains("bad") {
            return Err("HTTP 404: https://example.test/bad".to_string());
        }
        Ok(FetchResult {
            final_url: url.to_string(),
            title: Some("标题".to_string()),
            text_markdown: "正文".to_string(),
        })
    });

    // 端口内建的抓取器按冻结规格配置（30s 超时 / 传输层字节上限）。
    assert_eq!(port.fetcher().timeout(), Duration::from_millis(30_000));
    assert_eq!(port.fetcher().max_bytes(), 4 * 256 * 1024);
    // 传输层上限必须宽于上下文上限，否则传输截断会变成静默截断。
    assert!(port.fetcher().max_bytes() > 256 * 1024);

    let page = port.fetch("https://example.test/ok").expect("映射应成功");
    assert_eq!(page.final_url, "https://example.test/ok");
    assert_eq!(page.title.as_deref(), Some("标题"));
    assert_eq!(page.text, "正文");

    let err = port
        .fetch("https://example.test/bad")
        .expect_err("非 2xx 必须失败");
    assert!(err.contains("404"), "错误应带状态码：{err}");
}

/// 7) 字节上限：默认取冻结规格，可显式改写。
#[test]
fn byte_cap_defaults_to_the_frozen_spec() {
    let adapter = adapter_with(page("短"));
    assert_eq!(adapter.max_bytes(), 256 * 1024, "默认上限应与 spec 同值");
    assert_eq!(adapter.with_max_bytes(64).max_bytes(), 64);
}
