//! R1–R4 断言与「字段面 / 模板引用 / 赋值接线」的用例。

use super::extractors::template_refs;
use super::{
    balanced_span, is_identifier, lex, normalized, split_top_level, CONTEXT_SOURCE,
    PROMPT_VARS_SOURCE, TEMPLATE_SOURCES,
};
use std::collections::BTreeSet;

// ── `PromptVars` 字段面 ──

/// `PromptVars` 的字段名集合（从结构体定义抽；**不锁字段数量**）。
fn prompt_vars_fields(model_src: &str) -> BTreeSet<String> {
    let (code, _) = lex(model_src);
    let start = code
        .find("pub struct PromptVars {")
        .expect("model.rs 必须定义 `pub struct PromptVars`");
    let open = start + "pub struct PromptVars {".len() - 1;
    let (body_start, body_end) = balanced_span(&code, open).expect("PromptVars 结构体必须闭合");
    let mut fields = BTreeSet::new();
    for (a, b) in split_top_level(&code, body_start, body_end) {
        let item = code[a..b].trim();
        let Some(rest) = item.strip_prefix("pub ") else {
            continue;
        };
        let name = rest.split(':').next().unwrap_or("").trim();
        if is_identifier(name) {
            fields.insert(name.to_string());
        }
    }
    fields
}

/// `prompt_vars()` 里 `PromptVars { … }` 结构体字面量的**内容**区间。
fn prompt_vars_literal(code: &str) -> Option<(usize, usize)> {
    let fn_at = code.find("fn prompt_vars(")?;
    let params_open = fn_at + "fn prompt_vars".len();
    let (_, params_close) = balanced_span(code, params_open)?;
    let body_open = code[params_close..].find('{')? + params_close;
    let (body_start, body_end) = balanced_span(code, body_open)?;
    let lit = code[body_start..body_end].find("PromptVars {")? + body_start;
    balanced_span(code, lit + "PromptVars {".len() - 1)
}

/// `prompt_vars()` 里被**显式赋值**的字段名集合。
fn assigned_fields(context_src: &str) -> BTreeSet<String> {
    let (code, _) = lex(context_src);
    let (start, end) = prompt_vars_literal(&code)
        .expect("context.rs 必须保留 prompt_vars() 的 `PromptVars { … }`");
    let mut fields = BTreeSet::new();
    for (a, b) in split_top_level(&code, start, end) {
        if let Some(name) = field_name(&code[a..b]) {
            fields.insert(name.to_string());
        }
    }
    fields
}

/// 取一段赋值项的**字段名**（`标识符` + `:`）；不是字段形态则返回 `None`。
fn field_name(item: &str) -> Option<&str> {
    let item = item.trim();
    let end = item.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    let name = &item[..end];
    let rest = item[end..].trim_start();
    if !rest.starts_with(':') || rest.starts_with("::") || name.is_empty() {
        return None;
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(name)
}

// ── 断言出口 ──

/// **R1**：`PromptVars` 的**每个字段**都在 `prompt_vars()` 里被显式赋值。
pub(super) fn assert_r1_fields_assigned(model_src: &str, context_src: &str) {
    let fields = prompt_vars_fields(model_src);
    assert!(
        !fields.is_empty(),
        "必须能从 model.rs 抽出 PromptVars 的字段清单"
    );
    let assigned = assigned_fields(context_src);
    let missing: Vec<&String> = fields.difference(&assigned).collect();
    assert!(
        missing.is_empty(),
        "PromptVars 字段未在 prompt_vars() 里显式赋值：{missing:?}（漏赋值 ⇒ 走默认值 ⇒ 模板段静默不渲染）"
    );
}

/// **R2**：`prompt_vars()` 里没有 `..Default::default()` / `..PromptVars::default()`
/// 这类**批量兜底**（兜底会把漏填的字段藏起来）。
pub(super) fn assert_r2_no_batch_default(context_src: &str) {
    let (code, _) = lex(context_src);
    let (start, end) = prompt_vars_literal(&code)
        .expect("context.rs 必须保留 prompt_vars() 的 `PromptVars { … }`");
    for (a, b) in split_top_level(&code, start, end) {
        let item = code[a..b].trim();
        assert!(
            !item.starts_with(".."),
            "prompt_vars() 不得用 `{item}` 批量兜底：兜底会把漏填的字段藏起来"
        );
    }
    // 去掉全部空白后再比对 ⇒ 跨行写法与单行写法等价。
    let squashed: String = code[start..end].split_whitespace().collect();
    assert!(
        !squashed.contains("..Default::default()") && !squashed.contains("..PromptVars::default()"),
        "prompt_vars() 不得用 `..Default::default()` / `..PromptVars::default()` 批量兜底"
    );
}

/// **R3**：模板引用的变量名都存在于 `PromptVars`。
pub(super) fn assert_r3_template_refs_exist(model_src: &str, sources: &[(&str, &str)]) {
    let fields = prompt_vars_fields(model_src);
    let refs = template_refs(sources);
    let unknown: Vec<(&String, &BTreeSet<String>)> = refs
        .iter()
        .filter(|(name, _)| !fields.contains(*name))
        .collect();
    assert!(
        unknown.is_empty(),
        "模板引用了 PromptVars 里没有的变量：{unknown:?}（未知键 ⇒ 该模板段静默不渲染）"
    );
}

/// **R4**：`prompt_vars()` 里赋的字段名都存在于 `PromptVars`（防拼写错 / 残留旧名）。
pub(super) fn assert_r4_assigned_fields_known(model_src: &str, context_src: &str) {
    let fields = prompt_vars_fields(model_src);
    let assigned = assigned_fields(context_src);
    let unknown: Vec<&String> = assigned.difference(&fields).collect();
    assert!(
        unknown.is_empty(),
        "prompt_vars() 赋了 PromptVars 里不存在的字段：{unknown:?}（改名 / 拼写错 ⇒ 编译期不报错）"
    );
}

// ── 用例 ──

/// ① R1：`PromptVars` 每个字段都在 `prompt_vars()` 里被显式赋值（**不锁字段总数**）。
#[test]
fn prompt_vars_fields_are_all_explicitly_assigned() {
    assert!(normalized(PROMPT_VARS_SOURCE).contains("pub struct PromptVars {"));
    assert_r1_fields_assigned(PROMPT_VARS_SOURCE, CONTEXT_SOURCE);
}

/// ② R2：`prompt_vars()` 里没有 `..Default::default()` / `..PromptVars::default()` 兜底。
#[test]
fn prompt_vars_has_no_batch_default_fallback() {
    assert_r2_no_batch_default(CONTEXT_SOURCE);
}

/// ③ R3：模板引用的变量名都存在于 `PromptVars`。
#[test]
fn template_variable_refs_all_exist_in_prompt_vars() {
    assert_r3_template_refs_exist(PROMPT_VARS_SOURCE, TEMPLATE_SOURCES);
}

/// ④ R4：`prompt_vars()` 里赋的字段名都存在于 `PromptVars`。
#[test]
fn assigned_field_names_all_exist_in_prompt_vars() {
    assert_r4_assigned_fields_known(PROMPT_VARS_SOURCE, CONTEXT_SOURCE);
}

/// ⑪ 模板文件清单必须覆盖 `templates/**` 下的全部 `.rs`（防「新增模板漏登记」）。
#[test]
fn template_sources_cover_every_template_file() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../agent-prompts/src");
    let mut found: Vec<String> = Vec::new();
    collect_rs_files(&root.join("templates"), "templates/", &mut found);
    found.sort();
    assert!(!found.is_empty(), "templates/ 目录必须存在");
    for path in &found {
        let rel = path.strip_suffix(".rs").unwrap_or(path);
        assert!(
            TEMPLATE_SOURCES.iter().any(|(label, _)| *label == rel),
            "模板文件 {path} 未登记进 TEMPLATE_SOURCES：新增模板必须登记，否则它的变量引用不会被扫到"
        );
    }
}

/// 递归收集目录下全部 `.rs` 文件的相对路径。
fn collect_rs_files(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            collect_rs_files(&path, &format!("{prefix}{name}/"), out);
        } else if name.ends_with(".rs") {
            out.push(format!("{prefix}{name}"));
        }
    }
}
