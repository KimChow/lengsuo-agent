//! ACP **出站**通知方法面（host → client）的登记与「出站 ⊆ 入站」关系断言（W2 · lane proto）。
//!
//! # 治什么病
//! ACP 通知有**两个方向**，此前只有一半被守住：**入站**（client 收 host 的通知）由
//! `ACP_NOTIFICATION_METHODS` 守着；**出站**（host 主动推给 client）此前**零登记、零断言**。
//! 出站出口 `Host::notify(method: impl Into<String>, …)` 是**自由字符串、本身不构成一个面**；真正
//! 决定 method 名的是**调用点传入的实参**。而 client 侧收通知（`router.rs:275`
//! `TopLevel::Rpc(Message::Notification(n)) => self.push_event(...)`）**不校验 method 名、直接入队**
//! ⇒ 拼错一个字母 ⇒ 对端永远不认，却占着队列 ⇒ **静默降级**（不是硬失败）。
//!
//! # 登记口径
//! `ACP_OUTBOUND_NOTIFICATION_METHODS` 登记 host 侧实际能推出去的通知 method 名，口径 = **本仓
//! `notify(...)` 调用点传入的 method 字面量**（含 `method::XXX` 常量路径，按 `methods.rs` 还原）。
//! ⚠️ **事实记录**：生产侧两个文件（`host_send.rs` 的 `notify` 定义、`agent-gateway/src/acp.rs` 的
//! 转发包装）**今天没有任何硬编码 method 字面量** ⇒ 它们不构成独立的面，字面量**全部来自调用点**；
//! 扫描器仍把它们纳入，日后生产侧硬编码即会扫到。
//! ⚠️ **扫描器前置条件（如实记录，已由测试强制）**：注释剥离按行做、靠字符串成对跳过；被扫源文件里
//! **没有**「`//` 落在原始字符串内部」的形态。命中即红（要求先扩展扫描器），不会静默假绿。
//! ⛔ **不在本卡范围**：`Client::send_notification`（`client.rs:191`）是 client → host（host 的
//! **入站**方向），已由 `ACP_NOTIFICATION_METHODS` 覆盖。
//!
//! # 断言口径（**只守关系，不锁数量/取值**）
//! ① 出站面 ⊆ 入站通知面；② 扫描器自证（防空扫描 + 覆盖每一个登记项 + 解析前置条件）；③ 扫描面
//! 与登记面**双向相等**；④ 出站面 ∩ `ACP_CONTROL_SUBTYPES` == ∅。⚠️ **不锁数量**：任何
//! `assert_eq!(…, .len(), N)` 都会在正当扩充时必红（教训：`tests/method_ledger.rs` 的
//! `assert_eq!(ACP_METHODS.len(), 13)`）。

use contracts_check::{
    ACP_CONTROL_SUBTYPES, ACP_NOTIFICATION_METHODS, ACP_OUTBOUND_NOTIFICATION_METHODS,
};
use std::collections::{BTreeMap, BTreeSet};

// ---- 被扫描的源文件（**编译期**绑定，源文件改动即触发重编） ----
// 出口：host_send.rs（`Host::notify` 定义）、gateway/acp.rs（转发包装）。调用点：host_role.rs /
// stdio_host.rs / acp_bridge.rs。常量表：methods.rs（还原 `method::XXX`）。
const HOST_SEND_SOURCE: &str = include_str!("../../agent-acp/src/host_send.rs");
const GATEWAY_ACP_SOURCE: &str = include_str!("../../agent-gateway/src/acp.rs");
const HOST_ROLE_SOURCE: &str = include_str!("../../agent-acp/tests/host_role.rs");
const STDIO_HOST_SOURCE: &str = include_str!("../../agent-acp/tests/stdio_host.rs");
const ACP_BRIDGE_SOURCE: &str = include_str!("../../agent-gateway/tests/acp_bridge.rs");
const METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");

// ---- 扫描器（手写字符扫描，不用正则库） ----

/// 跳过从 `start` 起的字符串字面量，返回**其后**的字节下标。
fn skip_string_bytes(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// 去掉全部注释（字符串字面量里的 `//` **不算**注释）。注释里的括号与字面量不得进实现面。
fn strip_comments(source: &str) -> String {
    let mut out = Vec::new();
    for line in source.lines() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => i = skip_string_bytes(bytes, i),
                b'/' if bytes.get(i + 1) == Some(&b'/') => break,
                _ => i += 1,
            }
        }
        out.push(&line[..i]);
    }
    out.join("\n")
}

/// 从 `start`（`notify(` 的 `(` 之后）开始做括号平衡扫描，返回（实参文本，收尾后的下标）。
fn balanced_text(bytes: &[u8], start: usize) -> (String, usize) {
    let mut depth = 1i32; // `notify(` 自己的括号
    let mut text: Vec<u8> = Vec::new();
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            let end = skip_string_bytes(bytes, i);
            text.extend_from_slice(&bytes[i..end]);
            i = end;
            continue;
        }
        i += 1;
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        text.push(c);
    }
    (String::from_utf8_lossy(&text).into_owned(), i)
}

/// 切出实参文本的**第一个**实参（顶层逗号为界；字符串字面量里的逗号不算）。
fn first_argument(args: &[u8]) -> Option<String> {
    let mut depth = 0i32;
    let mut cut = args.len();
    let mut i = 0;
    while i < args.len() {
        let b = args[i];
        if b == b'"' {
            i = skip_string_bytes(args, i);
            continue;
        }
        i += 1;
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                cut = i - 1;
                break;
            }
            _ => {}
        }
    }
    let first = String::from_utf8_lossy(&args[..cut]).trim().to_owned();
    (!first.is_empty()).then_some(first)
}

/// 实参是不是字符串字面量？是则返回**去引号**后的值。
fn string_literal(arg: &str) -> Option<String> {
    let body = arg.trim().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(chars.next()?),
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

/// 实参是不是 `::<常量名>` 形态的常量路径（如 `method::SESSION_UPDATE`）？是则返回常量名。
fn const_path_name(arg: &str) -> Option<&str> {
    let (_path, name) = arg.trim().rsplit_once("::")?;
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    ok.then_some(name)
}

/// 第一个实参 ⇒ method 名：字符串字面量取其值；常量路径按 `methods.rs` 的常量表还原。
fn literal_or_const(arg: &str, consts: &BTreeMap<String, String>) -> Option<String> {
    if let Some(lit) = string_literal(arg) {
        return Some(lit);
    }
    let name = const_path_name(arg)?;
    consts.get(name).cloned()
}

/// 扫出 `methods.rs` 里每个 `pub const <NAME>: &str = "<值>";` ⇒ 常量名 → 字面量。
fn method_const_map(source: &str) -> BTreeMap<String, String> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub const ")?;
            let (name, rhs) = rest.split_once(": &str")?;
            let (_, value) = rhs.split_once('=')?;
            let value = string_literal(value.trim().trim_end_matches(';').trim())?;
            Some((name.trim().to_owned(), value))
        })
        .collect()
}

/// 扫出一个源文件里**所有** `notify(...)` 调用点传入的 method 名。只认**第一个实参**：字符串
/// 字面量取其值，`::常量名` 常量路径按 `methods.rs` 还原；其余（变量、`&self`）⇒ 不属于本面。
fn notify_method_names(source: &str, consts: &BTreeMap<String, String>) -> BTreeSet<String> {
    let cleaned = strip_comments(source);
    let bytes = cleaned.as_bytes();
    let mut out = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let is_call = bytes[i..].starts_with(b"notify(")
            && !(i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'));
        if !is_call {
            i += 1;
            continue;
        }
        let (args, next) = balanced_text(bytes, i + "notify(".len());
        if let Some(first) = first_argument(args.as_bytes()) {
            if let Some(name) = literal_or_const(&first, consts) {
                out.insert(name);
            }
        }
        i = next.max(i + 1);
    }
    out
}

/// 被扫文件（标签, 源码）：生产侧两个出口 + 三处调用点。
const SCANNED_FILES: &[(&str, &str)] = &[
    ("host_send.rs", HOST_SEND_SOURCE),
    ("gateway/acp.rs", GATEWAY_ACP_SOURCE),
    ("agent-acp/tests/host_role.rs", HOST_ROLE_SOURCE),
    ("agent-acp/tests/stdio_host.rs", STDIO_HOST_SOURCE),
    ("agent-gateway/tests/acp_bridge.rs", ACP_BRIDGE_SOURCE),
];

/// 每个被扫文件对出站面的贡献（`文件名 → method 名集合`）；并集即全部出站 method 名。
fn contributions() -> BTreeMap<&'static str, BTreeSet<String>> {
    let consts = method_const_map(METHODS_SOURCE);
    SCANNED_FILES
        .iter()
        .map(|(label, src)| (*label, notify_method_names(src, &consts)))
        .collect()
}

/// 扫描全部被扫文件 ⇒ 出站通知 method 名集合（**并集**）。
fn scanned_outbound_names() -> BTreeSet<String> {
    contributions().values().flatten().cloned().collect()
}

// ---- 判定函数（断言与「判定能咬人」的自证用例走**同一个**函数） ----

/// **判定**：出站面里**不被入站通知面承认**的名字（空 = 全部被承认）。
fn unrecognized_by_inbound<'a>(outbound: &[&'a str]) -> Vec<&'a str> {
    outbound
        .iter()
        .copied()
        .filter(|m| !ACP_NOTIFICATION_METHODS.contains(m))
        .collect()
}

/// **判定**：扫描面与登记面的**双向差集**（`扫描\登记`、`登记\扫描`）；空 = 两者相等。
fn scan_vs_registry_gap(scanned: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let registered: BTreeSet<String> = ACP_OUTBOUND_NOTIFICATION_METHODS
        .iter()
        .map(|m| (*m).to_owned())
        .collect();
    let unregistered: Vec<String> = scanned.difference(&registered).cloned().collect();
    let stale: Vec<String> = registered.difference(scanned).cloned().collect();
    (unregistered, stale)
}

// ---- 断言 ----

/// **① 出站面 ⊆ 入站通知面**（本卡最核心的一条）。红了说明 host 侧自创了一个**对端不认**的
/// 通知名 ⇒ 对端永远收不到，却占着队列 ⇒ **静默降级**。
#[test]
fn the_outbound_surface_is_a_subset_of_the_inbound_notification_surface() {
    let unrecognized = unrecognized_by_inbound(ACP_OUTBOUND_NOTIFICATION_METHODS);
    assert!(
        unrecognized.is_empty(),
        "出站面里有入站通知面不承认的名字 {unrecognized:?}——入站侧**不校验 method 名**\
         （`agent-acp/src/router.rs` 的 `TopLevel::Rpc(Message::Notification(n))` 直接入队）\
         ⇒ 拼错 ⇒ 对端永远不认，却占着队列 ⇒ **静默降级**。先登记进 \
         `ACP_NOTIFICATION_METHODS`，再合并"
    );
}

/// **② 扫描器自证**：扫到的集合**非空**（防空扫描），且**包含每一个登记项**。
/// ⛔ 漏掉这条 ⇒ 扫描器扫了个空集，断言照样绿。
#[test]
fn the_scanner_actually_sees_the_outbound_surface() {
    let scanned = scanned_outbound_names();
    assert!(
        !scanned.is_empty(),
        "一个出站通知 method 名都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    for &name in ACP_OUTBOUND_NOTIFICATION_METHODS {
        assert!(
            scanned.contains(name),
            "登记的出站方法 {name} 在源码里扫不到——扫描口径坏了；各文件贡献：{:?}",
            contributions()
        );
    }
}

/// **②b 扫描器解析前置条件**：被扫源文件里**没有**「`//` 落在原始字符串内部」的形态。
/// ⛔ 出现这种形态 ⇒ 注释剥离会把原始字符串内容当成注释截掉 ⇒ 真实调用点被吞 ⇒ 断言假绿
/// ⇒ 必须先扩展扫描器。原始字符串**后面**跟真注释是正常的，不算命中。
#[test]
fn the_scanner_parsing_precondition_holds() {
    let bad: Vec<(usize, String)> = SCANNED_FILES
        .iter()
        .flat_map(|(_, source)| source.lines().enumerate())
        .filter(|(_, line)| slash_inside_raw_string(line))
        .map(|(idx, line)| (idx + 1, line.to_owned()))
        .collect();
    assert!(
        bad.is_empty(),
        "{bad:?} 行有「`//` 落在原始字符串内部」的形态 ⇒ 注释剥离可能剥错位置 ⇒ \
         真实调用点被吞 ⇒ 假绿；先扩展扫描器（支持原始字符串），再合并"
    );
}

/// 这一行里有没有 `//` 落在原始字符串（`r#"…"#`）**内部**？
///
/// 原始字符串在行内**未闭合**也算命中（后续行都是原始内容 ⇒ 同样危险）。
fn slash_inside_raw_string(line: &str) -> bool {
    let mut rest = line;
    while let Some(start) = rest.find("r#\"") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("\"#") else {
            return true;
        };
        if after[..end].contains("//") {
            return true;
        }
        rest = &after[end + 2..];
    }
    false
}

/// **③ 扫描面与登记面双向相等**（漏登记 / 登记腐化都红）。
#[test]
fn the_scanned_outbound_surface_equals_the_registered_surface() {
    let scanned = scanned_outbound_names();
    let (unregistered, stale) = scan_vs_registry_gap(&scanned);
    assert!(
        unregistered.is_empty() && stale.is_empty(),
        "出站面与登记面不一致：源码扫到而登记没有 {unregistered:?}；登记有而源码扫不到 {stale:?}——\
         出站 method 名是**对外契约**（拼错 ⇒ 对端当未知通知 ⇒ 静默降级），先补登记再合并"
    );
}

/// **④ 「同名不同形」隔离**：出站面不得混入下划线形态的 control subtype 名。
#[test]
fn the_outbound_surface_never_carries_control_subtype_names() {
    let overlap: Vec<&str> = ACP_OUTBOUND_NOTIFICATION_METHODS
        .iter()
        .copied()
        .filter(|m| ACP_CONTROL_SUBTYPES.contains(m))
        .collect();
    assert!(
        overlap.is_empty(),
        "出站面里有 control subtype 名 {overlap:?}——出站是 ACP **wire method**（斜杠形态），\
         混进下划线形态的 control subtype 名会被对端当成别的东西"
    );
}

/// **判定真能咬人**（合成样本）：构造一个含入站面**从不承认**的名字的**假**出站名集合，判定函数
/// **必须**把它报出来。⛔ 不改生产代码、不放宽口径——只把假集合喂给**断言本身用的那个判定函数**。
#[test]
fn the_judge_detects_an_outbound_name_the_inbound_surface_never_recognized() {
    let fake = ["session/update", "session/not-a-real-method"];
    let unrecognized = unrecognized_by_inbound(&fake);
    assert!(
        unrecognized.contains(&"session/not-a-real-method"),
        "自创的出站名没被报出来——判定逻辑没生效的话，护栏只是**永远绿**的摆设；报出的：{unrecognized:?}"
    );
}

/// **判定真能咬人**（合成样本）：扫描到一个**未登记**的新名字时，双向差集必须报出它。
#[test]
fn the_judge_detects_a_newly_appearing_outbound_name() {
    let mut scanned = scanned_outbound_names();
    scanned.insert("session/not-a-real-method".to_owned());
    let (unregistered, _stale) = scan_vs_registry_gap(&scanned);
    assert!(
        unregistered.contains(&"session/not-a-real-method".to_owned()),
        "未登记的新出站名没被报出来——护栏只是**永远绿**的摆设；报出的：{unregistered:?}"
    );
}

/// 扫描器**只认第一个实参**：第二个实参里的字面量**不得**被当成 method 名。
#[test]
fn the_scanner_takes_the_method_name_from_the_first_argument_only() {
    let source = "host.notify(method, json!({\"note\": \"session/update\"}));\n";
    let scanned = notify_method_names(source, &BTreeMap::new());
    assert!(
        scanned.is_empty(),
        "扫描器把**第二个**实参里的字面量当成了 method 名：{scanned:?}——\
         method 名只可能来自第一个实参"
    );
}

/// 扫描器能把 `method::SESSION_UPDATE` 这类**常量路径**实参还原成字面量（仓内既有写法，见
/// `agent-acp/tests/transport_client.rs`）⇒ 还原不了 ⇒ 护栏会漏看它。
#[test]
fn the_scanner_resolves_a_constant_path_argument() {
    let consts = method_const_map(METHODS_SOURCE);
    let source = "host.notify(method::SESSION_UPDATE, json!({}));\n";
    let scanned = notify_method_names(source, &consts);
    assert!(
        scanned.contains("session/update"),
        "常量路径实参（`method::SESSION_UPDATE`）没被还原成字面量——护栏会漏看它"
    );
}

/// 扫描器**排除注释**：被注释掉的调用**不得**进实现面。
#[test]
fn the_scanner_ignores_commented_out_calls() {
    let source = "// host.notify(\"session/commented\", json!({}));\n\
                  host.notify(\"session/real\", json!({}));\n";
    let scanned = notify_method_names(source, &BTreeMap::new());
    assert!(
        scanned.contains("session/real") && !scanned.contains("session/commented"),
        "扫描器把注释里的调用当成了实现面：{scanned:?}"
    );
}
