//! 鉴权与信封形态（`AGENT-KICKOFF.md` 第 22 行「信封」+ 第 25 行「鉴权」）。
use contracts_check::{ENVELOPE_CODES, FROZEN_ENVELOPE_CODES};
use serde_json::json;

mod support;
use support::{read_handshake, request, request_with_headers, start_gateway};

/// 缺 `X-Gateway-Token` 或 token 错 → HTTP 403 + 信封 403；正确 token → 不是 403。
#[tokio::test]
async fn missing_or_wrong_token_is_rejected_with_403() {
    let (g, dir) = start_gateway().await;
    let real = read_handshake(dir.path())["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // 完全不发鉴权头。
    let (status, env) = request_with_headers(g.port(), None, "sys:ping", json!({})).await;
    assert_eq!(status, 403, "缺 token 必须 HTTP 403: {env}");
    assert_eq!(env["code"], 403);
    assert_eq!(env["data"], serde_json::Value::Null);

    // token 错。
    let (status, env) = request(g.port(), "wrong-token", "sys:ping", json!({})).await;
    assert_eq!(status, 403, "错误 token 必须 HTTP 403: {env}");
    assert_eq!(env["code"], 403);

    // 长度相同但内容不同的 token（走等长比较分支）。
    let mut flipped = real.clone();
    let last = flipped.pop().unwrap();
    flipped.push(if last == '0' { '1' } else { '0' });
    let (_, env) = request(g.port(), &flipped, "sys:ping", json!({})).await;
    assert_eq!(env["code"], 403, "等长错 token 也必须拒: {env}");

    // 正确 token → 不是 403。
    let (status, env) = request(g.port(), &real, "sys:ping", json!({})).await;
    assert_ne!(status, 403);
    assert_ne!(env["code"], 403);

    g.shutdown().await.unwrap();
}

/// 鉴权先于动作分派：未知动作 + 错 token 也必须是 403，不能泄漏「动作是否存在」。
#[tokio::test]
async fn auth_precedes_action_dispatch() {
    let (g, _dir) = start_gateway().await;
    let (_, env) = request(g.port(), "wrong", "nonexistent:action", json!({})).await;
    assert_eq!(env["code"], 403, "鉴权必须挡在 404 之前: {env}");
    g.shutdown().await.unwrap();
}

/// 成功响应必含 `code/msg/data` 三键；`code==0` 时 `data` 非 null。
///
/// `AGENT-KICKOFF.md` 第 22 行只规定信封三键与 `code==0` 成功，**没有**为
/// `code!=0` 的 `data` 规定非空；实现里失败分支一律 `data: null`，故只对
/// 成功分支断言非 null，不发明更宽的规则。
#[tokio::test]
async fn success_envelope_has_three_keys_and_non_null_data() {
    let (g, _dir) = start_gateway().await;
    let (_, env) = request(g.port(), g.token(), "sys:ping", json!({})).await;

    assert_eq!(env["code"], 0);
    assert!(env.get("code").is_some(), "缺 code 键: {env}");
    assert!(env.get("msg").is_some(), "缺 msg 键: {env}");
    assert!(env.get("data").is_some(), "缺 data 键: {env}");
    assert!(!env["data"].is_null(), "code==0 时 data 不得为 null: {env}");

    g.shutdown().await.unwrap();
}

/// 失败分支同样带三键，且 `data` 为空；错误码必须落在**登记码段**内。
///
/// 登记码段 = `AGENT-KICKOFF.md` 第 22 行**冻结**的 4 码（`0/403/404/408`）
/// + 实现**额外发出**、按实现为准补登记的 4 码（`400/409/500/502`）。
/// 此前这条断言给 `400` 开了「特赦开口」（`code == 400 || …`），等于把
/// 「实现发了未登记的码」变成合法——400 现在是 `ENVELOPE_CODES` 的正式成员，
/// 特赦已删。
///
/// probe 覆盖到实现实际会发的**四类失败码**：
/// - `404` 未注册动作（`rpc.rs` 的 `unknown action`）；
/// - `400` payload 非法（`rpc.rs` 的 `invalid payload`）；
/// - `409` 会话已结束、拒绝新输入（`rpc.rs` 的 `session closed` ←
///   `CoreError::SessionClosed`；触发路径：先用 `agent-core` 的
///   `Runtime::finish_session` 把会话置 `Done`，再对该会话发 `session:send`，
///   `agent-core/src/runtime/internals.rs` 的 `push_message` 拒绝已结束会话）；
/// - `500` 内部错误兜底（`rpc.rs` 的 `operation failed` ← `CoreError` 兜底分支
///   `_ => (500, "operation failed")`；触发路径：给运行时注入
///   `agent_core::provider::UnimplementedProvider`——它对任何输入都返回
///   `CoreError::Unsupported`，该变体落在 `core()` 映射表的**兜底分支**上 ⇒
///   信封 500）。
///
/// ⛔ `502`（ACP 转发传输失败）**不做运行时 probe**：它要求挂上 ACP host 句柄
/// 并让对端传输失败（子进程/传输层），属 ACP 侧用例、成本高且易 flaky；
/// `502` 由 `tests/envelope_code_surface.rs` 的**源码扫描断言**覆盖。
#[tokio::test]
async fn failure_envelope_uses_registered_codes_only() {
    let (g, _dir) = start_gateway().await;

    // 409 与 500 的 probe 需要**另起**运行时（一个要已结束的会话，一个要会
    // 失败的 provider），故与常规 probe 分开跑；各自用独立工作目录，避免两个
    // 运行时共用同一个事件日志目录。
    let probes = [
        ("nonexistent:action", json!({})),             // 404
        ("session:get", json!({})),                    // 400
        ("config:get", json!({"key": "no-such-key"})), // 400
    ];
    for (action, payload) in probes {
        let (_, env) = request(g.port(), g.token(), action, payload).await;
        assert!(env.get("code").is_some(), "缺 code 键: {env}");
        assert!(env.get("msg").is_some(), "缺 msg 键: {env}");
        assert!(env.get("data").is_some(), "缺 data 键: {env}");
        assert!(env["data"].is_null(), "失败分支 data 应为 null: {env}");
        let code = env["code"].as_u64().unwrap() as u16;
        assert!(
            ENVELOPE_CODES.contains(&code),
            "错误码 {code} 不在登记码段 {ENVELOPE_CODES:?}: {env}"
        );
    }

    // —— 409：会话已结束 → CoreError::SessionClosed → 409 ——
    let closed_dir = tempfile::tempdir().unwrap();
    let rt = support::runtime(closed_dir.path());
    let session = rt
        .create_session(&agent_core::runtime::CreateSession::plain(
            "closed",
            agent_core::schema::Isolation::local_default(),
        ))
        .unwrap();
    rt.finish_session(session.id).unwrap();
    let g409 = agent_gateway::GatewayHandle::start(
        rt,
        agent_gateway::GatewayConfig::new(
            closed_dir.path().join("gateway-409.json"),
            agent_core::schema::Isolation::local_default(),
        ),
    )
    .await
    .unwrap();
    let (_, env) = request(
        g409.port(),
        g409.token(),
        "session:send",
        json!({"id": session.id.to_string(), "text": "late"}),
    )
    .await;
    let code = env["code"].as_u64().unwrap() as u16;
    assert_eq!(code, 409, "已结束会话再发消息必须 409: {env}");
    assert_eq!(env["msg"], "session closed");
    assert!(ENVELOPE_CODES.contains(&code));
    g409.shutdown().await.unwrap();

    // —— 500：内部错误兜底 → CoreError::Unsupported → 500 ——
    // provider 对任何输入都报未实现 ⇒ 回合失败 ⇒ 兜底分支 ⇒ 500。
    let err_dir = tempfile::tempdir().unwrap();
    let rt_err = agent_core::runtime::Runtime::builder()
        .workdir(err_dir.path())
        .provider(std::sync::Arc::new(
            agent_core::provider::UnimplementedProvider,
        ))
        .build()
        .unwrap();
    let g500 = agent_gateway::GatewayHandle::start(
        rt_err,
        agent_gateway::GatewayConfig::new(
            err_dir.path().join("gateway-500.json"),
            agent_core::schema::Isolation::local_default(),
        ),
    )
    .await
    .unwrap();
    let (_, created) = request(g500.port(), g500.token(), "session:create", json!({})).await;
    let id = created["data"]["id"].clone();
    let (_, env) = request(
        g500.port(),
        g500.token(),
        "session:send",
        json!({"id": id, "text": "boom"}),
    )
    .await;
    let code = env["code"].as_u64().unwrap() as u16;
    assert_eq!(code, 500, "provider 内部错误必须落 500 兜底: {env}");
    assert_eq!(env["msg"], "operation failed");
    assert!(ENVELOPE_CODES.contains(&code));
    g500.shutdown().await.unwrap();

    g.shutdown().await.unwrap();
}

/// 登记码面自身的两条关系不变量（**不锁数量、不锁取值**）。
///
/// - **冻结 ⊆ 登记**：`AGENT-KICKOFF.md` 第 22 行冻结的 4 码必须**逐项**在
///   `ENVELOPE_CODES` 里（不得缩水）；
/// - **登记无重复**：`ENVELOPE_CODES` 里不得出现重复项。
///
/// ⚠️ 这条断言此前写成 `assert_eq!(ENVELOPE_CODES, &[0, 403, 404, 408])`——
/// **锁死了取值**，下一次正当扩充码面（实现多发一个码）时必红，而它又不在
/// 任何后续卡的 touches 里 ⇒ 连环假失败。故改为关系口径。
#[test]
fn frozen_envelope_codes_are_registered_without_duplicates() {
    for code in FROZEN_ENVELOPE_CODES {
        assert!(
            ENVELOPE_CODES.contains(code),
            "第 22 行冻结的码 {code} 不在 ENVELOPE_CODES 里（冻结面不得缩水）"
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for code in ENVELOPE_CODES {
        assert!(seen.insert(*code), "ENVELOPE_CODES 里码 {code} 重复登记");
    }
}
