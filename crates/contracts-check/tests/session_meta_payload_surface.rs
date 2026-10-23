//! agent-core runtime **会话元信息事件载荷键面** ⊆ 断言。
//!
//! # 为什么有本文件
//!
//! `tests/session_schema_surface/runtime_payload.rs` 只编译期绑定 `runtime/` 下
//! 四份**工具事件**构造点（`turns.rs` / `turns_settle.rs` / `tool_result.rs` /
//! `execute.rs`），并在那里**明写**未覆盖 `sessions.rs` / `context.rs` 发的
//! `session_update` / `user` / `warning` 等载荷。
//!
//! 那两份文件发的载荷同样进线上事件流（WS 推送 / 回放 / 壳侧消费），却**没有**
//! 断言守着——实现给 `session_update` 加键或改名时，契约侧静默落后且不变红
//! （fail-open）。本文件把这条缺口补上：**编译期绑定这两份源码，断言扫出的
//! payload 键集合 ⊆ 契约登记面**。
//!
//! # 口径（与 `tests/session_schema_surface/scanner.rs` 同一口径）
//!
//! 手写字符扫描（⛔ 不引正则依赖）：
//! 1. 先排除注释行（`///` / `//` 里出现键名不得被算成实现面）；
//! 2. 找 `json!` 记号（`serde_json::json!` 一并命中——搜索的是 `json!` 子串）；
//! 3. 其后（跳过空白）必须是 `(`，再后（跳过空白）必须是 `{`；
//! 4. 从 `{` 起做花括号配对扫描，只收**深度 1**（最外层对象）上「后随 `:` 的
//!    字符串字面量」——嵌套对象的键不是 payload 顶层键；嵌套的 `json!` 会被
//!    主循环**单独**再扫一遍，故它们的键也会进结果集。
//!
//! ★ **只守 ⊆ 关系，不锁键总数 / 不锁取值**：实现正当新增载荷键时，把新键登记进
//! `RUNTIME_EVENT_PAYLOAD_KEYS` 后本文件自然变绿。
//!
//! ⛔ 本文件**只读**源码（`include_str!` 编译期绑定，改动即触发重编），
//! 不改 `agent-core/**` 的任何实现。

use contracts_check::RUNTIME_EVENT_PAYLOAD_KEYS;
use std::collections::BTreeSet;

/// `sessions.rs`：建会话 / 更新会话 / 状态迁移路径发的 `session_update` /
/// `warning` / `memory_update` 载荷（`json!` 块分别在 `sessions.rs` 的
/// `create_session` / `execute_memory_tool` / `update_session` / `set_status`）。
const SESSIONS_SOURCE: &str = include_str!("../../agent-core/src/runtime/sessions.rs");

/// `context.rs`：回合上下文组装路径发的 `warning` / `user` 载荷（技能注入被拒、
/// 记忆重组失败、专家人格取用失败三条告警，以及压缩产物落库的 `user`）。
const CONTEXT_SOURCE: &str = include_str!("../../agent-core/src/runtime/context.rs");

/// 被扫描的两份**会话元信息**源文件：`(文件名, 源码文本)`。
///
/// 文件名只用于断言消息（红了要能一眼看出是哪份实现落后）。
const META_SOURCES: &[(&str, &str)] = &[
    ("sessions.rs", SESSIONS_SOURCE),
    ("context.rs", CONTEXT_SOURCE),
];

/// 把登记常量转成集合。
fn registered(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_string()).collect()
}

// ---------- 扫描基元（口径同 `session_schema_surface/scanner.rs`） ----------

/// 一行是不是注释：去掉缩进后以 `//` 开头。
///
/// 必须排除：被扫描的文件里有大量 `///` 文档注释，注释里出现的键名（如
/// `"kind"` / `"reason"`）会被误算进**实现面**——那正是「注释里的键名被当成
/// 实现面」的坑。
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

/// 抽出文本里**所有** `json!` 对象的顶层键名（并成一个集合）。
fn scan_json_object_keys(source: &str) -> BTreeSet<String> {
    scan_json_object_key_groups(source)
        .into_iter()
        .flatten()
        .collect()
}

/// 抽出文本里所有 `json!` 对象的键名，**按对象分组**（每个 `json!` 块各自的键集）。
fn scan_json_object_key_groups(source: &str) -> Vec<BTreeSet<String>> {
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
                // 读出一个字符串字面量（跳过 `\` 转义）。
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

// ---------- 断言辅助 ----------

/// 断言某份源文件扫出的 payload 键集合 **⊆** 登记面（差集必须为空）。
///
/// 红了说明：该文件在事件 payload 里发了新键（或改了键名），而契约侧没登记。
fn assert_subset_of_registered(file: &str, source: &str) {
    let contracted = registered(RUNTIME_EVENT_PAYLOAD_KEYS);
    let scanned = scan_json_object_keys(source);
    let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
    assert!(
        unregistered.is_empty(),
        "{file} 发出但 RUNTIME_EVENT_PAYLOAD_KEYS 未登记的 payload 键（差集）\
         {unregistered:?}——契约落后于实现，先补登记再合并"
    );
}

/// 登记项必须**唯一**。
fn assert_unique(name: &str, items: &[&str]) {
    for (i, item) in items.iter().enumerate() {
        assert!(!items[..i].contains(item), "{name} 出现重复项：{item}");
    }
}

// ---------- 断言 ----------

/// `sessions.rs` 扫出的 payload 键 **⊆** `RUNTIME_EVENT_PAYLOAD_KEYS`。
#[test]
fn sessions_payload_keys_are_subset_of_the_registered_contract() {
    assert_subset_of_registered("sessions.rs", SESSIONS_SOURCE);
}

/// `context.rs` 扫出的 payload 键 **⊆** `RUNTIME_EVENT_PAYLOAD_KEYS`。
#[test]
fn context_payload_keys_are_subset_of_the_registered_contract() {
    assert_subset_of_registered("context.rs", CONTEXT_SOURCE);
}

/// **防空扫描**：两份文件各自扫到的键集合**非空**。
///
/// 没有这一条，`include_str!` 路径写错或扫描口径改坏时会「因为什么都没扫到」
/// 而假绿——这正是 ⊆ 类断言最危险的失效模式。
#[test]
fn both_meta_sources_yield_a_non_empty_payload_key_set() {
    for (file, source) in META_SOURCES {
        let scanned = scan_json_object_keys(source);
        assert!(
            !scanned.is_empty(),
            "{file} 一个 payload 键名都没扫到——扫描口径或文件路径坏了，⊆ 断言会假绿"
        );
    }
}

/// 登记面 `RUNTIME_EVENT_PAYLOAD_KEYS` **唯一且有序**（按 `&str` 字节序）。
///
/// 与既有 `assert_unique` 口径一致：常量是人工维护的，重复项会让「差集为空」
/// 这类断言掩盖掉「其实漏登了另一个键」。
#[test]
fn registered_meta_payload_keys_are_unique_and_sorted() {
    assert_unique("RUNTIME_EVENT_PAYLOAD_KEYS", RUNTIME_EVENT_PAYLOAD_KEYS);
    let actual: Vec<String> = RUNTIME_EVENT_PAYLOAD_KEYS
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    let mut expected = actual.clone();
    expected.sort();
    assert_eq!(
        actual, expected,
        "RUNTIME_EVENT_PAYLOAD_KEYS 未按字节序排序——常量是人工维护的，排序能让漏登一眼可见"
    );
}

/// **扫描器自测**：注释行里的键名**不得**进实现面。
///
/// 这是「必须排除注释行」这条口径的**可执行证据**——若扫描器哪天不再排除注释，
/// 本用例会红，而不是等到实现侧真加注释时才发现口径坏了。
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
