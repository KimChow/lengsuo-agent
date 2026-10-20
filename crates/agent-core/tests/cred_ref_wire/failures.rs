//! 契约 2/3/4：形态非法 / 引用不存在 / 越权 / 解密失败 ⇒ 显式错误。
//!
//! 四类失败都必须**显式报错**且工具**零调用**；形态非法还必须**不得**
//! 把疑似引用当普通文本放过（fail-closed）。
use agent_core::credentials::cred_params::CredentialResolvingExecutor;
use agent_core::credentials::{CredentialError, StoreCredentialResolver};
use agent_core::host::CredentialResolver;
use agent_core::ToolExecutor;
use serde_json::json;
use std::sync::Arc;

use super::support::*;

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

/// 各类**形态非法**的记号都必须显式报错，不得当普通文本放过。
///
/// 覆盖：少一位（35 位）、多一位（尾随一个十六进制数字 / 连字符）、
/// 非法字符（`not-a-uuid`）、以及只有 scheme 没有 id。
#[test]
fn malformed_ref_shapes_fail_closed_instead_of_passing_through() {
    let store = MemoryStore::new();
    let resolver = StoreCredentialResolver::new(Arc::new(store), Scope::Env("env-a".to_string()));
    let executor = Arc::new(RecordingExecutor::default());
    let wrapper = CredentialResolvingExecutor::new(
        SharedExecutor(executor.clone()),
        Arc::new(resolver) as Arc<dyn CredentialResolver>,
    );
    let good = uuid::Uuid::new_v4().to_string();
    for broken in [
        // 少一位十六进制。
        format!("cred_ref://{}", "0".repeat(35)),
        // 多一位：尾随一个十六进制数字。
        format!("cred_ref://{good}0"),
        // 多一位：尾随一个连字符。
        format!("cred_ref://{good}-"),
        // 非法字符。
        "cred_ref://not-a-uuid".to_string(),
        // 只有 scheme，没有 id。
        "cred_ref://".to_string(),
    ] {
        let error = wrapper
            .execute("http.fetch", &json!({ "token": broken.clone() }))
            .expect_err("形态非法必须失败");
        assert!(
            error.contains("invalid_reference"),
            "必须是显式的形态错误: {broken} -> {error}"
        );
    }
    assert!(executor.calls().is_empty(), "形态非法 ⇒ 工具必须零调用");
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
