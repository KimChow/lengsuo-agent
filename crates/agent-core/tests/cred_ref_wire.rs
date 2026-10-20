//! 凭据引用化接线测试（C6）：`cred_ref://` 引用 → 明文，明文只进工具参数。
//!
//! 覆盖矩阵（对齐卡片的四条验收断言）：
//! - **有效引用** ⇒ 工具收到明文参数；事件流/历史里**只有引用**；
//! - **无效引用** ⇒ 显式错误，**工具不被执行**（零调用）；
//! - **越权引用**（记录属其它作用域）⇒ 显式错误；
//! - **解密失败**（口令错）⇒ 显式错误；
//! - **明文不出现在任何事件与日志中**（逐条扫描断言）。
//!
//! 语义出处：调研册 §16（引用化接线）+ §13（功能面对照）+ §15（AAD 绑定）；
//! 参考实现 `/home/ubuntu/ref/codex/codex-rs/secrets/`（`lib.rs` 门面 + `local.rs`
//! 后端 + `sanitizer.rs` 脱敏）。

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use agent_core::credentials::cred_params::{
    collect_cred_refs, resolve_cred_params, resolve_cred_params_typed, scrub_plaintexts,
    CredentialResolvingExecutor,
};
use agent_core::credentials::{parse_ref_id, CredentialError, StoreCredentialResolver};
use agent_core::host::{CredentialResolver, HostServices, NoopCredentialResolver};
use agent_core::ToolExecutor;
use credential_ref::{CredentialRef, FileStore, MemoryStore, Plaintext, Scope, SecretStore};

// ---------------- 引用形态 ----------------

/// 引用记号的文本形态：`cred_ref://` + 连字符 UUID。
fn cred_ref_of(reference: &CredentialRef) -> String {
    format!("cred_ref://{}", reference.id().as_uuid())
}

/// 造一条已入库的凭据，返回（引用, 明文）。
fn seed(store: &dyn SecretStore, scope: &Scope, plaintext: &str) -> CredentialRef {
    let reference = CredentialRef::new(scope.clone(), u64::MAX);
    store
        .put(&reference, &Plaintext::from_utf8_str(plaintext))
        .expect("写入凭据");
    reference
}

/// 越权场景用的「别的作用域」记录。
fn seed_other_scope(store: &dyn SecretStore, plaintext: &str) -> CredentialRef {
    let reference = CredentialRef::new(Scope::Tenant("tenant-other".to_string()), u64::MAX);
    store
        .put(&reference, &Plaintext::from_utf8_str(plaintext))
        .expect("写入他域凭据");
    reference
}

// ---------------- 记录型执行器 ----------------

/// 记录每次调用的执行器：参数原样留档，供断言「工具收到明文」。
#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<(String, Value)>>,
}

impl RecordingExecutor {
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("锁").clone()
    }
}

/// 可共享的执行器句柄（`Arc` 本身不实现 `ToolExecutor`，故用本 newtype 承载）。
struct SharedExecutor(Arc<RecordingExecutor>);

impl ToolExecutor for SharedExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.0
            .calls
            .lock()
            .expect("锁")
            .push((name.to_string(), args.clone()));
        Ok(json!({ "ok": true }))
    }
}

/// 回显执行器：把收到的参数原样回显（模拟工具把凭据打进输出）。
#[derive(Default)]
struct EchoExecutor {
    calls: Mutex<Vec<(String, Value)>>,
}

impl EchoExecutor {
    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("锁").clone()
    }
}

/// 可共享的回显执行器句柄。
struct SharedEcho(Arc<EchoExecutor>);

impl ToolExecutor for SharedEcho {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.0
            .calls
            .lock()
            .expect("锁")
            .push((name.to_string(), args.clone()));
        Ok(json!({ "echo": args.clone() }))
    }
}

/// 恒定解密失败的后端（模拟记录被篡改 / 口令错）。
struct FailingStore;

impl SecretStore for FailingStore {
    fn put(
        &self,
        _reference: &CredentialRef,
        _plaintext: &Plaintext,
    ) -> Result<(), credential_ref::StoreError> {
        Ok(())
    }

    fn get(&self, _reference: &CredentialRef) -> Result<Plaintext, credential_ref::StoreError> {
        Err(credential_ref::StoreError::RecordAuthentication)
    }

    fn delete(&self, _reference: &CredentialRef) -> Result<bool, credential_ref::StoreError> {
        Ok(false)
    }

    fn list(&self, _scope: &Scope) -> Result<Vec<CredentialRef>, credential_ref::StoreError> {
        Ok(Vec::new())
    }
}

// ---------------- 契约 1：有效引用 ⇒ 明文进参数，引用进上下文 ----------------

/// 有效引用 ⇒ 工具收到**明文**参数；替换返回**新值**（入参保持引用形态）。
#[test]
fn valid_ref_resolves_to_plaintext_in_params_only() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "s3cret-token");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);

    let args = json!({ "token": cred_ref_of(&reference), "path": "a.txt" });
    let resolved = resolve_cred_params_typed(&args, &resolver).expect("解析成功");

    assert_eq!(
        resolved.args["token"],
        json!("s3cret-token"),
        "工具参数里必须是明文"
    );
    assert_eq!(resolved.args["path"], json!("a.txt"), "非凭据参数原样保留");
    assert_eq!(
        args["token"],
        json!(cred_ref_of(&reference)),
        "入参不得被改写：进事件流/上下文的那一份始终是引用形态"
    );
    assert_eq!(resolved.pairs.len(), 1, "对照表记录一组（明文, 引用）");
    assert_eq!(resolved.pairs[0].1, cred_ref_of(&reference));
}

/// 引用出现在文本中间（`Bearer cred_ref://…`）也要替换。
#[test]
fn ref_inside_text_is_replaced() {
    let store = MemoryStore::new();
    let scope = Scope::Session("s-1".to_string());
    let reference = seed(&store, &scope, "abc123");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);
    let token = cred_ref_of(&reference);

    let args = json!({ "header": format!("Bearer {token}") });
    let resolved = resolve_cred_params_typed(&args, &resolver).expect("解析成功");

    assert_eq!(resolved.args["header"], json!("Bearer abc123"));
}

/// 数组与嵌套对象里的引用同样替换；对象**键名**不替换。
#[test]
fn nested_and_array_params_are_replaced_but_keys_are_not() {
    let store = MemoryStore::new();
    let scope = Scope::Env("env-a".to_string());
    let reference = seed(&store, &scope, "v");
    let resolver = StoreCredentialResolver::new(Arc::new(store), scope);
    let token = cred_ref_of(&reference);

    let args = json!({
        "list": [token.clone(), "plain"],
        "deep": { "inner": { "cred": token.clone() } },
        token.clone(): "键名不替换",
    });
    let resolved = resolve_cred_params_typed(&args, &resolver).expect("解析成功");

    assert_eq!(resolved.args["list"][0], json!("v"));
    assert_eq!(resolved.args["list"][1], json!("plain"));
    assert_eq!(resolved.args["deep"]["inner"]["cred"], json!("v"));
    assert_eq!(
        resolved.args[&token],
        json!("键名不替换"),
        "对象键名是 schema 名字，不是值：不得替换"
    );
}

// ---------------- 契约 2：无效引用 ⇒ 显式错误，工具不执行 ----------------

/// 引用形态非法 ⇒ 显式错误；工具**零调用**。
#[test]
fn invalid_ref_fails_closed_without_executing_tool() {
    let store = MemoryStore::new();
    let resolver = StoreCredentialResolver::new(Arc::new(store), Scope::Env("env-a".to_string()));
    let executor = Arc::new(RecordingExecutor::default());
    let wrapper = CredentialResolvingExecutor::new(
        SharedExecutor(executor.clone()),
        Arc::new(resolver) as Arc<dyn CredentialResolver>,
    );

    // 少一位十六进制 ⇒ 形态非法。
    let broken = format!("cred_ref://{}", "0".repeat(35));
    let error = wrapper
        .execute("http.fetch", &json!({ "token": broken }))
        .expect_err("非法引用必须失败");
    assert!(
        error.contains("invalid_reference"),
        "必须是显式的形态错误: {error}"
    );
    assert!(executor.calls().is_empty(), "工具必须零调用");
}

/// 引用不存在 ⇒ 显式错误；工具**零调用**；不返回空串。
#[test]
fn unknown_ref_fails_closed_without_executing_tool() {
    let store = MemoryStore::new();
    let resolver = StoreCredentialResolver::new(Arc::new(store), Scope::Env("env-a".to_string()));
    let executor = Arc::new(RecordingExecutor::default());
    let wrapper = CredentialResolvingExecutor::new(
        SharedExecutor(executor.clone()),
        Arc::new(resolver) as Arc<dyn CredentialResolver>,
    );

    let missing = format!("cred_ref://{}", uuid::Uuid::new_v4());
    let error = wrapper
        .execute("http.fetch", &json!({ "token": missing }))
        .expect_err("不存在的引用必须失败");
    assert!(
        error.contains("not_found"),
        "必须是显式的未找到错误: {error}"
    );
    assert!(executor.calls().is_empty(), "工具必须零调用");
}

// ---------------- 契约 3：越权 ⇒ 显式错误 ----------------

/// 记录属**别的作用域** ⇒ 显式错误（`scope_mismatch`），不返回明文。
#[test]
fn cross_scope_ref_is_rejected() {
    let store = MemoryStore::new();
    let other = seed_other_scope(&store, "other-tenant-secret");
    let resolver = StoreCredentialResolver::new(Arc::new(store), Scope::Env("env-a".to_string()));

    let error = resolver
        .resolve_ref(&cred_ref_of(&other))
        .expect_err("越权必须失败");
    assert!(
        matches!(error, CredentialError::ScopeMismatch),
        "必须是作用域不匹配: {error:?}"
    );
    assert_eq!(error.code(), "scope_mismatch");
    let text = error.to_string();
    assert!(
        !text.contains("other-tenant-secret"),
        "错误文案不得含明文: {text}"
    );
}

// ---------------- 契约 4：解密失败 ⇒ 显式错误 ----------------

/// 口令错 ⇒ 显式错误（`decrypt_failed`），不返回明文、不返回空串。
#[test]
fn decrypt_failure_is_explicit() {
    let scope = Scope::Env("env-a".to_string());
    let resolver = StoreCredentialResolver::new(Arc::new(FailingStore), scope);
    let reference = CredentialRef::new(Scope::Env("env-a".to_string()), u64::MAX);

    let error = resolver
        .resolve_ref(&cred_ref_of(&reference))
        .expect_err("解密失败必须显式报错");
    assert!(
        matches!(error, CredentialError::DecryptFailed),
        "必须是解密失败: {error:?}"
    );
    assert_eq!(error.code(), "decrypt_failed");
    let text = error.to_string();
    assert!(!text.is_empty(), "错误文案非空");
    assert!(
        !text.contains("cred_ref://"),
        "错误文案不得回显引用: {text}"
    );
}

/// 落盘后端（`FileStore`）用错口令重开 ⇒ 存储层 fail-closed（口令错误）。
#[test]
fn file_store_rejects_wrong_passphrase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("creds.bin");
    let store = FileStore::create(&path, "right-passphrase").expect("建库");
    let scope = Scope::Env("env-a".to_string());
    seed(&store, &scope, "file-secret");

    // 换错误口令重开同一文件 ⇒ 口令错（fail-closed，不当空库）。
    let error = FileStore::open(&path, "wrong-passphrase").expect_err("口令错必须失败");
    assert!(matches!(error, credential_ref::StoreError::WrongPassphrase));
}

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
