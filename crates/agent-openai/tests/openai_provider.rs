//! OpenAI 适配器集成测试。
//!
//! ## 跑法
//!
//! ```sh
//! # 默认套件（离线：错误路径 + 响应体解析，不打网络）
//! cargo test -p agent-openai --locked
//!
//! # 真打网关（需网络可达）
//! cargo test -p agent-openai --locked -- --ignored --nocapture
//! ```
//!
//! 真打网关的用例一律 `#[ignore]`：CI 无网时**不得**让默认套件变红。
//! 证据见 `docs/openai-provider-status.md`。

use agent_core::provider::{Provider, ProviderStep, TokenUsage, TurnContext, Uuid};
use agent_core::schema::{Message, Mode, Role};
use agent_openai::{OpenAiConfig, OpenAiProvider};

/// 默认网关地址（与 `OpenAiConfig::default()` 一致）。
const GATEWAY: &str = "http://192.168.1.151:18080";

fn context(user_text: &str) -> TurnContext {
    TurnContext {
        session_id: Uuid::nil(),
        mode: Mode::Quick,
        system_prompt: "你是一个测试助手。只用中文简短回答。".to_string(),
        memory_digest: None,
        history: Vec::new(),
        user_text: user_text.to_string(),
    }
}

fn text_of(steps: &[ProviderStep]) -> Option<&str> {
    steps.iter().find_map(|step| match step {
        ProviderStep::Text(text) => Some(text.as_str()),
        _ => None,
    })
}

/// 错误路径：打一个**不存在的端口** ⇒ 必须返回 `Err`，不 panic、不静默成功。
///
/// 端口取 `1`（特权端口，无进程监听）⇒ 连接必被拒。
#[test]
fn unreachable_endpoint_returns_error_instead_of_panicking() {
    let provider = OpenAiProvider::new(OpenAiConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        model: "auto".to_string(),
        api_key: None,
        timeout_secs: 5,
    });
    let result = provider.next_steps_with_context(&context("你好"));
    let error = result.expect_err("打不通的端点必须返回 Err");
    let text = error.to_string();
    assert!(
        text.contains("127.0.0.1:1"),
        "错误信息应带请求端点，实际: {text}"
    );
}

/// 错误路径：非 2xx ⇒ `Err`，且错误信息带状态码 + 响应体片段。
///
/// 用**本地桩服务**（不依赖外网）：起一个只回一次 500 的极简 HTTP 服务。
#[test]
fn http_error_status_is_reported() {
    let base_url = spawn_stub_server(
        500,
        "Content-Type: application/json",
        r#"{"error":{"message":"stub failure","code":"stub"}}"#,
    );
    let provider = OpenAiProvider::new(OpenAiConfig {
        base_url,
        model: "auto".to_string(),
        api_key: None,
        timeout_secs: 10,
    });
    let error = provider
        .next_steps_with_context(&context("你好"))
        .expect_err("非 2xx 必须返回 Err");
    let text = error.to_string();
    assert!(text.contains("500"), "错误信息应带状态码，实际: {text}");
    assert!(
        text.contains("stub failure"),
        "错误信息应带响应体片段，实际: {text}"
    );
}

/// 起一个只回**一次**响应的极简 HTTP 桩服务，返回它的基地址。
///
/// 只用标准库（不新增依赖）：读掉请求头，回一个固定响应即关闭。
fn spawn_stub_server(status: u16, content_type: &str, body: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本地端口");
    let port = listener.local_addr().expect("读本地地址").port();
    let content_type = content_type.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("接受连接");
        let mut buffer = [0u8; 4096];
        let _ = stream.read(&mut buffer);
        let response = format!(
            "HTTP/1.1 {status} Stub\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    format!("http://127.0.0.1:{port}")
}

/// 真打网关：产出至少一条非空 `Text`，且 usage 的 `total_tokens > 0`。
#[test]
#[ignore = "需要网络可达网关；手动跑：cargo test -p agent-openai -- --ignored"]
fn live_gateway_returns_text_and_usage() {
    let provider = OpenAiProvider::new(OpenAiConfig {
        base_url: GATEWAY.to_string(),
        model: "auto".to_string(),
        api_key: None,
        timeout_secs: 60,
    });
    let steps = provider
        .next_steps_with_context(&context("用一句话回答：1+1 等于几？"))
        .expect("真打网关应成功");

    let text = text_of(&steps).expect("应产出至少一个 Text 步骤");
    assert!(!text.trim().is_empty(), "Text 内容不应为空");

    let usage: TokenUsage = provider
        .take_usage()
        .expect("真打网关后 take_usage() 应返回 Some");
    assert!(
        usage.total_tokens > 0,
        "usage.total_tokens 应 > 0，实际: {usage:?}"
    );
    eprintln!("实调结果: text={text:?} usage={usage:?}");
}

/// 真打网关：多角色历史按 OpenAI 规范逐条映射（system/user/assistant）。
#[test]
#[ignore = "需要网络可达网关；手动跑：cargo test -p agent-openai -- --ignored"]
fn live_gateway_accepts_multi_role_history() {
    let provider = OpenAiProvider::new(OpenAiConfig {
        base_url: GATEWAY.to_string(),
        model: "auto".to_string(),
        api_key: None,
        timeout_secs: 60,
    });
    let mut context = context("我叫什么名字？");
    context.history = vec![
        Message {
            role: Role::User,
            content: "我叫小明".to_string(),
            tool: None,
            ts_ms: 0,
        },
        Message {
            role: Role::Assistant,
            content: "你好小明".to_string(),
            tool: None,
            ts_ms: 0,
        },
    ];
    let steps = provider
        .next_steps_with_context(&context)
        .expect("真打网关应成功");
    let text = text_of(&steps).expect("应产出 Text 步骤");
    eprintln!("实调结果: text={text:?}");
    assert!(!text.trim().is_empty(), "Text 内容不应为空");
}
