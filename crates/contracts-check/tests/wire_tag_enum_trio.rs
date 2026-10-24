//! 三个 `#[serde(tag)]` 枚举的 wire 判别值拼写面守卫（W2）。
//!
//! # 这条守卫治什么病
//!
//! 仓内三个 `#[serde(tag = "type")]` 枚举的变体 wire 值是**跨进程对端**直接消费
//! 的线上词，但 contracts-check 契约侧对它们的拼写**零字面量钉住**（2026-10-24，
//! base d7737c17 实测：全 tests/ 对 `"type":"text"` / `all_on_same_line` /
//! `all_within_lines` 零命中；`SearchMatchMode` 零出现；`task.created` 仅在
//! `event_surface.rs` 里作为「未登记 kind 应被拒绝」的负样本出现）：
//!
//! | 枚举 | 源 | wire 值机制 | 期望 wire 值 |
//! |---|---|---|---|
//! | `ContentBlock` | `crates/agent-acp/src/methods.rs` | 容器 `rename_all = "snake_case"` | Text⇒`text`、Resource⇒`resource` |
//! | `SearchMatchMode` | `crates/agent-memory/src/search.rs` | 容器 `rename_all = "snake_case"` | Any⇒`any`、AllOnSameLine⇒`all_on_same_line`、AllWithinLines⇒`all_within_lines` |
//! | `TaskEvent` | `crates/agent-tasks/src/event.rs` | 变体级显式 `rename`（点分协议） | Created⇒`task.created`、Updated⇒`task.updated` |
//!
//! round-trip 测试不构成契约钉住：序列化/反序列化双边同走 serde，`rename_all`
//! 被改、变体被改名时依旧自洽照绿，炸的只有跨进程对端。拼写一漂，对端静默失聪。
//!
//! # 做法与口径
//!
//! `contracts-check` 的 dev-dependencies **没有** `agent-acp` / `agent-memory` /
//! `agent-tasks`，且 `Cargo.toml` 属共享区不得改，故与同类守卫
//! （`tests/session_update_tag_surface.rs`、`tests/task_status_drift_guard.rs`）
//! 相同，走**源码文本扫描**：`include_str!` 编译期绑定三份源文件文本（源文件被
//! 移动 = 编译期红），另用 `CARGO_MANIFEST_DIR` 相对路径断言文件仍在该处。
//!
//! - **只守关系与拼写，不锁行数**：变体集 ⇄ 期望表双向集合相等是目的
//!   （新变体登记 wire 值后即绿），不算「锁数量必红」反例。
//! - 期望表是**手写的冻结真相**：不内置 PascalCase⇒snake_case 派生函数去推导
//!   wire 值（推导跟着源码一起漂，钉不住）。两个 rename_all 枚举靠「容器属性
//!   钉死 + 变体级 rename 禁令」钉住派生输入；TaskEvent 靠逐变体 rename 直读。
//! - 扫描器**防空**：枚举头找不到 / 花括号不配平 ⇒ `Err`（fail-closed 红）。
//! - 合成样本自证：`synthetic_samples_catch_three_drifts` 用假源码证明「tag 键
//!   被改 / 变体改名 / TaskEvent 变体丢 rename」三类漂移都真能红（真源不为测试而改）。

use std::collections::BTreeSet;
use std::path::Path;

/// agent-acp 源文本（**编译期**绑定：源文件改动即触发重编）。
const METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");
/// agent-memory 源文本（**编译期**绑定）。
const SEARCH_SOURCE: &str = include_str!("../../agent-memory/src/search.rs");
/// agent-tasks 源文本（**编译期**绑定）。
const EVENT_SOURCE: &str = include_str!("../../agent-tasks/src/event.rs");

/// (标签, 相对 `CARGO_MANIFEST_DIR` 路径)：存在性断言用（防 include_str! 绑到旧路径后假绿）。
const SOURCE_FILES: &[(&str, &str)] = &[
    ("methods.rs", "../agent-acp/src/methods.rs"),
    ("search.rs", "../agent-memory/src/search.rs"),
    ("event.rs", "../agent-tasks/src/event.rs"),
];

/// 枚举声明头（含 `{`，供行级定位；同名枚举在他 crate 另有，但本文件只看
/// include_str! 绑定的文本，互不可见）。
const CONTENT_BLOCK_HEADER: &str = "pub enum ContentBlock {";
/// 枚举声明头。
const SEARCH_MATCH_MODE_HEADER: &str = "pub enum SearchMatchMode {";
/// 枚举声明头。
const TASK_EVENT_HEADER: &str = "pub enum TaskEvent {";

/// **钉死的判别键**：三个枚举共用 `type`。
const TAG_KEY: &str = "type";
/// rename_all 枚举（ContentBlock / SearchMatchMode）钉死的派生规则。
const SNAKE_CASE: &str = "snake_case";
/// TaskEvent 变体 rename 的统一前缀（点分协议）。
const TASK_EVENT_PREFIX: &str = "task.";

/// `ContentBlock` 期望表：变体名 ⇒ wire tag（手写冻结真相）。
///
/// 这张表就是契约：正当扩充新变体 = 同步加一行表项（不红）；
/// 单侧改动（源加了变体不登记 / 表登记了不存在的变体）⇒ 红。
const CONTENT_BLOCK_TAGS: &[(&str, &str)] = &[("Text", "text"), ("Resource", "resource")];

/// `SearchMatchMode` 期望表：变体名 ⇒ wire tag（手写冻结真相；结构体变体
/// `AllWithinLines { line_count }` 的变体名同样取 `AllWithinLines`）。
const SEARCH_MATCH_MODE_TAGS: &[(&str, &str)] = &[
    ("Any", "any"),
    ("AllOnSameLine", "all_on_same_line"),
    ("AllWithinLines", "all_within_lines"),
];

/// `TaskEvent` 期望表：变体名 ⇒ wire tag。此枚举**无 rename_all**，wire 值来自
/// 变体级显式 rename，源里必须逐字出现——变体级 rename 在这里是**合法机制**。
const TASK_EVENT_TAGS: &[(&str, &str)] =
    &[("Created", "task.created"), ("Updated", "task.updated")];

// ---- 扫描器（只依赖源码文本；真源断言与合成样本自证走同一批函数）----

/// 枚举声明头的行号（0 基）；找不到 ⇒ None（fail-closed 由调用方变红）。
fn enum_header_at(source: &str, header: &str) -> Option<usize> {
    source
        .lines()
        .position(|l| l.trim_start().starts_with(header))
}

/// 枚举体逐行展开：`(相对枚举体的花括号深度, trim 后的行)`，深度**相对枚举体**
/// 计（变体级属性行/变体声明为 0，结构体变体的字段为 ≥1），吃到配平的收尾 `}`
/// 为止（该行不入列）；行数耗尽仍未配平（或多出 `}`）⇒ Err——花括号不配平必须红。
fn enum_body_lines<'a>(source: &'a str, header: &str) -> Result<Vec<(usize, &'a str)>, String> {
    let header_at = enum_header_at(source, header)
        .ok_or_else(|| format!("源码里找不到 `{header}`——形态变了"))?;
    let mut out = Vec::new();
    let mut depth = 1usize; // 头行的 `{` 已开一层
    for line in source.lines().skip(header_at + 1) {
        let t = line.trim();
        let before = depth - 1;
        for c in t.chars() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => return Err("枚举体花括号不配平：多出收尾 `}`".into()),
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth == 0 {
            return Ok(out); // 恰好吃到收尾 `}` 为止
        }
        out.push((before, t));
    }
    Err("枚举体到文件尾仍未配平收尾 `}`——花括号不配平".into())
}

/// 代码行：非空行、非注释（`//` / `///` / `//!` / `*` 续行）、非属性行（`#[…]`）。
fn is_code(t: &str) -> bool {
    !(t.is_empty() || t.starts_with("//") || t.starts_with('#') || t.starts_with('*'))
}

/// 行的前导标识符（`Xxx(..)` / `Xxx {` 两种形态都取到 `Xxx`）。
fn leading_ident(t: &str) -> String {
    t.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// ① 扫枚举体的**变体名**（保留源码顺序）。深度 0 的代码行必是变体声明，
/// 取不出前导标识符 ⇒ Err（形态变了 fail-closed）。
fn scan_variants(source: &str, header: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (depth, t) in enum_body_lines(source, header)? {
        if depth > 0 || !is_code(t) {
            continue; // 结构体变体字段 / 空行 / 注释 / 属性行
        }
        let name = leading_ident(t);
        if name.is_empty() {
            return Err(format!("枚举体深度 0 处出现解读不了的代码行：`{t}`"));
        }
        out.push(name);
    }
    Ok(out)
}

/// ② 扫枚举体内**变体级** `#[serde(rename = "…")]` 的属性行（只看深度 0）。
///
/// 变体级 `rename` 会让该变体 tag 偏离容器 snake_case 规则、绕过期望表 ⇒ 对
/// rename_all 枚举禁止；变体级 `rename_all` 作用于**字段**、不碰 tag ⇒ 允许。
/// 两者按 `rename` 后紧跟 `=` 还是 `_` 区分（`has_bare_rename_attr`）。
fn scan_variant_level_renames(source: &str, header: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (depth, t) in enum_body_lines(source, header)? {
        if depth > 0 || !t.starts_with("#[") {
            continue; // 字段级属性在深度 ≥1：只动字段名，不偏离 tag，不看
        }
        if has_bare_rename_attr(t) {
            out.push(t.to_owned());
        }
    }
    Ok(out)
}

/// ③ 变体名 ⇒ 紧邻其上的变体级 `rename` 取值（深度 0 属性行累积、遇变体名消费）。
///
/// TaskEvent 用：rename 是它的合法 wire 值机制，**每**变体都必须恰好挂一个。
fn scan_variants_with_rename(
    source: &str,
    header: &str,
) -> Result<Vec<(String, Option<String>)>, String> {
    let mut out = Vec::new();
    let mut pending: Option<String> = None;
    for (depth, t) in enum_body_lines(source, header)? {
        if depth > 0 {
            continue; // 结构体变体的字段与收尾 `},`
        }
        if t.starts_with("#[") {
            if has_bare_rename_attr(t) {
                pending = attr_value(t, "rename");
            }
            continue;
        }
        if !is_code(t) {
            continue;
        }
        let name = leading_ident(t);
        if name.is_empty() {
            return Err(format!("枚举体深度 0 处出现解读不了的代码行：`{t}`"));
        }
        out.push((name, pending.take()));
    }
    Ok(out)
}

/// ④ 从枚举声明上方属性行取 `(tag = "…", rename_all = "…")`；`rename_all`
/// 缺席 ⇒ None（TaskEvent 靠它钉「不得出现 rename_all」）。属性行里找不到
/// 带 tag 的 ⇒ Err（fail-closed：判别键被摘掉/形态变了都算红）。
fn scan_tag_attr(source: &str, header: &str) -> Result<(String, Option<String>), String> {
    let header_at = enum_header_at(source, header)
        .ok_or_else(|| format!("源码里找不到 `{header}`——形态变了"))?;
    let above: Vec<&str> = source.lines().take(header_at).collect();
    for line in above.into_iter().rev() {
        let t = line.trim();
        if !t.starts_with("#[") {
            if t.starts_with("//") || t.is_empty() {
                continue; // 文档注释可垫在属性行之上
            }
            break; // 撞到其它代码即停（再往上不是本枚举的属性了）
        }
        if let Some(tag) = attr_value(t, "tag") {
            return Ok((tag, attr_value(t, "rename_all")));
        }
    }
    Err(format!(
        "`{header}` 上方找不到带 `tag = \"…\"` 的 serde 属性行——判别键被摘或形态变了"
    ))
}

/// 属性行里是否出现**裸** `rename`（其后跳过空白紧跟 `=`）；`rename_all` /
/// `rename_all_fields` 的 `rename` 后是 `_`，不算，也不许被前缀标识符吞掉。
fn has_bare_rename_attr(attr: &str) -> bool {
    let bytes = attr.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = attr[from..].find("rename") {
        let at = from + rel;
        let after = at + "rename".len();
        let left_ok = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let mut j = after;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if left_ok && j < bytes.len() && bytes[j] == b'=' {
            return true;
        }
        from = after;
    }
    false
}

/// 从属性行取 `<key> = "<value>"` 的 value（引号内精确文本；键名前不许是
/// 标识符字符，防误吸 `xx_tag = "…"` 这类）。
fn attr_value(attr: &str, key: &str) -> Option<String> {
    let needle = format!("{key} = \"");
    let mut from = 0usize;
    while let Some(rel) = attr[from..].find(&needle) {
        let at = from + rel;
        let left_ok = at == 0
            || !attr[..at]
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let end = attr[at + needle.len()..]
            .find('"')
            .map(|e| at + needle.len() + e);
        if left_ok {
            return end.map(|e| attr[at + needle.len()..e].to_owned());
        }
        from = at + needle.len();
    }
    None
}

// ---- 集合判定（真源断言与合成样本自证共用）----

/// 期望表的变体名集合。
fn table_variants(table: &[(&'static str, &'static str)]) -> BTreeSet<&'static str> {
    table.iter().map(|(v, _)| *v).collect()
}

/// 期望表的 wire 值集合。
fn table_tags(table: &[(&'static str, &'static str)]) -> BTreeSet<&'static str> {
    table.iter().map(|(_, t)| *t).collect()
}

/// 期望表自检：非空、变体名与 wire 值各自无重复（两个变体撞同一个线上词会
/// 让反解歧义，先在表侧红）。
fn assert_table_shape(table: &[(&'static str, &'static str)], label: &str) {
    assert!(!table.is_empty(), "{label} 期望表被清空——契约真相不许删");
    assert_eq!(
        table_variants(table).len(),
        table.len(),
        "{label} 期望表变体名有重复"
    );
    assert_eq!(
        table_tags(table).len(),
        table.len(),
        "{label} 期望表 wire 值有重复（两个变体撞同一个线上词，反解歧义）"
    );
}

/// 双向差集：`(实际有而期望缺的多余项, 期望有而实际缺的缺失项)`。
fn bidirectional_diff<'a>(
    actual: &'a [String],
    expected: &BTreeSet<&'a str>,
) -> (Vec<&'a str>, Vec<&'a str>) {
    let actual_set: BTreeSet<&str> = actual.iter().map(String::as_str).collect();
    let extra: Vec<&str> = actual_set.difference(expected).copied().collect();
    let missing: Vec<&str> = expected.difference(&actual_set).copied().collect();
    (extra, missing)
}

/// 变体集 ⇄ 期望表双向集合相等 + 项数一致（防重复项）。
fn assert_variants_match_table(
    scanned: &[String],
    table: &[(&'static str, &'static str)],
    label: &str,
) {
    let (extra, missing) = bidirectional_diff(scanned, &table_variants(table));
    assert!(
        extra.is_empty() && missing.is_empty(),
        "{label} 变体与期望表不同步：实现有而表缺 {extra:?}；表有而实现缺 {missing:?}——\
         wire tag 是对端消费的线上词，正当扩充新变体须同步登记期望表"
    );
    assert_eq!(
        scanned.len(),
        table.len(),
        "{label} 变体数与表项数不一致（有变体或表项重复）"
    );
}

// ---- 断言面 ----

/// ① **ContentBlock**：tag 键与派生规则钉死 + 变体集 ⇄ 期望表双向相等 +
/// 变体级 rename 禁令。谁改了会红：改 `tag = "type"` 或摘掉/改掉
/// `rename_all = "snake_case"`、改名/增删变体（不登记表）、给变体挂显式
/// `rename` 绕过派生规则——分别由本测试三个断言面变红。
#[test]
fn content_block_wire_tag_surface_is_pinned() {
    assert_table_shape(CONTENT_BLOCK_TAGS, "ContentBlock");

    let (tag, rename_all) =
        scan_tag_attr(METHODS_SOURCE, CONTENT_BLOCK_HEADER).expect("属性行必须能扫到");
    assert_eq!(
        tag, TAG_KEY,
        "ContentBlock 判别键漂了（实为 {tag:?}）——对端按 {TAG_KEY:?} 取内容块类型"
    );
    assert_eq!(
        rename_all.as_deref(),
        Some(SNAKE_CASE),
        "ContentBlock 派生规则漂了（实为 {:?}）——期望表按 snake_case 手写，规则一变表即失真",
        rename_all
    );

    let scanned = scan_variants(METHODS_SOURCE, CONTENT_BLOCK_HEADER).expect("枚举体必须能解析");
    assert_variants_match_table(&scanned, CONTENT_BLOCK_TAGS, "ContentBlock");

    let offenders =
        scan_variant_level_renames(METHODS_SOURCE, CONTENT_BLOCK_HEADER).expect("枚举体必须能解析");
    assert!(
        offenders.is_empty(),
        "ContentBlock 变体上出现 `rename = …`（{offenders:?}）——变体级 rename 令该变体 \
         偏离 snake_case 规则、绕过期望表；tag 拼写只能整体随规则或同步改表"
    );
}

/// ② **SearchMatchMode**：与 ① 同构（tag 键 + snake_case 派生规则 + 双向集合
/// 相等 + 变体级 rename 禁令）。结构体变体 `AllWithinLines { line_count }` 的
/// 变体名同样按 `AllWithinLines` 登记。谁改了会红：同 ① 三类，外加把 tag 派生
/// 规则改成 kebab/camel 等任何非 snake_case。
#[test]
fn search_match_mode_wire_tag_surface_is_pinned() {
    assert_table_shape(SEARCH_MATCH_MODE_TAGS, "SearchMatchMode");

    let (tag, rename_all) =
        scan_tag_attr(SEARCH_SOURCE, SEARCH_MATCH_MODE_HEADER).expect("属性行必须能扫到");
    assert_eq!(
        tag, TAG_KEY,
        "SearchMatchMode 判别键漂了（实为 {tag:?}）——memory.search 请求体的模式字段"
    );
    assert_eq!(
        rename_all.as_deref(),
        Some(SNAKE_CASE),
        "SearchMatchMode 派生规则漂了（实为 {:?}）——期望表按 snake_case 手写，规则一变表即失真",
        rename_all
    );

    let scanned = scan_variants(SEARCH_SOURCE, SEARCH_MATCH_MODE_HEADER).expect("枚举体必须能解析");
    assert_variants_match_table(&scanned, SEARCH_MATCH_MODE_TAGS, "SearchMatchMode");

    let offenders = scan_variant_level_renames(SEARCH_SOURCE, SEARCH_MATCH_MODE_HEADER)
        .expect("枚举体必须能解析");
    assert!(
        offenders.is_empty(),
        "SearchMatchMode 变体上出现 `rename = …`（{offenders:?}）——变体级 rename 令该变体 \
         偏离 snake_case 规则、绕过期望表"
    );
}

/// ③ **TaskEvent**：此枚举**无 rename_all**、靠变体级显式 rename 定 wire 值
/// （与前两个枚举相反，rename 在这里是合法机制）。断言三层：
/// 属性行恰为 `tag = "type"` 且**不得**出现容器 `rename_all`（它会静默改写
/// 无 rename 变体的 wire 值）；变体集 ⇄ 期望表双向相等；**每**变体都有
/// `task.` 前缀的显式 rename 且值 ∈ 期望表（不存在无 rename 的变体——否则裸用
/// PascalCase 变体名当 wire 值，破坏点分协议）。
#[test]
fn task_event_wire_tag_surface_is_pinned() {
    assert_table_shape(TASK_EVENT_TAGS, "TaskEvent");

    let (tag, rename_all) =
        scan_tag_attr(EVENT_SOURCE, TASK_EVENT_HEADER).expect("属性行必须能扫到");
    assert_eq!(
        tag, TAG_KEY,
        "TaskEvent 判别键漂了（实为 {tag:?}）——任务事件投影按 {TAG_KEY:?} 分派"
    );
    assert_eq!(
        rename_all, None,
        "TaskEvent 出现了容器级 rename_all（实为 {rename_all:?}）——它靠变体级显式 rename \
         定 wire 值；容器规则会静默改写无 rename 变体的 wire 词，禁止混用"
    );

    let scanned =
        scan_variants_with_rename(EVENT_SOURCE, TASK_EVENT_HEADER).expect("枚举体必须能解析");
    let names: Vec<String> = scanned.iter().map(|(n, _)| n.clone()).collect();
    assert_variants_match_table(&names, TASK_EVENT_TAGS, "TaskEvent");

    let tags = table_tags(TASK_EVENT_TAGS);
    for (name, rename) in &scanned {
        assert!(
            rename.is_some(),
            "TaskEvent 变体 {name} 丢了变体级显式 rename——无 rename 的变体会裸用 \
             PascalCase 变体名当 wire 值，破坏 task. 点分协议"
        );
        let wire = rename.as_deref().expect("上一行刚断言过");
        assert!(
            wire.starts_with(TASK_EVENT_PREFIX),
            "TaskEvent 变体 {name} 的 rename 值 {wire:?} 不带 {TASK_EVENT_PREFIX:?} 前缀——点分协议破坏"
        );
        assert!(
            tags.contains(wire),
            "TaskEvent 变体 {name} 的 rename 值 {wire:?} 不在期望表 {tags:?} 里——wire 词须逐字登记"
        );
    }
}

/// ④ **路径存在性钉死**：三份源文件必须仍在该处（include_str! 绑定的是编译期
/// 路径，文件被移走会让本守卫整体静默失效）。
#[test]
fn source_files_still_exist() {
    for (label, rel) in SOURCE_FILES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        assert!(
            path.exists(),
            "{label} 被移动了（期望在 {}）——include_str! 绑定的是旧路径，本守卫会静默失效",
            path.display()
        );
    }
}

/// ⑤ **扫描器防空**：枚举头被移走 ⇒ 全部扫描器 Err（fail-closed，不是 Ok(空)
/// 假绿）；枚举体花括号不配平 ⇒ Err 而非 panic 漏判。
#[test]
fn scanner_fails_closed_on_broken_source() {
    let multiline = "\
#[serde(tag = \"type\")]
pub enum Demo {
    A,
    B {
        field: u8,
    },
}
";
    let header = "pub enum Demo {";
    assert!(!scan_variants(multiline, header)
        .expect("基线可解析")
        .is_empty());

    // 枚举头被改走 ⇒ 三个扫描器都必须 Err，不许静默空过。
    let header_gone = multiline.replace(header, "pub enum Demo2 {");
    assert!(
        scan_variants(&header_gone, header).is_err(),
        "头没了必须 Err"
    );
    assert!(
        scan_variants_with_rename(&header_gone, header).is_err(),
        "头没了必须 Err"
    );
    assert!(
        scan_tag_attr(&header_gone, header).is_err(),
        "头没了必须 Err"
    );

    // 吞掉结构体变体的收尾 `}` ⇒ 配平检测必须 Err。
    let unbalanced = multiline.replace("    },\n", "");
    assert_ne!(unbalanced, multiline, "替换没生效，样本构造有误");
    assert!(
        scan_variants(&unbalanced, header).is_err(),
        "花括号不配平必须 Err 红"
    );
}

/// ⑥ **合成样本自证**：用假源码证明三类漂移都真能红——tag 键被改 /
/// rename_all 枚举变体改名 / TaskEvent 变体丢 rename，外加 TaskEvent rename
/// 值改拼必须因不在期望表被抓出（先例：session_update_tag_surface.rs 的自证段）。
#[test]
fn synthetic_samples_catch_three_drifts() {
    // —— rename_all 形态基线（ContentBlock 形状：snake_case、无变体级 rename）。
    let base = "\
#[serde(tag = \"type\", rename_all = \"snake_case\")]
pub enum ContentBlock {
    /// 文本块。
    Text(TextContent),
    /// 资源块。
    Resource(Value),
}
";
    let header = "pub enum ContentBlock {";
    let (tag, rule) = scan_tag_attr(base, header).expect("基线样本应有 serde 属性行");
    assert_eq!(tag, TAG_KEY, "基线应读出钉死的键");
    assert_eq!(rule.as_deref(), Some(SNAKE_CASE), "基线应读出钉死的规则");
    assert!(!scan_variants(base, header).expect("基线可解析").is_empty());

    // 漂移 A：tag 键被改 ⇒ 扫出不同的键；被整个摘掉 ⇒ Err（fail-closed）。
    let key_changed = base.replace("tag = \"type\"", "tag = \"kind\"");
    assert_ne!(key_changed, base, "替换没生效，样本构造有误");
    let (changed, _) = scan_tag_attr(&key_changed, header).expect("属性行仍在");
    assert_ne!(changed, TAG_KEY, "tag 键被改后必须扫出不同的键");
    let no_tag = base.replace("tag = \"type\", ", "");
    assert!(
        scan_tag_attr(&no_tag, header).is_err(),
        "tag 属性被摘后必须 Err 红，不许静默通过"
    );

    // 漂移 B：rename_all 枚举的变体改名 ⇒ 双向差集必须同时报「多余 + 缺失」。
    let renamed = base.replace("Text(TextContent),", "Label(TextContent),");
    assert_ne!(renamed, base, "替换没生效，样本构造有误");
    let variants = scan_variants(&renamed, header).expect("renamed 可解析");
    let (extra, missing) = bidirectional_diff(&variants, &table_variants(CONTENT_BLOCK_TAGS));
    assert_eq!(extra, vec!["Label"], "改名后的变体必须作为多余项报出");
    assert_eq!(missing, vec!["Text"], "旧名必须作为缺失项报出");

    // —— TaskEvent 形态基线（无 rename_all、逐变体显式 rename、含结构体变体字段）。
    let events = "\
#[serde(tag = \"type\")]
pub enum TaskEvent {
    /// 任务创建。
    #[serde(rename = \"task.created\")]
    Created {
        task_id: String,
    },
    /// 任务更新。
    #[serde(rename = \"task.updated\")]
    Updated {
        task_id: String,
    },
}
";
    let event_header = "pub enum TaskEvent {";
    let (etag, erule) = scan_tag_attr(events, event_header).expect("基线可解析");
    assert_eq!(etag, TAG_KEY, "基线应读出钉死的键");
    assert!(erule.is_none(), "TaskEvent 基线不得带 rename_all");
    assert_eq!(
        scan_variants_with_rename(events, event_header).expect("基线可解析"),
        vec![
            ("Created".to_owned(), Some("task.created".to_owned())),
            ("Updated".to_owned(), Some("task.updated".to_owned())),
        ],
        "TaskEvent 解析口径变了：变体名与 rename 值必须成对读出"
    );

    // 漂移 C：TaskEvent 变体丢了 rename ⇒ (名字, None)，必须能抓出。
    let lost = events.replace("    #[serde(rename = \"task.created\")]\n", "");
    assert_ne!(lost, events, "替换没生效，样本构造有误");
    let lost_scan = scan_variants_with_rename(&lost, event_header).expect("lost 可解析");
    let lost_created = lost_scan
        .iter()
        .find(|(n, _)| n.as_str() == "Created")
        .expect("Created 仍在枚举体里");
    assert!(
        lost_created.1.is_none(),
        "丢了 rename 的变体必须以 (名字, None) 报出：{lost_scan:?}"
    );

    // rename 值改拼 ⇒ 直读得出新词、且因不在期望表被抓出。
    let respelled = events.replace("task.created", "task.create");
    assert_ne!(respelled, events, "替换没生效，样本构造有误");
    let respelled_scan = scan_variants_with_rename(&respelled, event_header).expect("可解析");
    let wire = respelled_scan
        .iter()
        .find(|(n, _)| n.as_str() == "Created")
        .and_then(|(_, r)| r.as_deref());
    assert_eq!(wire, Some("task.create"), "rename 改拼必须被直读出来");
    let tags = table_tags(TASK_EVENT_TAGS);
    assert!(
        wire.is_some_and(|w| !tags.contains(w)),
        "改拼后的 rename 值不在期望表内——必须抓出"
    );

    // 真源必须扫得到东西（否则上面各集合断言全是空对空）。
    assert!(!scan_variants(METHODS_SOURCE, CONTENT_BLOCK_HEADER)
        .expect("真源可解析")
        .is_empty());
    assert!(!scan_variants(SEARCH_SOURCE, SEARCH_MATCH_MODE_HEADER)
        .expect("真源可解析")
        .is_empty());
    assert!(!scan_variants(EVENT_SOURCE, TASK_EVENT_HEADER)
        .expect("真源可解析")
        .is_empty());
}
