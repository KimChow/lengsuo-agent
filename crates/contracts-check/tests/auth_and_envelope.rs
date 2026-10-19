//! 鉴权与信封形态（`AGENT-KICKOFF.md` 第 22 行「信封」+ 第 25 行「鉴权」）。
use contracts_check::ENVELOPE_CODES;
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

/// 失败分支同样带三键，且 `data` 为空；错误码必须落在冻结码段内。
#[tokio::test]
async fn failure_envelope_uses_frozen_codes_only() {
    let (g, _dir) = start_gateway().await;
    let probes = [
        ("nonexistent:action", json!({})),             // 404
        ("session:get", json!({})),                    // 400（payload 错，非冻结码段但实现稳定）
        ("config:get", json!({"key": "no-such-key"})), // 400
    ];
    for (action, payload) in probes {
        let (_, env) = request(g.port(), g.token(), action, payload).await;
        assert!(env.get("code").is_some(), "缺 code 键: {env}");
        assert!(env.get("msg").is_some(), "缺 msg 键: {env}");
        assert!(env.get("data").is_some(), "缺 data 键: {env}");
        assert!(env["data"].is_null(), "失败分支 data 应为 null: {env}");
        let code = env["code"].as_u64().unwrap() as u16;
        // 400 是实现里的 payload 错，不在 §2 冻结码段；其余必须命中冻结码。
        assert!(
            code == 400 || ENVELOPE_CODES.contains(&code),
            "错误码 {code} 既不是 400 也不在冻结码段 {ENVELOPE_CODES:?}: {env}"
        );
    }
    g.shutdown().await.unwrap();
}

/// `ENVELOPE_CODES` 是冻结码段的单一来源：常量本身不得被悄悄改宽/改窄。
#[test]
fn frozen_envelope_codes_are_exactly_the_contract_set() {
    assert_eq!(ENVELOPE_CODES, &[0, 403, 404, 408]);
}
