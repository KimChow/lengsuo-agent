//! 会话/消息 schema 面（§3）：**枚举取值面**与**线上键名面**的契约断言。
//!
//! 本卡治的是**键名漂移**这半个病（另一半是上一张卡的响应码面）。
//! `AGENT-KICKOFF.md` §3 冻结的是**Rust 侧结构体的源字段名**（snake_case：
//! `workspace_root` / `permission_mode` / `created_at_ms` / `ts_ms` …），
//! 而**线上协议用的是 camelCase**（`serde(rename_all = "camelCase")`）。
//! 这个「源码字段名 ≠ 线上键名」的落差在事件 payload 层**没有任何断言守着**，
//! 已造成一处真实漂移：core 发 `argsRef`，壳侧读 `args_ref` ⇒ 恒为 `None`。
//!
//! # 断言口径：**只守关系，不锁具体取值/数量**
//!
//! - 四组枚举：**实现侧取值集合 == 契约登记的取值集合**（集合相等，不锁顺序、
//!   不锁「恰好几项」）；
//! - 线上键名面：**实现侧实际序列化出的键集合 == 契约登记的键集合**（同样集合相等）；
//! - 事件 payload 键名面：**实现源码里扫出的键集合 ⊆ 契约登记的键集合**
//!   （只守 ⊆，实现正当新增键时先补登记即可）；
//! - 壳侧读取：**camelCase 形态（`argsRef`）与旧形态（`args_ref`）都能读到**。
//!
//! ⛔ 刻意**不写** `assert_eq!(x.len(), N)` / `assert_eq!(x, &[...])` 这类锁规模
//! 或锁取值的断言——它们会在正当扩充时必红。
//!
//! 做法照抄 `tests/envelope_code_surface.rs`：**只读文件、手写字符扫描、
//! 不新增依赖、不用正则库**。
use agent_core::schema::{
    Isolation, Message, PermissionMode, Role, Session, SessionStatus, ToolInfo,
};
use contracts_check::{
    ISOLATION_JSON_KEYS, KICKOFF_SCHEMA_FIELD_NAMES, MESSAGE_JSON_KEYS, MESSAGE_ROLES,
    PERMISSION_MODES, RUNTIME_EVENT_PAYLOAD_KEYS, SESSION_JSON_KEYS, SESSION_MODES,
    SESSION_STATUSES, TOOL_EVENT_PAYLOAD_KEYS, TOOL_INFO_JSON_KEYS,
};
use lengsuo_shell::gateway::{EventKind, GatewayEvent};
use lengsuo_shell::state::{SessionMode, SessionStreamState};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// 被扫描的 `agent-core` runtime 源文件（相对 `agent-core/src/runtime/`）。
///
/// 覆盖面说明：这四份是**事件 payload 的全部构造点**（工具事件的四条路径）：
/// - `turns.rs`——回合主循环，发 `user` / `thinking` / `text` / `done` / `tool_use`；
/// - `turns_settle.rs`——结算路径，发 `tool_use` / `tool_result` / `text` / `done`；
/// - `tool_result.rs`——`tool_result` payload 的构造（`push_tool_result`）；
/// - `execute.rs`——工具执行结算，发 `tool_result`（被拒 / 失败 / MCP 三条）。
///
/// ⚠️ **未覆盖**（本卡范围外，登记面不覆盖）：`sessions.rs` / `context.rs` 发的
/// `session_update` / `user` / `warning` 载荷——它们的键面另属「会话元信息面」。
const SCANNED_RUNTIME_FILES: &[&str] = &[
    "turns.rs",
    "turns_settle.rs",
    "tool_result.rs",
    "execute.rs",
];

/// 标识「工具事件载荷」的**签名键**——只含**工具语义独有**的键。
///
/// 载荷里出现其中任一个 ⇒ 该 `json!` 块是 `tool_use` / `tool_result` 载荷：
/// - `argsRef`——`tool_use` 携带的参数 blob 引用（`turns.rs` / `turns_settle.rs`）；
/// - `resultRef` / `chars` / `preview`——`tool_result` 的结果引用 / 长度 / 摘要；
/// - `isError`——被拒 / 执行失败两种结算标记。
///
/// 识别口径不靠「调用点长什么样」（`emit(…, "tool_use", json!({…}))` 的排版会随
/// 实现改写），而靠**载荷的键名形态**——那才是线上契约的稳定面。
const TOOL_PAYLOAD_SIGNATURE_KEYS: &[&str] =
    &["argsRef", "resultRef", "chars", "preview", "isError"];

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
fn scan_json_object_keys(source: &str) -> BTreeSet<String> {
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
fn scan_index_assignment_keys(source: &str) -> BTreeSet<String> {
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
fn runtime_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-core")
        .join("src")
        .join("runtime")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()))
}

/// 取一个 JSON 对象的键集合。
fn object_keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("被测值必须是 JSON 对象")
        .keys()
        .cloned()
        .collect()
}

/// 把登记常量转成集合。
fn registered(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_string()).collect()
}

/// 取一组枚举的**线上字符串**集合。
///
/// 取值口径按「实现侧真实可用的方式」分两种：
/// - `Mode` / `PermissionMode` 有 `as_str()` ⇒ 直接取；
/// - `SessionStatus` / `Role` **没有** `as_str()` ⇒ 用 **serde 序列化**
///   （`#[serde(rename_all = "snake_case")]` ⇒ 序列化结果就是线上那个串）。
///
/// 两种口径的**共同点**是「取出来的串就是线上那个串」——否则登记面与线上面对不上。
macro_rules! wire_values {
    ($($value:expr),+ $(,)?) => {{
        let mut set: BTreeSet<String> = BTreeSet::new();
        $(
            let text = serde_json::to_value($value)
                .expect("枚举可序列化")
                .as_str()
                .expect("枚举序列化为字符串")
                .to_string();
            set.insert(text);
        )+
        set
    }};
}

/// 构造一个**把所有 `Option` 字段都填上值**的 `Session`。
///
/// 必须填满：`env_binding` / `memory_digest` / `run_status` 都带
/// `skip_serializing_if = "Option::is_none"` ⇒ 键集合随「字段是否为空」变化。
/// 只填一部分会让断言随取值而变（= 又一次「锁取值」违规）。
///
/// 构造方式：用 `serde_json::from_value` 从**线上形态**（camelCase）反序列化
/// ——这一步本身就验证了「线上 camelCase 能被 core 读回」，且不需要 `uuid`
/// 直接依赖（`contracts-check` 的 dev-deps 里没有 `uuid`，本卡不改 `Cargo.toml`）。
fn full_session() -> Session {
    serde_json::from_value(json!({
        "id": "11111111-1111-1111-1111-111111111111",
        "title": "contract",
        "workspaceRoot": "/tmp/contract",
        "mode": "craft",
        "permissionMode": "full",
        "envBinding": "env-1",
        "isolation": {"userId": "u", "tenantId": "t", "envId": "e"},
        "messages": [{
            "role": "tool_call",
            "content": "summary",
            "tool": {
                "name": "fs.read",
                "argsRef": "blob-args-1",
                "resultRef": "blob-result-1",
                "ms": 7
            },
            "tsMs": 1_700_000_000_000u64
        }],
        "createdAtMs": 1_700_000_000_000u64,
        "updatedAtMs": 1_700_000_000_500u64,
        "status": "running",
        "lastSeq": 3,
        "memoryDigest": "digest",
        "runStatus": {
            "runState": "idle",
            "turnSeq": 0,
            "lastTransitionAt": 1_700_000_000_000u64
        }
    }))
    .expect("线上 camelCase 形态的会话应能被 agent-core 读回")
}

/// §3 的 `Mode` 五值必须与实现侧**集合相等**。
///
/// 实现侧取 `Mode::ALL`（`agent-core/src/schema.rs:27`）的 `as_str()`；
/// 登记侧是 [`SESSION_MODES`]。集合相等 ⇒ 两边**同时**增删项即红。
#[test]
fn session_mode_values_match_the_registered_contract() {
    let implemented: BTreeSet<String> = agent_core::schema::Mode::ALL
        .iter()
        .map(|m| (*m).as_str().to_string())
        .collect();
    let contracted = registered(SESSION_MODES);
    assert_eq!(
        implemented, contracted,
        "Mode 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `PermissionMode` 两值必须与实现侧**集合相等**。
#[test]
fn permission_mode_values_match_the_registered_contract() {
    let implemented = wire_values![PermissionMode::ReadOnly, PermissionMode::Full];
    let contracted = registered(PERMISSION_MODES);
    assert_eq!(
        implemented, contracted,
        "PermissionMode 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `SessionStatus` 四值必须与实现侧**集合相等**。
///
/// `SessionStatus` 没有 `as_str()` ⇒ 取 **serde 序列化**结果（`snake_case`）。
#[test]
fn session_status_values_match_the_registered_contract() {
    let implemented = wire_values![
        SessionStatus::Running,
        SessionStatus::Idle,
        SessionStatus::Done,
        SessionStatus::Failed
    ];
    let contracted = registered(SESSION_STATUSES);
    assert_eq!(
        implemented, contracted,
        "SessionStatus 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `Role` 四值必须与实现侧**集合相等**。
///
/// `Role` 没有 `as_str()` ⇒ 取 **serde 序列化**结果（`snake_case`）。
#[test]
fn message_role_values_match_the_registered_contract() {
    let implemented = wire_values![
        Role::User,
        Role::Assistant,
        Role::ToolCall,
        Role::ToolResult
    ];
    let contracted = registered(MESSAGE_ROLES);
    assert_eq!(
        implemented, contracted,
        "Role 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// `Session` 线上 JSON 的**实际键集合**必须与登记的 [`SESSION_JSON_KEYS`] 相等。
///
/// 登记的是**线上**那一份（camelCase）——§3 文档写的是 Rust 侧 snake_case
/// 源字段名，线上不是那一份。
#[test]
fn session_json_keys_match_the_registered_contract() {
    let session = full_session();
    let actual = object_keys(&serde_json::to_value(&session).expect("会话可序列化"));
    let contracted = registered(SESSION_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Session 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `Message` 线上 JSON 的**实际键集合**必须与登记的 [`MESSAGE_JSON_KEYS`] 相等。
#[test]
fn message_json_keys_match_the_registered_contract() {
    let message: Message = serde_json::from_value(json!({
        "role": "tool_call",
        "content": "summary",
        "tool": {
            "name": "fs.read",
            "argsRef": "blob-args-1",
            "resultRef": "blob-result-1",
            "ms": 7
        },
        "tsMs": 1_700_000_000_000u64
    }))
    .expect("线上 camelCase 形态的消息应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&message).expect("消息可序列化"));
    let contracted = registered(MESSAGE_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Message 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `ToolInfo` 线上 JSON 的**实际键集合**必须与登记的 [`TOOL_INFO_JSON_KEYS`] 相等。
#[test]
fn tool_info_json_keys_match_the_registered_contract() {
    let tool: ToolInfo = serde_json::from_value(json!({
        "name": "fs.read",
        "argsRef": "blob-args-1",
        "resultRef": "blob-result-1",
        "ms": 7
    }))
    .expect("线上 camelCase 形态的工具信息应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&tool).expect("工具信息可序列化"));
    let contracted = registered(TOOL_INFO_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "ToolInfo 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// `Isolation` 线上 JSON 的**实际键集合**必须与登记的 [`ISOLATION_JSON_KEYS`] 相等。
#[test]
fn isolation_json_keys_match_the_registered_contract() {
    let isolation: Isolation = serde_json::from_value(json!({
        "userId": "u",
        "tenantId": "t",
        "envId": "e"
    }))
    .expect("线上 camelCase 形态的隔离键应能被 agent-core 读回");
    let actual = object_keys(&serde_json::to_value(&isolation).expect("隔离键可序列化"));
    let contracted = registered(ISOLATION_JSON_KEYS);
    assert_eq!(
        actual, contracted,
        "Isolation 线上键名面与契约登记不一致：实现 {actual:?} vs 登记 {contracted:?}"
    );
}

/// **反向守卫**：登记的线上键名面**不得**含 §3 文档的 snake_case 源字段名。
///
/// 这条防的是本卡最容易犯的错：把 §3 原文（`workspace_root` / `ts_ms` /
/// `args_ref` …）误当成**线上键名**登记进去。线上是 camelCase 那一份。
///
/// 只比「与线上写法不同」的那些名字（[`KICKOFF_SCHEMA_FIELD_NAMES`]）——
/// `role` / `content` / `name` / `ms` 两侧同名，比了没有意义。
#[test]
fn registered_wire_keys_are_not_the_kickoff_snake_case_field_names() {
    let mut wire = registered(SESSION_JSON_KEYS);
    wire.extend(registered(MESSAGE_JSON_KEYS));
    wire.extend(registered(TOOL_INFO_JSON_KEYS));
    wire.extend(registered(ISOLATION_JSON_KEYS));
    let kickoff = registered(KICKOFF_SCHEMA_FIELD_NAMES);
    let leaked: Vec<&String> = wire.intersection(&kickoff).collect();
    assert!(
        leaked.is_empty(),
        "登记的线上键名面混进了 §3 的 snake_case 源字段名 {leaked:?}——\
         线上是 camelCase 那一份，§3 原文不是线上键名"
    );
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

/// **宽面断言**：实现源码里扫出的事件 payload 键名 **⊆ 契约登记的键名面**。
///
/// 红了说明：core 在事件 payload 里发了新键，而契约侧没登记——先补登记
/// （并同步 `docs/contracts/shell-agent.md`），再合并。
///
/// ⚠️ 刻意**只守 ⊆ 关系、不锁数量/取值**：实现正当新增 payload 键时，把新键
/// 登记进常量后本断言自然变绿。
#[test]
fn runtime_payload_keys_are_subset_of_the_registered_contract() {
    let contracted = registered(RUNTIME_EVENT_PAYLOAD_KEYS);
    for file in SCANNED_RUNTIME_FILES {
        let scanned = scan_json_object_keys(&runtime_source(file));
        // 防空扫描：扫不出任何键 ⇒ 扫描器本身坏了（路径写错 / 口径改坏），
        // 断言会「因为什么都没扫到」而假绿。
        assert!(
            !scanned.is_empty(),
            "{file} 一个 payload 键名都没扫到——扫描口径或文件路径坏了，断言会假绿"
        );
        let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
        assert!(
            unregistered.is_empty(),
            "{file} 发出但 RUNTIME_EVENT_PAYLOAD_KEYS 未登记的 payload 键（差集）\
             {unregistered:?}——契约落后于实现，先补登记再合并"
        );
    }
}

/// **窄面断言（本卡的核心那条）**：`tool_use` / `tool_result` 事件 payload 的
/// 键名 **⊆ 登记的窄面**。
///
/// 识别口径：按**签名键**认载荷（见 [`TOOL_PAYLOAD_SIGNATURE_KEYS`]）——
/// `tool_use` 载荷带 `argsRef`；`tool_result` 载荷带 `chars` / `preview` /
/// `resultRef` / `isError`。另把下标写入形态（`payload["result"] = …` /
/// `payload["isError"] = …`）计入——它们补进的就是 `tool_result` 载荷。
///
/// 只守 ⊆：实现正当新增工具事件 payload 键时，先登记进
/// [`TOOL_EVENT_PAYLOAD_KEYS`] 后本断言自然变绿。
#[test]
fn tool_event_payload_keys_are_subset_of_the_registered_contract() {
    let contracted = registered(TOOL_EVENT_PAYLOAD_KEYS);
    let mut scanned = BTreeSet::new();
    for file in SCANNED_RUNTIME_FILES {
        let source = runtime_source(file);
        for group in scan_json_object_key_groups(&source) {
            let is_tool_payload = TOOL_PAYLOAD_SIGNATURE_KEYS
                .iter()
                .any(|sig| group.contains(*sig));
            if is_tool_payload {
                scanned.extend(group);
            }
        }
        scanned.extend(scan_index_assignment_keys(&source));
    }
    assert!(
        !scanned.is_empty(),
        "一个工具事件 payload 键名都没扫到——扫描口径坏了，断言会假绿"
    );
    let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
    assert!(
        unregistered.is_empty(),
        "工具事件 payload 键超出 TOOL_EVENT_PAYLOAD_KEYS 登记面（差集）{unregistered:?}——\
         契约落后于实现，先补登记再合并"
    );
}

/// 喂一条**照抄 core 真实形态**的 `tool_use` 事件，返回壳侧解出的参数引用。
///
/// 「真实形态」指键名是 `argsRef`（`agent-core/src/runtime/turns.rs:347` 的
/// `json!({"name", "args", "argsRef", "tsMs"})`）——**camelCase**，不是
/// `args_ref`。
fn shell_reads_args_ref(payload: Value) -> Option<String> {
    let mut state = SessionStreamState::new("s-1", SessionMode::Craft);
    let event = GatewayEvent {
        seq: 1,
        session_id: "s-1".to_string(),
        kind: EventKind::ToolUse,
        payload,
        ts_ms: 1_700_000_000_000u64,
    };
    state
        .apply_event(event)
        .expect("工具调用事件应被接受（craft 模式允许工具消息）");
    state
        .messages()
        .iter()
        .find_map(|m| m.tool.as_ref())
        .expect("应落一条带工具信息的消息")
        .args_ref
        .clone()
}

/// **漂移守卫（本卡的价值所在）**：壳侧必须能读到**线上真实形态**的 `argsRef`。
///
/// 壳侧原先只读 `args_ref` ⇒ 恒为 `None`（字段在、值恒空）——这正是
/// 「core 发 `argsRef`、壳侧读 `args_ref`」的键名漂移。
///
/// 这条断言**在修之前必然红**（当前恒为 `None`）⇒ 它就是本卡的先红后绿证据。
#[test]
fn shell_reads_the_camel_case_args_ref_from_a_real_tool_use_payload() {
    let args_ref = "blob-args-1";
    let got = shell_reads_args_ref(json!({
        "name": "fs.read",
        "args": {"path": "a.txt"},
        "argsRef": args_ref,
        "tsMs": 1_700_000_000_000u64
    }));
    assert_eq!(
        got.as_deref(),
        Some(args_ref),
        "壳侧读不到线上 camelCase 的 `argsRef`——键名漂移（core 发 argsRef，壳侧读 args_ref）"
    );
}

/// **兼容不破坏**：旧形态（`args_ref`，snake_case）**仍应可读**。
#[test]
fn shell_still_reads_the_legacy_snake_case_args_ref() {
    let args_ref = "blob-args-legacy";
    let got = shell_reads_args_ref(json!({
        "name": "fs.read",
        "args": {"path": "a.txt"},
        "args_ref": args_ref,
        "tsMs": 1_700_000_000_000u64
    }));
    assert_eq!(
        got.as_deref(),
        Some(args_ref),
        "壳侧读不到旧形态的 `args_ref`——兼容读取被破坏"
    );
}
