//! 契约 5 + 宿主注入面：明文**不出现在**任何事件与日志中。
//!
//! 覆盖：工具回显的明文被换回引用形态、脱敏器作为第二道防线、
//! 宿主注入面（`HostServices`）接受本仓解析口、错误文案跨层不失真。
use agent_core::credentials::cred_params::{
    collect_cred_refs, resolve_cred_params, scrub_plaintexts, CredentialResolvingExecutor,
};
use agent_core::credentials::{parse_ref_id, CredentialError, StoreCredentialResolver};
use agent_core::host::{CredentialResolver, HostServices, NoopCredentialResolver};
use agent_core::ToolExecutor;
use serde_json::json;
use std::sync::Arc;

use super::support::*;

// ---------------- 契约 5：明文不出现在事件与日志中 ----------------

/// 工具结果回显了明文 ⇒ 换回引用形态（明文不进事件流/历史）。
#[test]
fn echoed_plaintext_is_scrubbed_back_to_ref() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "echoed-secret");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);
    let executor = Arc::new(EchoExecutor::default());
    let wrapper = CredentialResolvingExecutor::new(
        SharedEcho(executor.clone()),
        Arc::new(resolver) as Arc<dyn CredentialResolver>,
    );

    let token = cred_ref_of(&reference);
    let result = wrapper
        .execute("http.fetch", &json!({ "token": token.clone() }))
        .expect("执行成功");

    let rendered = result.to_string();
    assert!(
        !rendered.contains("echoed-secret"),
        "明文不得出现在工具结果里: {rendered}"
    );
    assert!(rendered.contains(&token), "结果里应是引用形态: {rendered}");
    // 工具确实收到了明文（执行面拿到真值）。
    assert_eq!(
        executor.calls()[0].1["token"],
        json!("echoed-secret"),
        "工具必须收到明文"
    );
}

/// 脱敏层是第二道防线：事件面过一遍脱敏器后仍无明文。
#[test]
fn redactor_is_second_line_of_defense() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "token-abcdefghijklmnop");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);
    let token = cred_ref_of(&reference);

    // 事件面：只放引用形态（运行时的既有口径）。
    // 注意键名：`token` 属脱敏器的敏感键名族，其值整体被替换 ⇒ 事件面连引用
    // 形态都不回显（比「只出现引用」更严）。用非敏感键名验证引用形态可见。
    let event_payload = json!({ "name": "http.fetch", "args": { "ref": token } });
    let redacted = agent_core::Redactor::default_v0().redact_json(&event_payload);
    let rendered = redacted.to_string();
    assert!(!rendered.contains("token-abcdefghijklmnop"));
    assert!(
        rendered.contains("cred_ref://"),
        "非敏感键名下引用形态应当可见: {rendered}"
    );

    // 敏感键名下：值整体替换（连引用都不回显），明文更不可能出现。
    let sensitive =
        agent_core::Redactor::default_v0().redact_json(&json!({ "token": token.clone() }));
    assert!(!sensitive.to_string().contains("token-abcdefghijklmnop"));

    // 第二道防线：即使工具把明文吐回结果面，脱敏器也拦得住「token=明文」形态。
    let leaked = json!({ "out": "token=token-abcdefghijklmnop" });
    let scrubbed = agent_core::Redactor::default_v0().redact_json(&leaked);
    assert!(!scrubbed.to_string().contains("token-abcdefghijklmnop"));

    // 解析口仍能取到真值（脱敏不改变执行面的真值来源）。
    assert_eq!(
        resolver
            .resolve_ref(&cred_ref_of(&reference))
            .expect("取真值"),
        "token-abcdefghijklmnop"
    );
}

// ---------------- 接线面：宿主注入口 ----------------

/// 宿主注入面（C1 的 `HostServices.credentials`）能装下本卡的解析口；
/// 未注入时是 fail-closed 桩（一律 `Err`，不返回明文）。
#[test]
fn host_services_accepts_store_resolver() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "host-secret");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope.clone());

    let services = HostServices {
        credentials: Some(Arc::new(resolver)),
        ..HostServices::default()
    };
    let injected = services
        .credentials
        .as_ref()
        .expect("凭据口已注入")
        .resolve(&cred_ref_of(&reference))
        .expect("宿主口径解析成功");
    assert_eq!(injected, "host-secret");

    // 未注入 ⇒ fail-closed 桩：显式错误，不返回空串。
    let stub = NoopCredentialResolver;
    let error = stub.resolve("cred_ref://abc").expect_err("桩必须拒绝");
    assert!(!error.is_empty(), "错误必须非空");
    assert!(!error.contains("cred_ref://abc"), "错误不得回显引用");
}

/// 宿主口径的错误文案可还原成类型化变体（错误语义跨层不失真）。
#[test]
fn host_error_message_round_trips_to_typed_error() {
    for error in [
        CredentialError::NotFound,
        CredentialError::Expired,
        CredentialError::ScopeMismatch,
        CredentialError::InvalidReference,
        CredentialError::DecryptFailed,
        CredentialError::StoreUnavailable,
        CredentialError::NotUtf8,
        CredentialError::EmptyPlaintext,
    ] {
        let message = error.to_host_message();
        let restored = CredentialError::from_host_message(&message);
        assert_eq!(
            restored.code(),
            error.code(),
            "错误码必须跨层一致: {message}"
        );
    }
}

// ---------------- 形态工具函数 ----------------

/// 引用形态解析：合法引用可解析，非法形态一律 `InvalidReference`。
#[test]
fn parse_ref_id_accepts_only_canonical_form() {
    let id = uuid::Uuid::new_v4();
    let good = format!("cred_ref://{id}");
    assert_eq!(
        parse_ref_id(&good).expect("合法引用").as_uuid(),
        id,
        "解析出的 id 必须与引用一致"
    );

    for bad in [
        "cred_ref://".to_string(),
        "cred_ref://not-a-uuid".to_string(),
        format!("cred_ref://{id}z"),
        id.to_string(),
        format!("CRED_REF://{id}"),
    ] {
        let error = parse_ref_id(&bad).expect_err("非法形态必须拒绝");
        assert!(
            matches!(error, CredentialError::InvalidReference),
            "非法形态必须是 InvalidReference: {bad} -> {error:?}"
        );
    }
}

/// 收集参数里的引用（去重、保序）。
#[test]
fn collect_cred_refs_dedups_and_keeps_order() {
    let a = format!("cred_ref://{}", uuid::Uuid::new_v4());
    let b = format!("cred_ref://{}", uuid::Uuid::new_v4());
    let args = json!({
        "x": a.clone(),
        "y": [b.clone(), a.clone()],
    });
    let refs = collect_cred_refs(&args);
    assert_eq!(refs, vec![a, b], "去重且保序");
}

/// 结果清洗：明文换回引用；无明文时原样返回。
#[test]
fn scrub_plaintexts_replaces_known_plaintexts() {
    let pairs = vec![("abc".to_string(), "cred_ref://x".to_string())];
    let scrubbed = scrub_plaintexts(&json!({ "out": "value=abc" }), &pairs);
    assert_eq!(scrubbed["out"], json!("value=cred_ref://x"));

    let untouched = scrub_plaintexts(&json!({ "out": "value=zzz" }), &pairs);
    assert_eq!(untouched["out"], json!("value=zzz"));
}

// ---------------- 宿主注入面（`resolve_cred_params` 口径） ----------------

/// 宿主注入面解析：成功走通；错误文案可还原成类型化变体。
#[test]
fn resolve_cred_params_uses_injected_resolver() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "injected-secret");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);

    let resolved = resolve_cred_params(
        &json!({ "token": cred_ref_of(&reference) }),
        &resolver as &dyn CredentialResolver,
    )
    .expect("解析成功");
    assert_eq!(resolved.args["token"], json!("injected-secret"));

    // 未注入 ⇒ 宿主口 fail-closed ⇒ 显式错误。
    let error = resolve_cred_params(
        &json!({ "token": cred_ref_of(&reference) }),
        &NoopCredentialResolver as &dyn CredentialResolver,
    )
    .expect_err("未注入必须失败");
    assert!(matches!(error, CredentialError::Rejected(_)));
}
