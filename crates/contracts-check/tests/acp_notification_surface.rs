//! ACP **通知方法面**（单向、无响应）的结构化登记断言（W2 · lane proto）。
//!
//! # 这条断言治什么病
//!
//! ACP 方法分两种**帧形状**（JSON-RPC 2.0）：**请求-响应**（`Message::Request`，
//! 发出去必须配一个 `Response` 回来）与**通知**（`Message::Notification`，
//! 发出去**永不配响应**）。此前「哪些 ACP 方法是通知」这件事**只由三个汉字
//! 承载**（散文注释里的「（通知）」）：实现侧没有通知族清单常量，契约侧
//! `ACP_METHODS` 里那两行也只是**行尾散文注释** ⇒ 被改写 / 挪位 / 重排就
//! **静默丢失**，没有任何机器可判定的断言。
//!
//! # 为什么它是**承重**语义
//!
//! 网关的 ACP 转发是**请求-响应**（`crates/agent-gateway/src/acp.rs` 的
//! `forward_action` 发 Rpc 请求帧、按 id 等响应帧配对）。把**通知**方法投进
//! 那条路径 ⇒ 对端永不应答 ⇒ 本端等到超时 ⇒ 用户拿
//! `502 "acp forwarding failed"`，而操作**实际已在对端发生** ⇒ **必然的错误
//! 行为**。`session:cancel` 的终局裁定（台账 §5）正是基于这条推理。
//!
//! # 做法
//!
//! 照抄 `tests/envelope_code_surface.rs` 的骨架：**手写字符扫描、不新增依赖、
//! 不用正则库**；用 `include_str!` **编译期绑定** `agent-acp/src/methods.rs`
//! （`agent-acp` 不是本 crate 的依赖，`Cargo.toml` 属共享区、本卡不得改）。
//!
//! 「该常量是通知」的**唯一**信号是它**紧邻上一行**的 `///` 文档注释里含
//! 「通知」（`methods.rs:367` → `:368` 是 `SESSION_CANCEL`；
//! `methods.rs:369` → `:370` 是 `SESSION_UPDATE`）。本文件**刻意不用**
//! `tests/cancel_answer_surface.rs` 的 ±4 行散文窗口——那会把**别的**方法的
//! 注释误吸进来。
//!
//! ⛔ **新增通知方法时的写法约定**（否则本卡的扫描口径扫不到 ⇒ 登记面与
//! 实现面判红）：把「（通知）」写在**紧邻常量行的上一行** `///` 文档注释里，
//! 与 `methods.rs:367/369` 的现有写法同形。
//!
//! # 断言口径（**只守关系，不锁数量/取值**）
//!
//! 实现正当新增一个通知方法时，把新方法登记进
//! `contracts_check::ACP_NOTIFICATION_METHODS` 即可，本组断言自然变绿；任何
//! 「恰好 N 个通知方法」的断言都会在正当扩充时必红（教训：
//! `tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。
//!

use contracts_check::{
    ACP_FORWARDED_ACTIONS, ACP_FORWARD_WHITELIST_SNAPSHOT, ACP_METHODS, ACP_NOTIFICATION_METHODS,
};
use std::collections::{BTreeMap, BTreeSet};

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
///
/// `agent-acp` 不是本 crate 的依赖（dev-dependencies 里没有它，`Cargo.toml` 属
/// 共享区、本卡不得改），故按 `tests/cancel_answer_surface.rs` 的先例用
/// `include_str!` 绑定源文件**文本**，在测试里做源码扫描。
const ACP_METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");

/// 一个 `pub const <NAME>: &str = "<value>";` 声明（连同它的文档注释）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConstDecl {
    /// 常量名（如 `SESSION_CANCEL`）。
    name: String,
    /// 字符串字面量的值（如 `session/cancel`）。
    value: String,
    /// 该常量行**紧邻上一行**的 `///` 文档注释（去掉 `///` 前缀后的原文）；
    /// `None` 表示上一行**不是** `///` 文档注释（或该常量是文件首行）。
    doc: Option<String>,
}

/// 一行是不是注释行：去掉缩进后以 `//` 开头。
///
/// ⛔ **必须排除注释行**：`methods.rs:5-6` 的模块头（`//!`）与
/// `methods.rs:367/369` 的 `///` 文档注释里都写着「（通知）」字样——不排除
/// 就把**散文当实现**（`tests/envelope_code_surface.rs:29` 的同款先例）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 从**单行**里抽出 `pub const <NAME>: &str = "<value>";` 声明；不是这个形态
/// 返回 `None`。
///
/// 口径（手写字符扫描，不用正则）：①行内找 `pub const `；②常量名取到 `:`
/// 为止，必须**全为大写字母 / 数字 / 下划线**（排除结构体字段等其它 `const`
/// 字样）；③类型标注必须**恰好**是 `&str`（容忍 `& str` 的空白写法；排除
/// `&[u8]` / `u8` 等）；④`=` 之后取右值：必须是**字符串字面量**且以 `";`
/// 收尾（排除 `"a" + b` 这类表达式）。
fn parse_str_const(line: &str) -> Option<(String, String)> {
    let const_at = line.find("pub const ")?;
    let after = &line[const_at + "pub const ".len()..];
    let colon_at = after.find(':')?;
    let name = after[..colon_at].trim();
    // 「常量名」形态：非空且全为大写字母 / 数字 / 下划线。
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let eq_at = after[colon_at + 1..].find('=')?;
    let ty = after[colon_at + 1..colon_at + 1 + eq_at].trim();
    if ty != "&str" && ty != "& str" {
        return None;
    }
    let rhs = after[colon_at + 1 + eq_at + 1..].trim();
    let rhs = rhs.strip_prefix('"')?;
    let end = rhs.find('"')?;
    // 右值必须以 `";` 收尾（排除 `"a" + b` 这类拼接表达式）。
    if !rhs[end + 1..].trim_start().starts_with(';') {
        return None;
    }
    Some((name.to_owned(), rhs[..end].to_owned()))
}

/// 扫出源码里全部 `pub const <NAME>: &str = "<value>";` 声明（**排除注释行**），
/// 每个声明带它**紧邻上一行**的 `///` 文档注释。
fn scan_const_decls(source: &str) -> Vec<ConstDecl> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if is_comment_line(line) {
            continue;
        }
        let Some((name, value)) = parse_str_const(line) else {
            continue;
        };
        // 文档注释只认**紧邻上一行**的 `///`（`//!` 是模块级文档，不算）。
        let doc = idx.checked_sub(1).and_then(|prev_idx| {
            lines[prev_idx]
                .trim_start()
                .strip_prefix("///")
                .map(|rest| rest.trim().to_owned())
        });
        out.push(ConstDecl { name, value, doc });
    }
    out
}

/// 这个声明**是不是通知**：判据是它**紧邻上一行**的 `///` 文档注释里含「通知」。
fn is_notification(decl: &ConstDecl) -> bool {
    decl.doc.as_deref().is_some_and(|doc| doc.contains("通知"))
}

/// 扫出的**全部** `&str` 常量声明（`常量名 -> 字面量值`）。
fn scanned_const_decls() -> BTreeMap<String, String> {
    scan_const_decls(ACP_METHODS_SOURCE)
        .into_iter()
        .map(|d| (d.name, d.value))
        .collect()
}

/// 扫出的**通知方法面**（`方法名 -> 常量名`）。
///
/// 值域来自 `methods.rs` 的 `pub mod method`（13 个 `pub const`），其中
/// 「紧邻上一行的 `///` 注释含『通知』」的那几项即通知族。
fn scanned_notification_methods() -> BTreeMap<String, String> {
    scan_const_decls(ACP_METHODS_SOURCE)
        .into_iter()
        .filter(is_notification)
        .map(|d| (d.value, d.name))
        .collect()
}

/// 契约登记的通知方法面（`方法名` 集合）。
///
/// 常量 `ACP_NOTIFICATION_METHODS` 是 `&'static [&'static str]`（const 里的省略
/// 生命周期即 `'static`），故集合元素是 `&'static str`。
fn registered_notification_methods() -> BTreeSet<&'static str> {
    ACP_NOTIFICATION_METHODS.iter().copied().collect()
}

/// **实现面 ⊆ 登记面**：实现里标了「通知」的方法，契约必须登记。
///
/// 红了说明：实现新增了一个通知方法，但 `ACP_NOTIFICATION_METHODS` 没登记——
/// 先补登记（并同步 `docs/contracts/acp-method-ledger.md` §5.1 与
/// `docs/contracts/acp-host.md` §10），再合并。
#[test]
fn scanned_notification_methods_are_subset_of_the_registered_surface() {
    let registered = registered_notification_methods();
    let unregistered: Vec<String> = scanned_notification_methods()
        .into_iter()
        .filter(|(method, _)| !registered.contains(method.as_str()))
        .map(|(method, const_name)| format!("{method}（{const_name}）"))
        .collect();
    assert!(
        unregistered.is_empty(),
        "实现里标了「通知」但契约未登记的方法（差集）{unregistered:?}——\
         通知族是承重语义（转发是请求-响应，通知投进去必然超时 ⇒ 用户拿 502），\
         先补登记再合并"
    );
}

/// **登记面 ⊆ 实现面**：契约登记的通知方法，实现里必须仍带「通知」锚点。
///
/// 红了说明：登记面腐化了（实现的锚点被改写 / 常量被删 / 改名）⇒ 把它撤下
/// 登记，或把实现改回来。
#[test]
fn registered_notification_methods_are_present_in_the_implementation() {
    let scanned = scanned_notification_methods();
    let missing: Vec<String> = registered_notification_methods()
        .iter()
        .copied()
        .filter(|method| !scanned.contains_key(*method))
        .map(|method| method.to_owned())
        .collect();
    assert!(
        missing.is_empty(),
        "契约登记但实现里扫不到「通知」锚点的方法（差集）{missing:?}——\
         登记面腐化了，撤下登记或把实现改回来；\
         （若实现只是把「（通知）」挪到了多行文档注释里，请按本文件头的\
         「写法约定」把它放回紧邻常量行的上一行）\
         实扫到的通知方法：{:?}",
        scanned
    );
}

/// **通知族 ⊆ [`ACP_METHODS`]**：通知方法必须是 ACP 方法面的成员。
///
/// 防的是「登记出一个根本不是 ACP 方法的名字」（自创名）。
#[test]
fn notification_methods_are_a_subset_of_the_acp_method_surface() {
    let outside: Vec<&str> = registered_notification_methods()
        .iter()
        .copied()
        .filter(|method| !ACP_METHODS.contains(method))
        .collect();
    assert!(
        outside.is_empty(),
        "ACP_NOTIFICATION_METHODS 里 {outside:?} 不在 ACP_METHODS 里——\
         通知族是 ACP 方法面的子集，不得登记自创名"
    );
}

/// **通知族 ∩ [`ACP_FORWARDED_ACTIONS`] == ∅**：通知方法**不得**进转发面。
///
/// 转发机制是**请求-响应**（`forward_action` 发 Rpc 请求帧、按 id 等响应帧
/// 配对），对端**通知**永不配响应 ⇒ 原样转发必然等到超时 ⇒ 用户拿
/// `502 "acp forwarding failed"`，而操作**实际已在对端发生**。
///
/// 写法照抄 `tests/action_dispatch_surface.rs:270` 的
/// `forwarded_actions_do_not_collide_with_acp_methods`（收集交集 + 断言为空）。
#[test]
fn notification_methods_never_enter_the_forwarded_actions() {
    let overlap: Vec<&str> = registered_notification_methods()
        .iter()
        .copied()
        .filter(|method| ACP_FORWARDED_ACTIONS.contains(method))
        .collect();
    assert!(
        overlap.is_empty(),
        "通知方法进了 ACP 转发面（交集）{overlap:?}——转发是请求-响应机制，\
         投通知方法必然等不到响应 ⇒ 超时 ⇒ 用户拿 502，而操作已在对端发生"
    );
}

/// **通知族 ∩ [`ACP_FORWARD_WHITELIST_SNAPSHOT`] == ∅**：白名单快照同样
/// **不得**含通知方法（与断言 4 互为另一侧快照）。
#[test]
fn notification_methods_never_enter_the_forward_whitelist_snapshot() {
    let overlap: Vec<&str> = registered_notification_methods()
        .iter()
        .copied()
        .filter(|method| ACP_FORWARD_WHITELIST_SNAPSHOT.contains(method))
        .collect();
    assert!(
        overlap.is_empty(),
        "通知方法进了 ACP 转发白名单快照（交集）{overlap:?}——\
         转发白名单只收**请求-响应**形状的方法；通知方法投进去必然超时 ⇒ 502"
    );
}

/// **逐项锚点仍在**：每个登记成员的常量声明，其**紧邻上一行**的 `///` 文档
/// 注释必须仍写明「通知」。
///
/// ⛔ **刻意不用** `tests/cancel_answer_surface.rs` 的 ±4 行散文窗口——窗口
/// 会把**别的**方法的注释误吸进来。本文件用「上一行」的精确口径。
///
/// 这条补的是此前的缺口：两个通知方法里**只有** `SESSION_CANCEL` 被
/// `cancel_answer_surface.rs` 的散文窗口守着，`SESSION_UPDATE`（`methods.rs:370`）
/// **全仓无任何断言守它** ⇒ 裸奔。
#[test]
fn each_registered_notification_method_keeps_its_notification_anchor() {
    let decls = scan_const_decls(ACP_METHODS_SOURCE);
    for method in ACP_NOTIFICATION_METHODS {
        let Some(decl) = decls.iter().find(|d| d.value == *method) else {
            panic!(
                "ACP_NOTIFICATION_METHODS 里的 {method} 在 agent-acp/src/methods.rs 里\
                 找不到对应的 `pub const … : &str` 声明——登记面腐化了，\
                 撤下登记或把实现改回来"
            );
        };
        assert!(
            is_notification(decl),
            "通知锚点消失了：{method}（常量 {}）紧邻上一行的 `///` 文档注释里\
             没有「通知」（实为 {:?}）——对端语义疑似从通知改成请求-响应：\
             若属实，通知族与转发面的裁定可以重开，但必须先更新台账 \
             docs/contracts/acp-method-ledger.md §5.1 与 §5，而不是静默改口",
            decl.name,
            decl.doc
        );
    }
}

/// **登记面内部无重复**：同一方法名登记两次会让差集断言静默少跑一圈。
#[test]
fn registered_notification_methods_have_no_duplicates() {
    let unique: BTreeSet<&str> = registered_notification_methods();
    assert_eq!(
        unique.len(),
        ACP_NOTIFICATION_METHODS.len(),
        "ACP_NOTIFICATION_METHODS 内部有重复项：{:?}",
        ACP_NOTIFICATION_METHODS
    );
}

/// **防空扫描**：扫描器本身坏了（口径改坏 / 源文件形态变了）时，断言会
/// 「因为什么都没扫到」而假绿 ⇒ 单独守一条「扫得到东西」。
#[test]
fn scanner_actually_finds_the_acp_method_constants() {
    let all = scanned_const_decls();
    assert!(
        !all.is_empty(),
        "一个 `pub const … : &str` 声明都没扫到——扫描口径或源文件形态坏了，\
         断言会假绿"
    );
    let notifications = scanned_notification_methods();
    assert!(
        !notifications.is_empty(),
        "一个「通知」方法都没扫到——扫描口径（紧邻上一行的 `///` 注释含「通知」）\
         或源文件形态坏了，断言会假绿；扫到的常量：{all:?}"
    );
}

/// 扫描器**排除注释行**这条规则的自我证明。
///
/// ⛔ 若不排除注释，被注释掉的 `pub const … : &str = "…";` 声明就会被当成
/// 实现面 ⇒ 注释里的方法名会被误收进扫描结果。本断言用**合成样本**把这条
/// 规则钉死（样本里的注释行是**完整可解析的声明**——否则这条自检只是
/// 「碰巧绿」，起不到守规则的作用）。
#[test]
fn scanner_excludes_comment_lines() {
    let sample = "\
/// `session/cancel`（通知）
/// pub const SESSION_CANCEL: &str = \"session/cancel\";
// pub const SESSION_UPDATE: &str = \"session/update\";
pub const SESSION_NEW: &str = \"session/new\";
";
    let decls = scan_const_decls(sample);
    assert_eq!(
        decls.len(),
        1,
        "注释行里的声明被当成了实现——注释排除规则失效：{decls:?}"
    );
    assert_eq!(decls[0].value, "session/new");
    assert!(
        !decls[0].doc.as_deref().is_some_and(|d| d.contains("通知")),
        "上一行的注释被误当成了本常量的文档注释"
    );
}

/// 扫描器**「紧邻上一行」精确口径**的自我证明。
///
/// 「通知」信号只从**紧邻上一行**的 `///` 注释取值 ⇒ 隔了一行的「通知」字样
/// **不算**（这正是本文件与 `tests/cancel_answer_surface.rs` 的 ±4 行散文
/// 窗口的分界：窗口会把**别的**方法的注释误吸进来）。
#[test]
fn scanner_reads_the_notification_anchor_from_the_immediately_preceding_line() {
    let sample = "\
/// `session/new`（通知）
pub const SESSION_NEW: &str = \"session/new\";
/// `session/load`
pub const SESSION_LOAD: &str = \"session/load\";
/// `session/prompt`（通知）
///
/// 中间隔了一行
pub const SESSION_PROMPT: &str = \"session/prompt\";
";
    let notifications: Vec<String> = scan_const_decls(sample)
        .into_iter()
        .filter(is_notification)
        .map(|d| d.value)
        .collect();
    assert_eq!(
        notifications,
        vec!["session/new".to_owned()],
        "「通知」信号必须只从**紧邻上一行**的 `///` 注释取值"
    );
}
