//! 凭据引用化接线测试的共享 helper（引用记号/记录构造/执行器/失败后端）。
//!
//! 由 `main.rs` 统一 re-export，各主题文件 `use super::support::*;` 取用。
use agent_core::ToolExecutor;
use credential_ref::{Plaintext, SecretStore};
use serde_json::json;
use std::sync::Mutex;

// 共享类型对主题文件可见：主题文件 `use super::support::*;` 即可取用。
pub use credential_ref::{CredentialRef, FileStore, MemoryStore, Scope};
pub use serde_json::Value;
pub use std::sync::Arc;
// ---------------- 引用形态 ----------------

/// 引用记号的文本形态：`cred_ref://` + 连字符 UUID。
pub fn cred_ref_of(reference: &CredentialRef) -> String {
    format!("cred_ref://{}", reference.id().as_uuid())
}

/// 造一条已入库的凭据，返回（引用, 明文）。
pub fn seed(store: &dyn SecretStore, scope: &Scope, plaintext: &str) -> CredentialRef {
    let reference = CredentialRef::new(scope.clone(), u64::MAX);
    store
        .put(&reference, &Plaintext::from_utf8_str(plaintext))
        .expect("写入凭据");
    reference
}

/// 越权场景用的「别的作用域」记录。
pub fn seed_other_scope(store: &dyn SecretStore, plaintext: &str) -> CredentialRef {
    let reference = CredentialRef::new(Scope::Tenant("tenant-other".to_string()), u64::MAX);
    store
        .put(&reference, &Plaintext::from_utf8_str(plaintext))
        .expect("写入他域凭据");
    reference
}

// ---------------- 记录型执行器 ----------------

/// 记录每次调用的执行器：参数原样留档，供断言「工具收到明文」。
#[derive(Default)]
pub struct RecordingExecutor {
    calls: Mutex<Vec<(String, Value)>>,
}

impl RecordingExecutor {
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("锁").clone()
    }
}

/// 可共享的执行器句柄（`Arc` 本身不实现 `ToolExecutor`，故用本 newtype 承载）。
pub struct SharedExecutor(pub Arc<RecordingExecutor>);

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
pub struct EchoExecutor {
    calls: Mutex<Vec<(String, Value)>>,
}

impl EchoExecutor {
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("锁").clone()
    }
}

/// 可共享的回显执行器句柄。
pub struct SharedEcho(pub Arc<EchoExecutor>);

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
pub struct FailingStore;

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
