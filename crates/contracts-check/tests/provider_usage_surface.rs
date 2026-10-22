//! `Provider` 用量面的契约登记：`last_usage` 方法 + `TokenUsage` 字段与计费口径。
//!
//! # 只守关系，不锁数量/取值
//!
//! - `Provider` trait 必须带 `last_usage`（带默认实现，既有实现方零改动）；
//! - `TokenUsage` 必须保留 4 个 OpenAI 规范字段名；
//! - `billed_tokens()` 必须保持「输入 − 缓存命中 + 输出」的复用点；
//! - `OpenAiProvider` 必须在其 `impl Provider` 里实现 `last_usage`。
//!
//! ⛔ 刻意**不写**「方法有几个」「字段有几个」这类锁规模的断言——它们会在正当
//! 扩充时必红。源码用 `include_str!` 在**编译期**绑定，改动被扫文件即触发重编。
//!
//! # 护栏自证
//!
//! 所有断言都走同一个 [`assert_contains`]。`guard_detects_source_drift` 用一份
//! 合成漂移样本调它，`catch_unwind` **必须捕获到 panic**：捕获不到 = 护栏恒绿
//! = 本卡白做。

use std::any::Any;

/// 被扫描的 provider 面源码（编译期绑定）。
const PROVIDER_SOURCE: &str = include_str!("../../agent-core/src/provider.rs");
/// 被扫描的 OpenAI 适配器源码（编译期绑定）。
const OPENAI_SOURCE: &str = include_str!("../../agent-openai/src/lib.rs");

/// 归一化空白，使跨行写法与单行写法等价（不引入正则依赖）。
fn normalized(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// **唯一断言出口**：源码里必须出现某段关系；缺失即 panic（文案带标签）。
fn assert_contains(label: &str, haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "{label} 与契约登记漂移：源码里找不到 `{needle}`"
    );
}

/// 取 `marker` 之后的源码片段（到 `end` 标记为止）；找不到 `marker` 即红。
fn section<'a>(label: &str, source: &'a str, marker: &str, end: Option<&str>) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("{label} 漂移：源码里找不到 `{marker}`"));
    let tail = &source[start..];
    match end.and_then(|end| tail.find(end)) {
        Some(offset) => &tail[..offset],
        None => tail,
    }
}

/// `Provider` trait 本体（到 `EchoProvider` 文档之前）。
fn provider_trait_body(source: &str) -> &str {
    section(
        "Provider trait",
        source,
        "pub trait Provider: Send + Sync {",
        Some("/// 测试用回显 provider"),
    )
}

/// `TokenUsage` 结构体本体（到 `impl TokenUsage` 之前）。
fn token_usage_struct_body(source: &str) -> &str {
    section(
        "TokenUsage struct",
        source,
        "pub struct TokenUsage {",
        Some("impl TokenUsage {"),
    )
}

/// `impl TokenUsage` 本体（到下一项文档之前）。
fn token_usage_impl_body(source: &str) -> &str {
    section(
        "impl TokenUsage",
        source,
        "impl TokenUsage {",
        Some("/// 一次回合产出的一个步骤"),
    )
}

/// `impl Provider for OpenAiProvider` 本体（文件内本 impl 之后无其它 impl）。
fn openai_provider_impl_body(source: &str) -> &str {
    section(
        "impl Provider for OpenAiProvider",
        source,
        "impl Provider for OpenAiProvider {",
        None,
    )
}

/// ① `Provider` trait 确实有 `last_usage`（默认实现 ⇒ 不破既有实现方）。
#[test]
fn provider_trait_declares_last_usage() {
    assert_contains(
        "Provider::last_usage",
        &normalized(provider_trait_body(PROVIDER_SOURCE)),
        "fn last_usage(&self) -> Option<TokenUsage>",
    );
}

/// ② `TokenUsage` 的 4 个字段名必须保留（对齐 OpenAI usage 规范）。
#[test]
fn token_usage_keeps_the_four_openai_field_names() {
    let body = normalized(token_usage_struct_body(PROVIDER_SOURCE));
    for field in [
        "pub prompt_tokens: u64",
        "pub completion_tokens: u64",
        "pub total_tokens: u64",
        "pub cached_tokens: u64",
    ] {
        assert_contains("TokenUsage 字段", &body, field);
    }
}

/// ③ `billed_tokens()` 口径必须保持「输入 − 缓存命中 + 输出」。
#[test]
fn billed_tokens_keeps_the_charged_caliber() {
    let body = normalized(token_usage_impl_body(PROVIDER_SOURCE));
    assert_contains(
        "billed_tokens 方法",
        &body,
        "pub fn billed_tokens(&self) -> u64",
    );
    assert_contains(
        "billed_tokens 口径",
        &body,
        "self.prompt_tokens.saturating_sub(self.cached_tokens) + self.completion_tokens",
    );
}

/// ④ `OpenAiProvider` 在 `impl Provider` 里实现了 `last_usage`。
#[test]
fn openai_provider_implements_last_usage() {
    assert_contains(
        "OpenAiProvider::last_usage",
        &normalized(openai_provider_impl_body(OPENAI_SOURCE)),
        "fn last_usage(&self) -> Option<TokenUsage>",
    );
}

/// 取出 panic payload 里的文本（`catch_unwind` 之后自证用）。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// 护栏自证：合成一份**缺少** `last_usage` 的漂移样本，`assert_contains` 必须红。
#[test]
fn guard_detects_source_drift() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        assert_contains(
            "drift-probe",
            &normalized("impl Provider for Synthetic {}"),
            "fn last_usage(&self) -> Option<TokenUsage>",
        );
    });
    std::panic::set_hook(previous_hook);

    let payload = outcome.expect_err("护栏必须能红：缺失的源码关系竟未触发 panic");
    let message = panic_message(payload.as_ref());
    assert!(
        message.contains("drift-probe"),
        "panic 文案应带上断言标签，实际 {message}"
    );
}
