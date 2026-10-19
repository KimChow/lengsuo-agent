//! spawn 前的子进程环境 **scrub** 规则（纯函数 + 常量，零 IO）。
//!
//! 安全基线：spawn 前 `env_clear`，只装回过滤过的变量——保留最小系统变量白名单
//! （[`SYSTEM_ENV_KEEP`]），剔除凭据族（名字含 `KEY`/`TOKEN`/`SECRET`/`PASSWORD`
//! 等，或云厂商前缀），支持显式 denylist 追加（[`env_pair_allowed`] 的
//! `extra_denied`）与「白名单整体替换」（同函数的 `allowlist_only`）两档收口。
//!
//! 判定方向是 **fail-closed**：不确定即视为敏感，一律剥离。非 UTF-8 的变量名/值
//! 也照此处理，但那一层在调用方（[`crate::stdio_client`] 的 spawn 流程）里做：
//! 本模块只回答「这个名字可不可以传」。

/// 凭据族变量名模式（大写包含匹配）。
const CREDENTIAL_PATTERNS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "COOKIE",
    "AUTH",
];

/// 凭据族前缀（云厂商/凭证链）。
const CREDENTIAL_PREFIXES: &[&str] = &["AWS_", "AZURE_", "GCP_", "GOOGLE_", "ALIBABA_CLOUD_"];

/// 最小系统变量白名单（allowlist 模式下唯一保留集）。
pub const SYSTEM_ENV_KEEP: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// 变量名是否属于凭据族（不确定即视为敏感）。
#[must_use]
pub fn is_credential_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    CREDENTIAL_PREFIXES.iter().any(|p| upper.starts_with(p))
        || CREDENTIAL_PATTERNS.iter().any(|p| upper.contains(p))
}

/// scrub 规则：该变量是否可传给子进程。
#[must_use]
pub fn env_pair_allowed(key: &str, allowlist_only: bool, extra_denied: &[String]) -> bool {
    if extra_denied.iter().any(|d| d == key) || is_credential_env(key) {
        return false;
    }
    !allowlist_only || SYSTEM_ENV_KEEP.contains(&key)
}
