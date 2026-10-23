//! `json!` 键名扫描基元（含注释行排除的自证）。

use std::collections::BTreeSet;

/// 一行是不是注释：去掉缩进后以 `//` 开头。
///
/// 必须排除：被扫描的文件里有大量 `///` 行文档注释，注释里出现的键名
/// （如 `"argsRef"`、`"tsMs"`）会被误算进**实现面**——那正是「注释里的键名
/// 被当成实现面」的坑。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 去掉行注释后的源码文本。
fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .filter(|line| !is_comment_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 抽出文本里所有 `json!({...})` 对象的**顶层键名**（并成一个集合）。
///
/// 口径（手写字符扫描，不用正则）：
/// 1. 找每个 `json!` 记号，其后（跳过空白）必须是 `(`，再（跳过空白）必须是 `{`
///    ——否则不是本口径要扫的「对象字面量」；
/// 2. 从 `{` 起做**花括号配对**扫描（跳过字符串字面量内部的花括号与转义），
///    只收**深度 1**（最外层对象）上「后随 `:` 的字符串字面量」——即键名。
///
/// 为什么只取深度 1：嵌套对象（如挂在 `payload["result"]` 之下的错误对象
/// `{"type":…,"message":…}`）的键**不是** payload 顶层键。
///
/// 嵌套的 `json!` 会被**单独**扫一遍（主循环继续往后找 `json!` 记号），
/// 故嵌套错误对象的键也会进结果集——它们是实现侧真实发出的键，登记在宽面
/// 常量里更安全。
pub(super) fn scan_json_object_keys(source: &str) -> BTreeSet<String> {
    scan_json_object_key_groups(source)
        .into_iter()
        .flatten()
        .collect()
}

/// 抽出文本里所有 `json!` 对象的键名**（按对象分组）**。
///
/// 与 [`scan_json_object_keys`] 的差别：后者把**所有**对象的键并成一个集合
/// （宽面用）；本函数保留「每个 `json!` 对象各自的键集合」，供**按签名**
/// 识别工具事件载荷用（窄面用）。
pub(super) fn scan_json_object_key_groups(source: &str) -> Vec<BTreeSet<String>> {
    let text = strip_comment_lines(source);
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut groups = Vec::new();
    let mut i = 0usize;
    while i < n {
        if bytes[i..].starts_with(b"json!") {
            let mut k = i + 5;
            while k < n && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < n && bytes[k] == b'(' {
                k += 1;
                while k < n && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < n && bytes[k] == b'{' {
                    let mut keys = BTreeSet::new();
                    scan_object_keys_at(&text, k, &mut keys);
                    groups.push(keys);
                    i = k;
                    continue;
                }
            }
            i += 5;
            continue;
        }
        i += 1;
    }
    groups
}

/// 从 `text[start]`（必须是 `{`）起做花括号配对扫描，把**深度 1** 上的
/// 「后随 `:` 的字符串字面量」收进 `out`。
fn scan_object_keys_at(text: &str, start: usize, out: &mut BTreeSet<String>) {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut depth = 0usize;
    let mut i = start;
    while i < n {
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return;
                }
            }
            b'"' => {
                // 读出一个字符串字面量。
                let mut j = i + 1;
                while j < n {
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                let content = &text[i + 1..j.min(n)];
                // 是键名？——后随（跳过空白）是 `:`。
                let mut k = j + 1;
                while k < n && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if depth == 1 && k < n && bytes[k] == b':' {
                    out.insert(content.to_string());
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
}

/// 抽出文本里所有 `payload["key"] = …` 形态的**下标写入键**。
///
/// `turns_settle.rs` 用这条路径把 `result` / `isError` 补进 `tool_result`
/// 载荷（`push_tool_result` 造的是基础载荷，错误体后补）——不扫它，窄面就
/// 漏掉这两个键。
pub(super) fn scan_index_assignment_keys(source: &str) -> BTreeSet<String> {
    let text = strip_comment_lines(source);
    let mut out = BTreeSet::new();
    for (idx, _) in text.match_indices("[\"") {
        let rest = &text[idx + 2..];
        if let Some(end) = rest.find('"') {
            let key = &rest[..end];
            let after = rest[end + 1..].trim_start();
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && after.starts_with("] =")
            {
                out.insert(key.to_string());
            }
        }
    }
    out
}

/// 读被扫描的 runtime 源文件（相对本 crate 的 `CARGO_MANIFEST_DIR`）。
pub(super) fn runtime_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-core")
        .join("src")
        .join("runtime")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()))
}

/// **扫描器自测**：注释行里的键名**不得**进实现面。
///
/// 这是「必须排除注释行」这条口径的**可执行证据**——若扫描器哪天不再排除
/// 注释，本用例会红，而不是等到实现侧真加注释时才发现口径坏了。
#[test]
fn the_scanner_excludes_comment_lines() {
    let source = r#"
        // 这行注释里的 "commentedKey" 不该被扫到
        /// 这行文档注释里的 "docKey" 也不该被扫到
        let payload = json!({"realKey": 1});
    "#;
    let scanned = scan_json_object_keys(source);
    assert!(scanned.contains("realKey"), "扫描器应扫到真实代码里的键名");
    assert!(
        !scanned.contains("commentedKey") && !scanned.contains("docKey"),
        "扫描器把注释里的键名算进了实现面：{scanned:?}——口径坏了"
    );
}
