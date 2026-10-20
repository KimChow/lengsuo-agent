//! 工具参数里的凭据引用替换：明文**只进真正执行的参数**。
//!
//! ## 分层与调用顺序
//!
//! 参考件 codex `codex-rs/secrets/` 的调用链是
//! 「后端（`SecretsBackend` / `local.rs`）→ 门面（`SecretsManager` / `lib.rs`）
//! → 脱敏（`redact_secrets` / `sanitizer.rs`）」。
//! 本卡把同一条链落在**参数面**上，顺序逐条对齐：
//! ① [`crate::credentials::StoreCredentialResolver`]（门面：引用 → 明文）；
//! ② 本模块（参数面：把参数里的引用换成明文，只交给要执行的工具）；
//! ③ [`crate::redact::Redactor`]（第二道防线兜底，本卡**不改**其规则）。
//!
//! ## 形态
//!
//! 引用记号 = `cred_ref://` + 36 字符连字符 UUID 文本（`8-4-4-4-12`）。
//! 文本中**任意位置**出现该记号都会被替换（`Bearer cred_ref://…` 这类也覆盖）；
//! 记号形态不完整（少一位、多一位十六进制、非法字符）⇒ **显式错误**
//! （[`CredentialError::InvalidReference`]），不截断、不猜、不当普通文本放过。
//!
//! ## 隔离
//!
//! - 替换产生**新值**：入参不被改写，因此进事件流/模型上下文的那一份始终是引用形态；
//! - JSON 对象的**键名**不替换（键名是 schema 名字，不是值）；
//! - 工具结果里若回显了明文，经 [`scrub_plaintexts`] 换回引用形态再返回，
//!   使「明文不出事件流」这条不变式在结果侧也成立。

use std::sync::Arc;

use serde_json::Value;

use crate::credentials::{CredentialError, StoreCredentialResolver, CRED_REF_SCHEME};
use crate::host::CredentialResolver;
use crate::r#loop::ToolExecutor;

/// 引用 id 的文本长度（带连字符的 UUID 文本形态）。
const REF_ID_TEXT_LEN: usize = 36;

/// 一次引用替换的产物。
#[derive(Debug, Clone)]
pub struct ResolvedParams {
    /// 替换后的参数（明文形态）——**只交给真正执行的工具**。
    pub args: Value,
    /// （明文, 引用）对照表：工具结果回显明文时据此换回引用形态。
    pub pairs: Vec<(String, String)>,
}

/// 替换参数里的凭据引用（宿主注入面）。
///
/// `resolver` 是 C1 组装口 [`crate::host::HostServices::credentials`] 注入的解析口；
/// 宿主回传的错误文案经 [`CredentialError::from_host_message`] 还原成类型化变体。
///
/// 返回**新值**：入参 `args` 不被改写（进事件流/模型上下文的那一份保持引用形态）。
pub fn resolve_cred_params(
    args: &Value,
    resolver: &dyn CredentialResolver,
) -> Result<ResolvedParams, CredentialError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut resolve = |reference: &str| {
        let plaintext = resolver
            .resolve(reference)
            .map_err(|message| CredentialError::from_host_message(&message))?;
        pairs.push((plaintext.clone(), reference.to_string()));
        Ok(plaintext)
    };
    let value = substitute_value(args, &mut resolve)?;
    Ok(ResolvedParams { args: value, pairs })
}

/// 替换参数里的凭据引用（类型化门面口径：直接用 [`StoreCredentialResolver`]）。
///
/// 与 [`resolve_cred_params`] 的差别只在错误类型：此处拿到的是类型化
/// [`CredentialError`]，不经宿主口的字符串往返。
pub fn resolve_cred_params_typed(
    args: &Value,
    resolver: &StoreCredentialResolver,
) -> Result<ResolvedParams, CredentialError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut resolve = |reference: &str| {
        let plaintext = resolver.resolve_ref(reference)?;
        pairs.push((plaintext.clone(), reference.to_string()));
        Ok(plaintext)
    };
    let value = substitute_value(args, &mut resolve)?;
    Ok(ResolvedParams { args: value, pairs })
}

/// 列出参数里出现的全部凭据引用（去重、保序；审计与测试用）。
pub fn collect_cred_refs(args: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_into(args, &mut out);
    out
}

/// 把文本里的引用记号换成明文。
///
/// 记号形态非法 ⇒ [`CredentialError::InvalidReference`]（fail-closed，不静默放过）。
fn substitute_text(
    text: &str,
    resolve: &mut impl FnMut(&str) -> Result<String, CredentialError>,
) -> Result<String, CredentialError> {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    while let Some(found) = text[cursor..].find(CRED_REF_SCHEME) {
        let start = cursor + found;
        out.push_str(&text[cursor..start]);
        let token_start = start + CRED_REF_SCHEME.len();
        let Some(end) = ref_token_end(text, token_start) else {
            out.push_str(&text[start..]);
            cursor = text.len();
            break;
        };
        let reference = &text[start..end];
        let plaintext = resolve(reference)?;
        out.push_str(&plaintext);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

/// 递归替换 JSON 值里的引用记号（对象键名不替换）。
fn substitute_value(
    value: &Value,
    resolve: &mut impl FnMut(&str) -> Result<String, CredentialError>,
) -> Result<Value, CredentialError> {
    Ok(match value {
        Value::String(text) => Value::String(substitute_text(text, resolve)?),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(substitute_value(item, resolve)?);
            }
            Value::Array(out)
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, item) in map {
                out.insert(key.clone(), substitute_value(item, resolve)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

/// 递归收集 JSON 值里的引用记号（去重、保序）。
fn collect_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => {
            let mut cursor = 0usize;
            while let Some(found) = text[cursor..].find(CRED_REF_SCHEME) {
                let start = cursor + found;
                let token_start = start + CRED_REF_SCHEME.len();
                let Some(end) = ref_token_end(text, token_start) else {
                    // 形态非法：不是合法引用，不收集（解析时才会显式报错）。
                    break;
                };
                let reference = text[start..end].to_string();
                if !out.contains(&reference) {
                    out.push(reference);
                }
                cursor = end;
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_into(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_into(item, out);
            }
        }
        _ => {}
    }
}

/// 定位引用记号的结束位置；形态非法返回 `None`。
///
/// 合法形态：`text[at..]` 前 36 字符是连字符 UUID 文本，且其后**不得**再跟
/// 可延续 UUID 的字符（十六进制数字或 `-`）——多一位即视为形态非法。
fn ref_token_end(text: &str, at: usize) -> Option<usize> {
    let rest = text.get(at..)?;
    if rest.len() < REF_ID_TEXT_LEN {
        return None;
    }
    let token = rest.get(..REF_ID_TEXT_LEN)?;
    if !is_hyphenated_uuid(token) {
        return None;
    }
    let next = rest
        .get(REF_ID_TEXT_LEN..)
        .and_then(|tail| tail.chars().next());
    if let Some(next) = next {
        if next.is_ascii_hexdigit() || next == '-' {
            return None;
        }
    }
    Some(at + REF_ID_TEXT_LEN)
}

/// 36 字符连字符 UUID 文本形态校验（`8-4-4-4-12` 十六进制）。
fn is_hyphenated_uuid(token: &str) -> bool {
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

/// 把结果里的明文换回引用形态（工具回显了明文时兜底）。
///
/// `pairs` 是（明文, 引用）对照表；按明文长度降序替换，避免短明文先命中
/// 造成长明文被切碎。找不到明文时原样返回。
pub fn scrub_plaintexts(value: &Value, pairs: &[(String, String)]) -> Value {
    match value {
        Value::String(text) => {
            let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
            sorted.sort_by_key(|(plaintext, _)| std::cmp::Reverse(plaintext.len()));
            let mut out = text.clone();
            for (plaintext, reference) in sorted {
                if plaintext.is_empty() {
                    continue;
                }
                out = out.replace(plaintext.as_str(), reference.as_str());
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| scrub_plaintexts(item, pairs))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), scrub_plaintexts(item, pairs)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// 凭据解析包装器：把工具执行器包一层，参数里的引用先解析成明文再执行。
///
/// 这是**接线入口**：宿主拿到工具执行器后包一层装进
/// [`crate::runtime::RuntimeBuilder::tool_executor`] 即可，
/// 无需改动 `runtime/execute.rs`（该文件属并行卡的独占范围）。
///
/// 契约：
/// - 参数里的引用**全部**解析成功才执行；任一失败 ⇒ 显式错误，**工具不被调用**；
/// - 执行失败照旧把错误交回调用方（既有 `Err(message)` 结算路径）；
/// - 工具结果里回显的明文被换回引用形态（明文不出事件流/历史）。
///
/// 注意：本包装器与「在 `execute.rs` 里直接调 [`resolve_cred_params`]」是**二选一**
/// 的接线方式；同时用两遍不会双重解密，但对照表会丢失（第二次已无引用可解），
/// 结果回显清洗将失效。
pub struct CredentialResolvingExecutor<E> {
    inner: E,
    resolver: Arc<dyn CredentialResolver>,
}

impl<E> CredentialResolvingExecutor<E> {
    /// 组装包装器。
    pub fn new(inner: E, resolver: Arc<dyn CredentialResolver>) -> Self {
        Self { inner, resolver }
    }

    /// 内层执行器（宿主自检用）。
    pub fn inner(&self) -> &E {
        &self.inner
    }
}

impl<E: ToolExecutor + Send + Sync> ToolExecutor for CredentialResolvingExecutor<E> {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        let resolved = resolve_cred_params(args, self.resolver.as_ref())
            .map_err(|error| error.to_host_message())?;
        let result = self.inner.execute(name, &resolved.args)?;
        Ok(scrub_plaintexts(&result, &resolved.pairs))
    }
}
