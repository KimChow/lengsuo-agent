//! 凭据引用化接线（C6）：`cred_ref://` 引用 → 明文的解析口，与工具参数替换。
//!
//! ## 不变式（本卡的核心契约）
//! - **模型上下文、事件流、日志里只出现引用**：形态固定为 `cred_ref://<uuid>`；
//! - **明文只进真正执行的参数**：解析结果只交给工具执行器，不回写历史/事件/日志；
//! - **fail-closed**：引用无效 / 越权 / 过期 / 解密失败 ⇒ 显式错误
//!   （[`CredentialError`]），**绝不静默返回空串**，也绝不把引用原文当明文传下去。
//!
//! ## 分层（对齐参考件 codex `codex-rs/secrets/` 的「后端 → 门面 → 脱敏」三层）
//! - codex `SecretsBackend`（`local.rs`）↔ 本仓 [`credential_ref::SecretStore`]
//!   （存储后端，本卡**不改**其加密与 AAD 实现）；
//! - codex `SecretsManager`（`lib.rs`）↔ 本仓 [`StoreCredentialResolver`]（解析门面，本卡新增）；
//! - codex `redact_secrets`（`sanitizer.rs`）↔ 本仓 [`crate::redact::Redactor`]
//!   （**第二道防线**，本卡不改规则）。
//!
//! ## 作用域绑定（为什么引用里不带作用域）
//! 解析口在**组装期绑定一个作用域**（宿主按会话/租户组装），引用只带凭据 id。
//! 若让模型自己指定作用域，它就能构造「他人作用域 + 正确 id」把别人的凭据取走；
//! 绑定作用域后，越权访问必然撞上存储层的 `ScopeMismatch`（显式错误）。
//!
//! ## 与 C1 组装口的关系
//! [`StoreCredentialResolver`] 实现 C1 留下的注入面
//! [`crate::host::CredentialResolver`]，宿主经
//! [`crate::host::HostServices::credentials`] 注入即可；
//! 类型化错误经 [`StoreCredentialResolver::resolve_ref`] 与
//! [`cred_params`] 的返回类型暴露（`host.rs` 属 C1 的文件，本卡不改其 trait 签名）。

use std::fmt;
use std::sync::Arc;

use credential_ref::{CredentialRef, Plaintext, RefId, Scope, SecretStore, StoreError};
use uuid::Uuid;

use crate::host::CredentialResolver;

pub mod cred_params;

/// 凭据引用的 scheme 前缀（模型上下文、事件流、日志里出现的唯一形态）。
pub const CRED_REF_SCHEME: &str = "cred_ref://";

/// 类型化凭据错误。
///
/// 全部变体都不含明文、口令、派生密钥、密文字节或作用域目标 id；
/// [`CredentialError::code`] 给上层稳定短码（便于映射到信封 code）。
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// 引用不存在（存储层未找到该 id）。
    #[error("凭据引用不存在")]
    NotFound,
    /// 引用已过期（达到过期时刻即过期，不宽限）。
    #[error("凭据引用已过期")]
    Expired,
    /// 引用作用域与记录作用域不一致：越权访问被拒。
    #[error("凭据作用域不匹配，拒绝跨作用域访问")]
    ScopeMismatch,
    /// 引用形态非法（不是 `cred_ref://<uuid>`）。
    #[error("凭据引用非法，已拒绝")]
    InvalidReference,
    /// 解密失败（口令错误 / 记录认证失败 / 容器结构非法 / 版本不识别）。
    #[error("凭据解密失败")]
    DecryptFailed,
    /// 存储层不可用（IO 失败）。
    #[error("凭据存储不可用")]
    StoreUnavailable,
    /// 明文不是合法 UTF-8。
    #[error("凭据明文不是合法 UTF-8")]
    NotUtf8,
    /// 明文为空：拒绝以空串充当凭据（失败与「空凭据」必须可区分）。
    #[error("凭据明文为空，拒绝以空串充当凭据")]
    EmptyPlaintext,
    /// 被宿主解析口拒绝（错误文案由宿主给出；不含明文）。
    #[error("凭据解析被拒绝: {0}")]
    Rejected(String),
}

impl CredentialError {
    /// 稳定短码（不含敏感信息），便于上层映射错误码。
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Expired => "expired",
            Self::ScopeMismatch => "scope_mismatch",
            Self::InvalidReference => "invalid_reference",
            Self::DecryptFailed => "decrypt_failed",
            Self::StoreUnavailable => "store_unavailable",
            Self::NotUtf8 => "not_utf8",
            Self::EmptyPlaintext => "empty_plaintext",
            Self::Rejected(_) => "rejected",
        }
    }

    /// 存储层错误 → 类型化错误（一一对应，语义不合并）。
    ///
    /// 口令错误、记录认证失败、容器结构非法、版本不识别**都归入**
    /// [`CredentialError::DecryptFailed`]：对调用点而言都是「取不到明文」的同一
    /// 失败面，但原始变体不被吞掉（`StoreError` 自身已是脱敏的）。
    pub fn from_store_error(error: StoreError) -> Self {
        match error {
            StoreError::NotFound => Self::NotFound,
            StoreError::Expired => Self::Expired,
            StoreError::ScopeMismatch => Self::ScopeMismatch,
            StoreError::InvalidReference => Self::InvalidReference,
            StoreError::WrongPassphrase
            | StoreError::RecordAuthentication
            | StoreError::MalformedContainer(_)
            | StoreError::UnsupportedVersion { .. } => Self::DecryptFailed,
            StoreError::Io(_) => Self::StoreUnavailable,
        }
    }

    /// 还原宿主解析口（`host::CredentialResolver`）的错误文案。
    ///
    /// 约定：宿主口回传 `"{code}: {文案}"`，本函数据此还原类型化变体；
    /// 不符合约定的文案一律归入 [`CredentialError::Rejected`]。
    pub fn from_host_message(message: &str) -> Self {
        let Some((code, rest)) = message.split_once(": ") else {
            return Self::Rejected(message.to_string());
        };
        match code {
            "not_found" => Self::NotFound,
            "expired" => Self::Expired,
            "scope_mismatch" => Self::ScopeMismatch,
            "invalid_reference" => Self::InvalidReference,
            "decrypt_failed" => Self::DecryptFailed,
            "store_unavailable" => Self::StoreUnavailable,
            "not_utf8" => Self::NotUtf8,
            "empty_plaintext" => Self::EmptyPlaintext,
            "rejected" => Self::Rejected(rest.to_string()),
            _ => Self::Rejected(message.to_string()),
        }
    }

    /// 交给宿主口的错误文案（`"{code}: {文案}"`，与 [`Self::from_host_message`] 对称）。
    pub fn to_host_message(&self) -> String {
        format!("{}: {self}", self.code())
    }
}

/// 解析 `cred_ref://<uuid>` 形态的引用；形态不符 ⇒ [`CredentialError::InvalidReference`]。
///
/// 只认「scheme + 36 字节连字符 UUID 文本」这一种形态：多一位少一位都不是引用，
/// 绝不「截断后凑合解析」。
pub fn parse_ref_id(reference: &str) -> Result<RefId, CredentialError> {
    let rest = reference
        .strip_prefix(CRED_REF_SCHEME)
        .ok_or(CredentialError::InvalidReference)?;
    // 只认「scheme + 36 字符连字符 UUID 文本」这一种形态：多一位少一位都不是引用，
    // 绝不「截断后凑合解析」。
    if !is_hyphenated_uuid_text(rest) {
        return Err(CredentialError::InvalidReference);
    }
    let uuid = Uuid::parse_str(rest).map_err(|_| CredentialError::InvalidReference)?;
    Ok(RefId::from_uuid(uuid))
}

/// 36 字符连字符 UUID 文本形态校验（`8-4-4-4-12` 十六进制）。
///
/// **只认这一种形态**：32 字符无连字符形式、`urn:uuid:` 前缀形式、花括号形式
/// 一律不收（`Uuid::parse_str` 收那些，但它们不是本仓的引用文本形态）。
/// 判据单点定义 here：`parse_ref_id` 与参数面扫描器共用同一份，不写两套。
pub(crate) fn is_hyphenated_uuid_text(token: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut rest = token;
    for (index, len) in GROUPS.iter().enumerate() {
        if index > 0 {
            let Some(stripped) = rest.strip_prefix('-') else {
                return false;
            };
            rest = stripped;
        }
        let Some(head) = rest.get(..*len) else {
            return false;
        };
        if !head.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return false;
        }
        rest = rest.get(*len..).unwrap_or_default();
    }
    rest.is_empty()
}

/// 把 `credential_ref::SecretStore` 适配成宿主注入的凭据解析口。
///
/// 参照参考件 codex `secrets/src/lib.rs` 的 `SecretsManager`：**门面只转发、
/// 不持有策略**；作用域绑定是本仓为「越权 fail-closed」加的一层（codex 的
/// `SecretScope` 由调用方逐次传入，本仓把作用域固定在解析口上）。
///
/// 解析链（与参考件 `SecretsManager::get` 的调用顺序一致）：
/// ① 解析引用形态 → ② 绑定作用域组装 [`CredentialRef`] → ③ 存储层取明文
/// → ④ UTF-8 解码 → ⑤ 空串检查。任何一步失败都是**显式错误**。
pub struct StoreCredentialResolver {
    store: Arc<dyn SecretStore>,
    scope: Scope,
}

impl StoreCredentialResolver {
    /// 组装解析口：`store` 是凭据后端，`scope` 是本解析口**唯一**可见的作用域。
    pub fn new(store: Arc<dyn SecretStore>, scope: Scope) -> Self {
        Self { store, scope }
    }

    /// 本解析口绑定的作用域（只读；作用域目标 id 属隔离标识，不进日志）。
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// 凭据后端句柄。
    pub fn store(&self) -> &Arc<dyn SecretStore> {
        &self.store
    }

    /// 类型化解析：引用 → 明文。
    ///
    /// - 引用形态非法 ⇒ [`CredentialError::InvalidReference`]；
    /// - 绑定作用域非法（空目标 / 超长目标）⇒ [`CredentialError::InvalidReference`]；
    /// - 记录不存在 ⇒ [`CredentialError::NotFound`]；
    /// - 记录属其它作用域 ⇒ [`CredentialError::ScopeMismatch`]（越权）；
    /// - 记录已过期 ⇒ [`CredentialError::Expired`]；
    /// - 解密失败 ⇒ [`CredentialError::DecryptFailed`]；
    /// - 明文非 UTF-8 / 为空 ⇒ [`CredentialError::NotUtf8`] / [`CredentialError::EmptyPlaintext`]。
    pub fn resolve_ref(&self, reference: &str) -> Result<String, CredentialError> {
        let id = parse_ref_id(reference)?;
        // 过期时刻用「永不过期」占位：真正的过期判定由存储层按**记录自带的**
        // 过期时刻执行（`SecretStore::get` 的既有语义），解析口不另设一套。
        let bound = CredentialRef::with_id(id, self.scope.clone(), u64::MAX);
        if !bound.is_valid() {
            // 绑定作用域非法（空目标或超长目标）⇒ 引用无法定位，拒绝解析。
            return Err(CredentialError::InvalidReference);
        }
        let plaintext: Plaintext = self
            .store
            .get(&bound)
            .map_err(CredentialError::from_store_error)?;
        let text = plaintext.to_utf8().ok_or(CredentialError::NotUtf8)?;
        if text.is_empty() {
            // 空明文与「解析失败」必须可区分：绝不返回空串充当凭据。
            return Err(CredentialError::EmptyPlaintext);
        }
        Ok(text)
    }
}

impl fmt::Debug for StoreCredentialResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 作用域目标 id 属隔离标识：只报类别，不报目标、不报后端内部。
        f.debug_struct("StoreCredentialResolver")
            .field("scope_kind", &self.scope.kind().as_str())
            .field("redacted", &true)
            .finish()
    }
}

impl CredentialResolver for StoreCredentialResolver {
    /// 宿主注入面（C1 的 `HostServices.credentials`）：错误以
    /// `"{code}: {文案}"` 回传，调用方可经 [`CredentialError::from_host_message`]
    /// 还原类型化变体。
    fn resolve(&self, reference: &str) -> Result<String, String> {
        self.resolve_ref(reference)
            .map_err(|error| error.to_host_message())
    }
}
