//! ACP **JSON-RPC 2.0 基础信封键面**（`agent-acp/src/message.rs`）的登记与护栏断言。
//!
//! # 治什么病
//!
//! JSON-RPC 基础信封（`JsonRpcRequest` / `JsonRpcNotification` / `JsonRpcResponse` /
//! `JsonRpcErrorObject`）的**线上键名面**在契约层**零登记**：帧分类入口
//! `Message::from_value`（`message.rs:236`）判别三分类时**只认四个键名**
//! （`id` / `method` / `result` / `error`）⇒ 任一方改名，帧就被分到错误的类别
//! （请求被当成响应、通知被当成请求），或整帧落 `MalformedMessage`（**硬失败**）。
//! 而这四个键名此前**没有任何断言**守着。
//!
//! # 与相邻已登记面的边界（别误以为重复登记）
//!
//! ACP 控制面信封顶层 `type`（`ACP_ENVELOPE_TAGS`）是**另一层**信封、正交；方法名
//! 面（`ACP_METHODS` / `ACP_NOTIFICATION_METHODS`）是**方法名**、不是键名面；ACP
//! wire 错误码**数值**面（`ACP_STANDARD_ERROR_CODES` / `ACP_PRIVATE_ERROR_CODES`）是
//! **码值**、不是 error 子对象的**形状**——本卡只管**键名面**。
//!
//! # 为什么是源码扫描
//!
//! `agent-acp` 不是本 crate 的依赖（dev-dependencies 里没有它，`Cargo.toml` 属共享区、
//! 本卡不得改）⇒ 只能按 `tests/acp_envelope_tag_surface.rs` 的先例用 `include_str!`
//! **编译期**绑定源文件文本（源文件改动即触发重编），在测试里做手写字符扫描
//! （**不新增依赖、不用正则库**）。
//!
//! # 口径：只守关系，不锁数量 / 不锁取值
//!
//! 实现正当新增一个键时，把新键登记进 `contracts_check` 的对应常量即可；任何
//! 「恰好 N 个」「集合等于某个定值」的断言都会在正当扩充时必红（教训：
//! `tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。本文件**不**写
//! `assert_eq!(<面>.len(), N)`、也**不**写把具体取值写死的 `assert_eq!(<面>, &[…])`；
//! 只守**子集 / 双向相等 / 实现面 ⊆ 登记面 / 集合不等**。

use contracts_check::{
    ACP_JSONRPC_DISCRIMINATION_KEYS, ACP_JSONRPC_ERROR_OBJECT_KEYS, ACP_JSONRPC_NOTIFICATION_KEYS,
    ACP_JSONRPC_REQUEST_KEYS, ACP_JSONRPC_RESPONSE_KEYS, ACP_JSONRPC_VERSION,
    GATEWAY_ENVELOPE_KEYS,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
const MESSAGE_SOURCE: &str = include_str!("../../agent-acp/src/message.rs");

/// 判别规则依赖的键（**契约镜像**里手写的角色名，取自 JSON-RPC 2.0 规范本身）。
const RULE_METHOD_KEY: &str = "method";
const RULE_ID_KEY: &str = "id";
const RULE_RESULT_KEY: &str = "result";
const RULE_ERROR_KEY: &str = "error";
const RULE_DEPENDENCY_KEYS: &[&str] = &[
    RULE_METHOD_KEY,
    RULE_ID_KEY,
    RULE_RESULT_KEY,
    RULE_ERROR_KEY,
];

/// `contains_key("` 前缀（扫判别键用）。
const CONTAINS_KEY_PREFIX: &str = "contains_key(\"";

// ---------------------------------------------------------------- 扫描器 ----

/// 一行是不是注释：去掉缩进后以 `//` 开头。口径对齐
/// `tests/acp_error_surface.rs` 的 `is_comment_line`（只认**行注释**够用：
/// 被扫描的 `message.rs` 里没有块注释）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 一行的缩进宽度（按字符数计）。
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

/// 扫出文本里全部 `<marker>X` 的 `X`（`marker` **以引号结尾**；`X` 到下一个引号为止）。
///
/// **排除注释行**；一行里出现多次时**全部**收集。
fn scan_after(text: &str, marker: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        if is_comment_line(line) {
            continue;
        }
        let mut idx = 0usize;
        while let Some(pos) = line[idx..].find(marker) {
            let start = idx + pos + marker.len();
            let Some(end) = line[start..].find('"') else {
                break;
            };
            out.insert(line[start..start + end].to_owned());
            idx = start + end + 1;
        }
    }
    out
}

/// 扫出实现面里**全部**「版本形状」的字面量（`"<数字>.<数字>"`，**排除注释行**）。
///
/// 逐字符扫出每个**字符串字面量**（`\"` 是转义引号、不另起一个字面量），再只留
/// **纯数字点号数字**的那种 ⇒ 错误消息里那句 `\"2.0\"`（转义形态、混在散文里）
/// 不会被吸进来 ⇒ 扫到的就是**真正的版本取值**。
fn scan_version_literals(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            i += 1;
            let mut seg = String::new();
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    seg.push('\\');
                    seg.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    break;
                }
                seg.push(bytes[i] as char);
                i += 1;
            }
            i += 1;
            let parts: Vec<&str> = seg.split('.').collect();
            if parts.len() == 2
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                out.insert(seg);
            }
        }
    }
    out
}

/// 取出 `pub fn from_value` 的**函数体**文本（收到缩进 ≤ 函数行缩进的单独 `}` 为止）。
fn from_value_body(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains("pub fn from_value"))?;
    let base = indent_of(lines[start]);
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim() == "}" && indent_of(line) <= base)
        .map_or(lines.len(), |pos| start + 1 + pos);
    Some(lines[start + 1..end].join("\n"))
}

/// 扫出某个结构体的**字段名集合**与**体部 `serde(rename)` 取值**。
///
/// 字段只认 `pub <名字>:` 声明行 ⇒ 属性行（`#[serde(…)]`）与文档注释天然被排除。
fn scan_struct(source: &str, name: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let lines: Vec<&str> = source.lines().collect();
    let head = format!("pub struct {name} {{");
    let Some(start) = lines.iter().position(|line| line.trim_start() == head) else {
        return (BTreeSet::new(), BTreeSet::new());
    };
    let base = indent_of(lines[start]);
    let (mut fields, mut renames) = (BTreeSet::new(), BTreeSet::new());
    for line in lines.iter().skip(start + 1) {
        if line.trim() == "}" && indent_of(line) <= base {
            break; // 结构体收尾
        }
        if is_comment_line(line) {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix("pub ") {
            let field: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !field.is_empty() && rest[field.len()..].starts_with(':') {
                fields.insert(field);
                continue;
            }
        }
        if let Some(rest) = line.trim_start().strip_prefix("#[serde(rename = \"") {
            if let Some(end) = rest.find('"') {
                renames.insert(rest[..end].to_owned());
            }
        }
    }
    (fields, renames)
}

// ---------------------------------------------------------------- 比对器 ----

/// 登记面（`String` 化，便于与扫出来的集合直接比）。
fn registered_set(surface: &[&str]) -> BTreeSet<String> {
    surface.iter().map(|s| (*s).to_owned()).collect()
}

/// 断言两个集合**逐项相等**（双向差集为空）。
fn assert_same(left: &BTreeSet<String>, right: &BTreeSet<String>, what: &str) {
    let missing: Vec<&str> = left.difference(right).map(String::as_str).collect();
    let extra: Vec<&str> = right.difference(left).map(String::as_str).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{what}：左侧有右侧无 {missing:?}；右侧有左侧无 {extra:?}"
    );
}

/// 断言 `sub ⊆ sup`。
fn assert_subset(sub: &BTreeSet<String>, sup: &BTreeSet<String>, what: &str) {
    let extra: Vec<&str> = sub.difference(sup).map(String::as_str).collect();
    assert!(extra.is_empty(), "{what}：{extra:?} 越出了被包含的面");
}

/// 登记面里**不在** `known` 里的那些键。
fn offenders<'a>(surface: &'a [&'a str], known: &BTreeSet<String>) -> Vec<&'a str> {
    surface
        .iter()
        .copied()
        .filter(|k| !known.contains(*k))
        .collect()
}

/// 取一个 JSON 值的**顶层键集合**。
fn wire_keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("帧必须是 JSON 对象")
        .keys()
        .cloned()
        .collect()
}

/// 帧类别（**契约镜像**，不是生产实现的复制）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// 有 `method` 有 `id`。
    Request,
    /// 有 `method` 无 `id`。
    Notification,
    /// 无 `method` 有 `id` 且有 `result` / `error`。
    Response,
    /// 既无 `method` 又无 `id`（或响应缺 `result` / `error`）⇒ 判别失败。
    Unclassified,
}

/// **契约镜像**：按 `message.rs:236-263` 的判别规则重演一遍帧分类。
///
/// 生产实现的正确性由 `agent-acp` 自己的测试负责；本函数只回答「登记的判别键面
/// **够不够**表达这 5 条规则」。
fn classify_frame(obj: &Map<String, Value>) -> FrameKind {
    let has_id = obj.contains_key(RULE_ID_KEY);
    let has_result_or_error = obj.contains_key(RULE_RESULT_KEY) || obj.contains_key(RULE_ERROR_KEY);
    if obj.contains_key(RULE_METHOD_KEY) {
        return if has_id {
            FrameKind::Request
        } else {
            FrameKind::Notification
        };
    }
    if has_id && has_result_or_error {
        return FrameKind::Response;
    }
    FrameKind::Unclassified
}

// ------------------------------------------------------------------ 断言 ----

/// **A1 实现面 ⊆ 登记面**：`from_value` 实际查的判别键必须在登记面里。
#[test]
fn implementation_discrimination_keys_are_subset_of_the_registered_face() {
    let scanned = scan_after(
        &from_value_body(MESSAGE_SOURCE).unwrap(),
        CONTAINS_KEY_PREFIX,
    );
    assert!(
        !scanned.is_empty(),
        "一个判别键都没扫到——扫描口径坏了，断言会假绿"
    );
    assert_same(
        &scanned,
        &registered_set(ACP_JSONRPC_DISCRIMINATION_KEYS),
        "判别键面不一致——判别键漂移 ⇒ 帧被分到错误的类别（请求当响应 / 通知当请求）",
    );
}

/// **A2 登记面 ⊆ 实现面**：登记的每个判别键都必须真的被 `from_value` 用到。
///
/// 反向也守：否则登记面自身腐化了（登记了实现早已不用的键）而没人知道。
#[test]
fn registered_discrimination_keys_are_subset_of_the_implementation() {
    let scanned = scan_after(
        &from_value_body(MESSAGE_SOURCE).unwrap(),
        CONTAINS_KEY_PREFIX,
    );
    assert!(
        !scanned.is_empty(),
        "一个判别键都没扫到——扫描口径坏了，断言会假绿"
    );
    assert_same(
        &registered_set(ACP_JSONRPC_DISCRIMINATION_KEYS),
        &scanned,
        "判别键面不一致——登记面腐化了，撤下登记或把实现改回来",
    );
}

/// **A3 判别键 ⊆ 请求键 ∪ 通知键 ∪ 响应键**：不能凭空多一个判别键。
#[test]
fn discrimination_keys_lie_inside_the_three_frame_faces() {
    let mut union = registered_set(ACP_JSONRPC_REQUEST_KEYS);
    union.extend(registered_set(ACP_JSONRPC_NOTIFICATION_KEYS));
    union.extend(registered_set(ACP_JSONRPC_RESPONSE_KEYS));
    let bad = offenders(ACP_JSONRPC_DISCRIMINATION_KEYS, &union);
    assert!(
        bad.is_empty(),
        "判别键 {bad:?} 不属于任何帧的键面——判别键必须挂在真实信封上"
    );
}

/// **B1 各结构体字段名面与登记面逐项双向相等**（⛔ 只登记**字段名**，不登记类型）。
#[test]
fn every_jsonrpc_struct_field_set_equals_its_registered_face() {
    let cases: [(&str, &[&str]); 4] = [
        ("JsonRpcRequest", ACP_JSONRPC_REQUEST_KEYS),
        ("JsonRpcNotification", ACP_JSONRPC_NOTIFICATION_KEYS),
        ("JsonRpcResponse", ACP_JSONRPC_RESPONSE_KEYS),
        ("JsonRpcErrorObject", ACP_JSONRPC_ERROR_OBJECT_KEYS),
    ];
    for (name, registered) in cases {
        let (scanned, _) = scan_struct(MESSAGE_SOURCE, name);
        assert!(
            !scanned.is_empty(),
            "{name} 一个字段都没扫到——扫描口径坏了，断言会假绿"
        );
        assert_same(
            &scanned,
            &registered_set(registered),
            &format!("{name} 的字段名面与登记面不一致——线上键名漂移 ⇒ 编解码两端对不上"),
        );
    }
}

/// **B2「wire 键名 == 字段名」这个前提本身要被守**。
///
/// 本仓 `message.rs` **无任何** `serde(rename)` ⇒ 字段名即线上键名；一旦有人加了
/// `rename`，登记的键面就整体失真（登记的是字段名、线上是另一个名字）⇒ 必须红。
#[test]
fn no_jsonrpc_struct_renames_its_fields() {
    for name in [
        "JsonRpcRequest",
        "JsonRpcNotification",
        "JsonRpcResponse",
        "JsonRpcErrorObject",
    ] {
        let (_, renames) = scan_struct(MESSAGE_SOURCE, name);
        assert!(
            renames.is_empty(),
            "{name} 用了 serde rename {renames:?}——线上键名不再等于字段名，\
             本卡的键面登记（按字段名登记）整体失真，先同步登记面"
        );
    }
}

/// **C1 两层「同名不同形」必须可区分**。
///
/// JSON-RPC 的 error 子对象（`i64` 码 + `message`）与网关的 u16 信封
/// （`{code,msg,data}`）**都叫 `code`**，但**不是**同一个东西 ⇒ 两层**都**登记了
/// `code`（同名键），且两层的键名集合**不相同**（JSON-RPC 层有 `message`、u16
/// 信封层有 `msg`）⇒ 两层可区分。⛔ 守的是「两层不是同一个东西」这条**关系**，
/// **不**锁任何一层的成员数。
#[test]
fn the_jsonrpc_error_object_and_the_u16_envelope_are_distinguishable() {
    let jsonrpc = registered_set(ACP_JSONRPC_ERROR_OBJECT_KEYS);
    let envelope = registered_set(GATEWAY_ENVELOPE_KEYS);
    assert!(
        jsonrpc.contains("code") && envelope.contains("code"),
        "两层没都登记 `code`——同名不同形的隔离前提变了（JSON-RPC 层 {jsonrpc:?} / \
         u16 信封层 {envelope:?}）"
    );
    assert_ne!(
        jsonrpc, envelope,
        "JSON-RPC error 子对象与 u16 信封的键名集合相同了——两层被混成了同一个东西"
    );
}

/// **C2 error 子对象挂在响应上**：其键名不得成为无人登记的孤儿键。
#[test]
fn the_error_object_hangs_off_the_response_face() {
    let response = registered_set(ACP_JSONRPC_RESPONSE_KEYS);
    assert!(
        response.contains(RULE_ERROR_KEY),
        "响应键面里没有 `error` 键——error 子对象不挂在响应上"
    );
    let mut known = response;
    known.extend(registered_set(ACP_JSONRPC_ERROR_OBJECT_KEYS));
    let orphans = offenders(ACP_JSONRPC_ERROR_OBJECT_KEYS, &known);
    assert!(
        orphans.is_empty(),
        "error 子对象的键 {orphans:?} 两个 JSON-RPC 键面里都没登记——孤儿键"
    );
}

/// **D1 版本取值**：实现面里**只**认登记的那一个版本字面量，且登记的那个确实在用。
///
/// 判据（接受侧 `== "…"`）与写回侧（`serialize_str("…")`）**用同一个字面量**；
/// 实现面**不得**出现第二个版本取值（否则「只支持一个版本」的硬保证失效）。
/// ⛔ 不写死取值：断言两侧都由**扫描**得来，实现改了版本、登记面跟着改即绿。
#[test]
fn the_version_literal_is_single_and_matches_the_registered_one() {
    let literals = scan_version_literals(MESSAGE_SOURCE);
    assert!(
        !literals.is_empty(),
        "一个版本字面量都没扫到——扫描口径坏了，断言会假绿"
    );
    assert!(
        literals.contains(ACP_JSONRPC_VERSION),
        "登记的版本取值 {ACP_JSONRPC_VERSION:?} 在实现面里扫不到——登记面腐化了"
    );
    let drifted: Vec<&str> = literals
        .iter()
        .map(String::as_str)
        .filter(|v| *v != ACP_JSONRPC_VERSION)
        .collect();
    assert!(
        drifted.is_empty(),
        "实现面出现了登记面之外的版本字面量 {drifted:?}（登记值 {ACP_JSONRPC_VERSION:?}）——\
         「只支持一个版本」的硬保证失效"
    );
    for (label, marker) in [("接受判据", "== \""), ("写回", "serialize_str(\"")] {
        let site = scan_after(MESSAGE_SOURCE, marker);
        assert!(
            !site.is_empty(),
            "版本{label}处扫不到字面量——源文件形态变了，断言会假绿"
        );
        let mismatched: Vec<&str> = site
            .iter()
            .map(String::as_str)
            .filter(|v| *v != ACP_JSONRPC_VERSION)
            .collect();
        assert!(
            mismatched.is_empty(),
            "版本{label}处用了与登记面不同的版本字面量 {mismatched:?}——收到别的版本会被整帧拒"
        );
    }
}

/// **E1 扫描器能发现漂移**（合成样本，**不改生产代码**）。
///
/// 把 `contains_key("id")` 改成 `contains_key("identifier")` ⇒ 扫出的键面必须
/// **不同**、且**不再 ⊆ 登记面** ⇒ A1 那条断言会红。这证明 A1 不是「永远绿」的摆设。
#[test]
fn the_scanner_detects_a_renamed_discrimination_key() {
    let sample = "\
impl Message {
    pub fn from_value(v: &Value) -> crate::Result<Self> {
        let obj = v.as_object().unwrap();
        let has_id = obj.contains_key(\"id\");
        let has_result_or_error = obj.contains_key(\"result\") || obj.contains_key(\"error\");
        if obj.contains_key(\"method\") {
            return Ok(Self::Request);
        }
        Ok(Self::Response)
    }
}
";
    let original = scan_after(
        &from_value_body(sample).expect("样本里没有 from_value——扫描口径坏了"),
        CONTAINS_KEY_PREFIX,
    );
    assert!(
        !original.is_empty(),
        "合成样本一个判别键都没扫到——扫描口径坏了，本断言失去意义"
    );
    let drifted_source = sample.replace(
        &format!("{CONTAINS_KEY_PREFIX}{RULE_ID_KEY}\")"),
        &format!("{CONTAINS_KEY_PREFIX}identifier\")"),
    );
    assert_ne!(
        drifted_source, sample,
        "合成样本里找不到 `contains_key(\"id\")` 这一处——变异样本没造出来，本断言失去意义"
    );
    let drifted = scan_after(
        &from_value_body(&drifted_source).unwrap(),
        CONTAINS_KEY_PREFIX,
    );
    assert_ne!(
        drifted, original,
        "判别键改名后扫出的键面居然没变——扫描器没在读那些字面量（恒返回空/恒返回全集）"
    );
    assert!(
        !drifted.is_subset(&registered_set(ACP_JSONRPC_DISCRIMINATION_KEYS)),
        "改名后的判别键竟然仍 ⊆ 登记面——A1 断言咬不到这种漂移，护栏失效"
    );
}

/// **E2 扫描器排除注释行**（合成样本）。
///
/// 注释里的 `contains_key("fake_key")` **不得**被扫出来（对齐
/// `tests/acp_error_surface.rs` 的 `is_comment_line` 口径）。
#[test]
fn the_scanner_excludes_comment_lines() {
    let sample = "\
impl Message {
    pub fn from_value(v: &Value) -> crate::Result<Self> {
        let obj = v.as_object().unwrap();
        // contains_key(\"fake_key\")
        /// contains_key(\"fake_doc_key\")
        let has_id = obj.contains_key(\"id\");
        Ok(Self::Request)
    }
}
";
    let scanned = scan_after(sample, CONTAINS_KEY_PREFIX);
    assert!(
        !scanned.contains("fake_key") && !scanned.contains("fake_doc_key"),
        "注释行里的键被扫进来了（{scanned:?}）——注释排除规则失效"
    );
    assert!(
        scanned.contains(RULE_ID_KEY),
        "真行里的判别键没被扫到（{scanned:?}）——扫描口径坏了"
    );
}

/// **F1 端到端行为级探针**：真实 wire 帧的键集合与登记面逐项对齐。
///
/// 帧用 `serde_json` 手工构造（**走真实解码路径**的镜像），不是文本扫描。
#[test]
fn real_wire_frames_match_the_registered_faces() {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "session/new", "params": {}});
    let notification = json!({"jsonrpc": "2.0", "method": "session/update", "params": {}});
    let full = json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {}});
    let error_obj = json!({"code": -32601, "message": "m", "data": {}});
    for (frame, surface) in [
        (&request, ACP_JSONRPC_REQUEST_KEYS),
        (&notification, ACP_JSONRPC_NOTIFICATION_KEYS),
        (&full, ACP_JSONRPC_RESPONSE_KEYS),
        (&error_obj, ACP_JSONRPC_ERROR_OBJECT_KEYS),
    ] {
        assert_eq!(
            wire_keys(frame),
            registered_set(surface),
            "wire 帧的线上键集合与登记面不一致：{frame}"
        );
    }
    // 响应面：`result` 与 `error` 互斥（`skip_serializing_if`）⇒ 线上键是响应面的
    // **子集**，但判别成员必须在场、另一个必须不在场。
    let ok = json!({"jsonrpc": "2.0", "id": 1, "result": {}});
    let err = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "m"}});
    for (frame, present, absent) in [
        (&ok, RULE_RESULT_KEY, RULE_ERROR_KEY),
        (&err, RULE_ERROR_KEY, RULE_RESULT_KEY),
    ] {
        let keys = wire_keys(frame);
        assert_subset(
            &keys,
            &registered_set(ACP_JSONRPC_RESPONSE_KEYS),
            "响应的线上键越出了登记的响应面",
        );
        assert!(
            keys.contains(present) && !keys.contains(absent),
            "响应的判别成员不对（{keys:?}）——`result` 与 `error` 必须互斥"
        );
    }
}

/// **F2 判别行为探针**：5 种帧形态必须被分到正确的类别。
///
/// ⚠️ 这是**契约镜像**（按 `message.rs:236-263` 的规则重演），不是复制生产实现：
/// 生产实现的正确性由 `agent-acp` 自己的测试负责；本卡只负责「登记的判别键面
/// 能完整表达这 5 条规则所依赖的键」。
#[test]
fn the_registered_face_expresses_every_classification_rule() {
    // 规则所依赖的键 ⊆ 登记的判别键面。
    let registered = registered_set(ACP_JSONRPC_DISCRIMINATION_KEYS);
    let missing = offenders(RULE_DEPENDENCY_KEYS, &registered);
    assert!(
        missing.is_empty(),
        "判别规则依赖的键 {missing:?} 不在登记的判别键面里——登记面表达不了这 5 条规则"
    );
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "session/new"});
    let notification = json!({"jsonrpc": "2.0", "method": "session/update"});
    let ok = json!({"jsonrpc": "2.0", "id": 1, "result": {}});
    let err = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "m"}});
    let bare = json!({"jsonrpc": "2.0"});
    for (frame, expected) in [
        (&request, FrameKind::Request),
        (&notification, FrameKind::Notification),
        (&ok, FrameKind::Response),
        (&err, FrameKind::Response),
        (&bare, FrameKind::Unclassified),
    ] {
        let obj = frame.as_object().expect("帧必须是 JSON 对象");
        assert_eq!(
            classify_frame(obj),
            expected,
            "帧 {frame} 的判别结果不对——判别键面表达不了判别规则"
        );
    }
}
