//! 契约 1：有效引用 ⇒ 明文进工具参数、引用进上下文（事件流/历史里只有引用）。
//!
//! 覆盖：替换返回**新值**（入参不被改写）、引用出现在文本中间、
//! 数组与嵌套对象里的引用同样替换而**对象键名不替换**。
use agent_core::credentials::cred_params::resolve_cred_params_typed;
use agent_core::credentials::StoreCredentialResolver;
use serde_json::json;
use std::sync::Arc;

use super::support::*;

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
