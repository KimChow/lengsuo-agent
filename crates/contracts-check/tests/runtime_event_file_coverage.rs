//! agent-core runtime **事件构造点覆盖面** 完备性护栏 + 未覆盖文件载荷键面 ⊆ 断言。
//!
//! # 为什么有本文件
//!
//! `contracts-check` 侧原有**两个**事件载荷扫描器，各自把被扫文件清单**手工写死**：
//!
//! | 扫描器 | 覆盖文件 |
//! |---|---|
//! | `tests/session_schema_surface/runtime_payload.rs` | `turns.rs` / `turns_settle.rs` / `tool_result.rs` / `execute.rs` |
//! | `tests/session_meta_payload_surface.rs` | `sessions.rs` / `context.rs` |
//!
//! 实测（2026-10-24，base 5d2ed81）runtime 下**另有三个**文件真发事件，却
//! **零断言**守着：`turn_queue.rs`（7 处 `.emit(`）/ `skills.rs`（1）/ `retry_policy.rs`（1）。
//! 它们发出的载荷键（`ownerRequestId` / `enqueuedAtMs` / `sourceRef` / `path` /
//! `attempt` / `delayMs` / `errorKind` / `willRetry` …）在契约侧 **0 命中** ⇒
//! 实现给这些 payload 加键或改名时，契约侧**静默落后且不变红**（fail-open）。
//!
//! 更根本的问题：**扫描器的文件清单是手工维护的**——新增一个发事件的 runtime
//! 文件不会让任何测试变红。本文件同时补这两条：
//!
//! 1. **载荷键面 ⊆ 断言**：`include_str!` 编译期绑定上述三份源码，扫其 `json!` 块
//!    顶层键，断言 ⊆ `RUNTIME_EVENT_PAYLOAD_KEYS`；
//! 2. **★ 覆盖面完备性护栏**：**运行期**枚举 `agent-core/src/runtime/**` 全部
//!    `*.rs`，凡源文本含 `.emit(` 的文件必须落在「已覆盖清单 ∪ 显式豁免表」内，
//!    否则 **fail-closed 变红并指名该文件**（新增发事件的文件即红）。
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
//! ★ **只守 ⊆ 与「文件被覆盖」两种关系，不锁键总数 / 不锁取值**：实现正当新增
//! 载荷键时，「登记 + 本文件」两处同步即绿。
//!
//! ⛔ 本文件**只读**源码（`include_str!` 编译期绑定 + `read_dir` 只读枚举），
//! 不改 `agent-core/**` 的任何实现。

use contracts_check::RUNTIME_EVENT_PAYLOAD_KEYS;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------- 编译期绑定：三个原先零守护的发出事件文件 ----------

/// `turn_queue.rs`：投递队列的 `turn.{steer,queued}.enqueued` /
/// `turn.queue.expired` / `turn.{steer,queued}.consumed` 载荷，以及排队项消费时
/// 补发的 `user` 载荷。D5 把排队项 payload 的键名定死
///（`ownerRequestId` / `kind` / `text` / `enqueuedAtMs`）。
const TURN_QUEUE_SOURCE: &str = include_str!("../../agent-core/src/runtime/turn_queue.rs");

/// `skills.rs`：技能注入段落的 `SKILL_INJECTED_EVENT` 载荷（来源标记 + 段落内容）。
const SKILLS_SOURCE: &str = include_str!("../../agent-core/src/runtime/skills.rs");

/// `retry_policy.rs`：模型调用重试的 `MODEL_RETRY_EVENT` 载荷（尝试次数 / 退避延时
/// / 错误类别 / 是否还会重试）。
const RETRY_POLICY_SOURCE: &str = include_str!("../../agent-core/src/runtime/retry_policy.rs");

/// 被本文件扫描的三份源文件：`(文件名, 源码文本)`。
/// 文件名只用于断言消息（红了要能一眼看出是哪份实现落后）。
const NEW_SOURCES: &[(&str, &str)] = &[
    ("turn_queue.rs", TURN_QUEUE_SOURCE),
    ("skills.rs", SKILLS_SOURCE),
    ("retry_policy.rs", RETRY_POLICY_SOURCE),
];

// ---------- 覆盖面清单 ----------

/// 契约侧**已知被扫描**的 runtime 源文件（相对 `runtime/` 的路径，`/` 分隔）。
///
/// 与上表两个既有扫描器 + 本文件保持一致：工具事件面 4 份、会话元信息面 2 份、
/// 本文件 3 份。其中 `tool_result.rs` 本身不调 `.emit(`（payload 由
/// `push_tool_result` 造、别处发出），但它**被契约侧扫描**，故一并列名。
///
/// ⚠️ 新增发事件的 runtime 文件 ⇒ **必须同时**为该文件补一个扫描器并列入此处，
/// 否则本文件末尾的「覆盖面完备性」用例变红。
const COVERED_RUNTIME_FILES: &[&str] = &[
    "context.rs",
    "execute.rs",
    "retry_policy.rs",
    "sessions.rs",
    "skills.rs",
    "tool_result.rs",
    "turn_queue.rs",
    "turns.rs",
    "turns_settle.rs",
];

/// 覆盖面护栏的**显式豁免表**：确实发事件、但经论证**不便**做载荷键面契约的文件。
///
/// 当前为空——即「runtime 下每个 `.emit(` 的文件都有契约侧扫描器守着」。
/// 往这里加项须写明理由（为什么该文件的载荷键面可以不受契约约束）；
/// 豁免表是**有据的逃逸口**，不是「红了就塞进去」的垃圾桶。
const EMIT_FILE_EXEMPT: &[&str] = &[];

/// `runtime/` 目录（相对本 crate 的 `CARGO_MANIFEST_DIR`）。
fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-core")
        .join("src")
        .join("runtime")
}

/// 递归枚举 `dir` 下全部 `*.rs`，产出 `(相对 `runtime/` 的路径, 绝对路径)`。
///
/// ⚠️ **目录读不到必须 `panic!`**：路径写错 / 权限不足时若静默返回空集，
/// 覆盖面护栏会「因为一个文件都没枚举到」而**假绿**——那正是 fail-closed
/// 护栏最危险的失效模式。子目录（`compaction/` / `sessions/`）一并递归：
/// 那些目录里若新增发事件的文件，同样不应逃过本护栏。
fn collect_runtime_rs_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| {
        panic!(
            "读不到 runtime 目录 {}: {e}——覆盖面护栏会因假绿而失效",
            dir.display()
        )
    });
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|e| panic!("读 runtime 目录 {} 的条目失败: {e}", dir.display()));
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_runtime_rs_files(&path, &format!("{prefix}{name}/"), out);
        } else if name.ends_with(".rs") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// 源文本里是否出现 `.emit(` 调用（先排除注释行：注释里提到 `.emit(` 不算发事件）。
fn emits_events(source: &str) -> bool {
    strip_comment_lines(source).contains(".emit(")
}

// ---------- 扫描基元（口径同 `session_schema_surface/scanner.rs`） ----------

/// 一行是不是注释：去掉缩进后以 `//` 开头。
///
/// 必须排除：`turn_queue.rs` 的 `QueuedItem::payload` 文档注释里就写着
/// `ownerRequestId` / `kind` / `text` / `enqueuedAtMs` 四个键名——不排除注释，
/// 注释里的键名会被误算进**实现面**（「注释里的键名被当成实现面」的坑）。
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
    let text = strip_comment_lines(source);
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut out = BTreeSet::new();
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
                    scan_object_keys_at(&text, k, &mut out);
                    i = k;
                    continue;
                }
            }
            i += 5;
            continue;
        }
        i += 1;
    }
    out
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
    let contracted: BTreeSet<String> = RUNTIME_EVENT_PAYLOAD_KEYS
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    let scanned = scan_json_object_keys(source);
    let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
    assert!(
        unregistered.is_empty(),
        "{file} 发出但 RUNTIME_EVENT_PAYLOAD_KEYS 未登记的 payload 键（差集）\
         {unregistered:?}——契约落后于实现，先补登记再合并"
    );
}

// ---------- 断言 ----------

/// `turn_queue.rs` 扫出的 payload 键 **⊆** `RUNTIME_EVENT_PAYLOAD_KEYS`。
#[test]
fn turn_queue_payload_keys_are_subset_of_the_registered_contract() {
    assert_subset_of_registered("turn_queue.rs", TURN_QUEUE_SOURCE);
}

/// `skills.rs` 扫出的 payload 键 **⊆** `RUNTIME_EVENT_PAYLOAD_KEYS`。
#[test]
fn skills_payload_keys_are_subset_of_the_registered_contract() {
    assert_subset_of_registered("skills.rs", SKILLS_SOURCE);
}

/// `retry_policy.rs` 扫出的 payload 键 **⊆** `RUNTIME_EVENT_PAYLOAD_KEYS`。
#[test]
fn retry_policy_payload_keys_are_subset_of_the_registered_contract() {
    assert_subset_of_registered("retry_policy.rs", RETRY_POLICY_SOURCE);
}

/// **防空扫描**：三份源文件各自扫到的键集合**非空**。
///
/// 没有这一条，`include_str!` 路径写错或扫描口径改坏时会「因为什么都没扫到」
/// 而假绿——这正是 ⊆ 类断言最危险的失效模式。
#[test]
fn every_new_source_yields_a_non_empty_payload_key_set() {
    for (file, source) in NEW_SOURCES {
        let scanned = scan_json_object_keys(source);
        assert!(
            !scanned.is_empty(),
            "{file} 一个 payload 键名都没扫到——扫描口径或文件路径坏了，⊆ 断言会假绿"
        );
    }
}

/// **★ 覆盖面完备性护栏**：runtime 目录里每个含 `.emit(` 的文件都在
/// 「已覆盖清单 ∪ 豁免表」内。
///
/// 这是本卡的核心防线：契约侧的扫描器清单是**手工**维护的，新增一个发事件的
/// runtime 文件不会让任何既有测试变红 ⇒ 那个文件的事件载荷键面就**无人守护**。
/// 本用例把这条通道堵死——**fail-closed 并指名该文件**。
#[test]
fn every_runtime_file_that_emits_events_is_covered_or_exempt() {
    let dir = runtime_dir();
    let mut files = Vec::new();
    collect_runtime_rs_files(&dir, "", &mut files);
    assert!(
        !files.is_empty(),
        "{} 下一个 *.rs 都没枚举到——目录路径坏了，本护栏会假绿",
        dir.display()
    );

    // 清单里的文件名必须真实存在：名字写错 / 文件被改名 ⇒ 护栏同样变红
    //（否则「已覆盖」是个空头承诺，与 fail-open 同罪）。
    let present: BTreeSet<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
    for covered in COVERED_RUNTIME_FILES {
        assert!(
            present.contains(covered),
            "覆盖清单里的 {covered} 在 {} 下不存在（改名了？）——清单与实测对不上",
            dir.display()
        );
    }

    let mut emitters = Vec::new();
    for (rel, path) in &files {
        let source = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("读不到 runtime 源文件 {}: {e}", path.display()));
        if emits_events(&source) {
            emitters.push(rel.clone());
        }
    }
    assert!(
        !emitters.is_empty(),
        "{} 下没有任何文件含 `.emit(`——枚举或读取坏了，本护栏会假绿",
        dir.display()
    );

    let uncovered: Vec<&String> = emitters
        .iter()
        .filter(|f| {
            !COVERED_RUNTIME_FILES.contains(&f.as_str()) && !EMIT_FILE_EXEMPT.contains(&f.as_str())
        })
        .collect();
    assert!(
        uncovered.is_empty(),
        "runtime 下这些文件发事件（含 `.emit(`）却不在「已覆盖 ∪ 豁免」清单里：{uncovered:?}\
         ——新增发事件的 runtime 文件必须**同时**补契约侧载荷扫描器，否则契约侧静默落后\
         （fail-open）"
    );
}

/// **扫描器自测**：注释行里的键名**不得**进实现面。
///
/// 这是「必须排除注释行」这条口径的**可执行证据**——若扫描器哪天不再排除注释，
/// 本用例会红：`turn_queue.rs` 的 `payload` 文档注释里就写着那四个键名，
/// 不排除注释就会拿注释冒充实现面。
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
