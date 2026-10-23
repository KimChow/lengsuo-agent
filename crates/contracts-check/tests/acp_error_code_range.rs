//! ACP 错误码**「JSON-RPC 保留区 vs 实现自定义应用区」互不重叠**守卫。
//!
//! `agent-acp` 的 wire 错误码由**两组常量**构成，却**没有任何断言**守着它们的关系
//! （实测 base `f5f7929`：`crates/*/tests` 对 `JSONRPC_ERROR_*` 这些常量名**零命中**）：
//!
//! | 组 | 位置 | 常量 |
//! |---|---|---|
//! | 保留区 | `src/message.rs` | `JSONRPC_ERROR_PARSE = -32700`、`JSONRPC_ERROR_INVALID_REQUEST = -32600`、`JSONRPC_ERROR_METHOD_NOT_FOUND = -32601`、`JSONRPC_ERROR_INVALID_PARAMS = -32602` |
//! | 应用区 | `src/router.rs` | `ACP_ERROR_CONNECTION_CLOSED = -32000`、`ACP_ERROR_INTERNAL = -32001` |
//! | 应用区（别名） | `src/host_router.rs` | `ACP_ERROR_HOST_CONNECTION_CLOSED = ACP_ERROR_CONNECTION_CLOSED` |
//!
//! JSON-RPC 2.0 把保留码放在 `[-32768, -32000)`，而 `-32000..=-32099` 是**实现自定义的
//! server error 区**。把某个应用码写成 `-32700` / `-32602`（极易手误），客户端就
//! **无法区分**「协议级错误（请求本身不合法 ⇒ 不该重试）」与「应用级错误（连接关闭
//! ⇒ 应重连）」⇒ fail-closed 判定被静默破坏，且**没有任何东西变红**。往保留区塞应用码同理。
//!
//! `agent-acp` **不在**本 crate 的 dev-dependencies 里、`Cargo.toml` 属共享禁区 ⇒
//! 照本 crate 同类先例（`tests/acp_default_parity.rs`）：`include_str!` **编译期**绑定
//! 源文本 + 手写扫常量定义，**不 import 被检查的 crate**、**不引入正则**。
//! ⛔ 守的是**关系与区间**；解析/解引用不出来时**显式失败**（`expect_*`），绝不默认放过。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// 五个被检查源文件的文本，均**编译期**绑定：源文件一改动就触发重编。

const MESSAGE_SOURCE: &str = include_str!("../../agent-acp/src/message.rs");
const ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/router.rs");
const HOST_ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/host_router.rs");
const CODEC_SOURCE: &str = include_str!("../../agent-acp/src/codec.rs");
const HOST_BASELINE_SOURCE: &str = include_str!("../../agent-acp/src/host_baseline.rs");

/// 错误码两族的**全部定义点**：`(文件标签, 源文本)`。单表唯一来源。
const CODE_SOURCES: [(&str, &str); 3] = [
    ("message.rs", MESSAGE_SOURCE),
    ("router.rs", ROUTER_SOURCE),
    ("host_router.rs", HOST_ROUTER_SOURCE),
];

/// 错误码源文件所在目录（供「谁定义了这两个族」的全目录扫描用）。
const ACP_SRC_DIR: &str = "../agent-acp/src";

// 两个区间：JSON-RPC 保留区 `[RESERVED_MIN, RESERVED_MAX_EXCLUSIVE)` 半开，
// 实现自定义 server error 区 `[SERVER_ERROR_MIN, SERVER_ERROR_MAX]` 闭合。

const RESERVED_MIN: i64 = -32768;
const RESERVED_MAX_EXCLUSIVE: i64 = -32000;
const SERVER_ERROR_MIN: i64 = -32099;
const SERVER_ERROR_MAX: i64 = -32000;

/// JSON-RPC 保留码的**精确**期望值（标准规定，本卡要求钉死取值本身）。
const EXPECTED_RESERVED: [(&str, i64); 4] = [
    ("JSONRPC_ERROR_PARSE", -32700),
    ("JSONRPC_ERROR_INVALID_REQUEST", -32600),
    ("JSONRPC_ERROR_METHOD_NOT_FOUND", -32601),
    ("JSONRPC_ERROR_INVALID_PARAMS", -32602),
];

/// ACP 应用码的**全部**期望名（别名也算一处定义）。
const EXPECTED_APPLICATION: [&str; 3] = [
    "ACP_ERROR_CONNECTION_CLOSED",
    "ACP_ERROR_INTERNAL",
    "ACP_ERROR_HOST_CONNECTION_CLOSED",
];

// 判别口径与钉死取值：前缀区分两族；`== "` / `serialize_str("` 是版本字面量的两种写法；
// 单行上限与探针名是 wire 侧的固定承诺；`MIN_SCANNED_FILES` 防「空目录 ⇒ 假绿」。

const RESERVED_PREFIX: &str = "JSONRPC_ERROR_";
const APPLICATION_PREFIX: &str = "ACP_ERROR_";
const EQUALITY_NEEDLE: &str = "== \"";
const SERIALIZE_NEEDLE: &str = "serialize_str(\"";
const VERSION_LITERAL: &str = "2.0";
const EXPECTED_MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;
const EXPECTED_SYS_PING_METHOD: &str = "sys:ping";
const MIN_SCANNED_FILES: usize = 5;

/// 错误码定义表：`常量名 -> (定义所在文件, `=` 与 `;` 之间的右值文本)`。
type CodeTable = BTreeMap<&'static str, (&'static str, &'static str)>;

/// 取值是否落在 JSON-RPC 保留区 `[-32768, -32000)`。
fn in_reserved_band(value: i64) -> bool {
    (RESERVED_MIN..RESERVED_MAX_EXCLUSIVE).contains(&value)
}

/// 取值是否落在实现自定义的 server error 区 `[-32099, -32000]`。
fn in_server_error_band(value: i64) -> bool {
    (SERVER_ERROR_MIN..=SERVER_ERROR_MAX).contains(&value)
}

/// 去掉 `_` 分组并 trim；空则 `None`（**不**默认成 0 或空串）。
fn digits_of(text: &str) -> Option<String> {
    let joined: String = text.chars().filter(|c| *c != '_').collect();
    let trimmed = joined.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 解析有符号整数字面量（允许 `_` 与前导 `-`）；非法即 `None`。
fn parse_i64(text: &str) -> Option<i64> {
    let digits = digits_of(text)?;
    let negative = digits.starts_with('-');
    let digits = digits.trim_start_matches('-');
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// 解析无符号整数字面量；非法即 `None`。
fn parse_unsigned(text: &str) -> Option<u64> {
    let digits = digits_of(text)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// 解析 `N * M * K` 形态的表达式（逐项相乘）⇒ 上限写成 `8 * 1024 * 1024` 或
/// `8388608` 读到**同一个数**；认不出任何一项 ⇒ `None`（**不**默认放过）。
fn parse_unsigned_product(text: &str) -> Option<u64> {
    let mut product: u64 = 1;
    let mut terms = 0usize;
    for term in text.split('*') {
        product = product.checked_mul(parse_unsigned(term)?)?;
        terms += 1;
    }
    (terms > 0).then_some(product)
}

/// 取字符串字面量的**内容**（两端引号必须成对）；否则 `None`。
fn parse_string_literal(text: &str) -> Option<&str> {
    text.trim().strip_prefix('"')?.strip_suffix('"')
}

/// 取一个**必须解析出来**的值；解析不出即 panic（不得默认放过）。
fn expect_some<T>(value: Option<T>, what: &str) -> T {
    value.unwrap_or_else(|| panic!("解析不出 {what}——源文件写法变了；解析不出来就红，绝不默认放过"))
}

/// 一行是不是注释（只认行注释；被扫的源文件里没有块注释）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 标识符（常量名 / 别名目标名）：非空且全是 ASCII 字母数字或 `_`。
fn is_identifier(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `const <name>:` 之前的可见性修饰是否合法（空 / `pub` / `pub(...)`）⇒
/// 注释散文与正文里的**引用**都不会被读成定义。
fn is_const_visibility(visibility: &str) -> bool {
    if visibility.is_empty() {
        return true;
    }
    let Some(rest) = visibility.strip_prefix("pub") else {
        return false;
    };
    rest.is_empty()
        || (rest.starts_with('(')
            && rest.ends_with(')')
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "()_".contains(c)))
}

/// 扫出源文件里**全部**常量定义：`(常量名, 右值文本)`。注释行与引用行天然被排除。
fn const_definitions(source: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let trimmed = line.trim_start();
        let Some(at) = trimmed.find("const ") else {
            continue;
        };
        if !is_const_visibility(trimmed[..at].trim()) {
            continue;
        }
        let Some((name, rest)) = trimmed[at + "const ".len()..].split_once(':') else {
            continue;
        };
        if !is_identifier(name) {
            continue;
        }
        let Some((_, rhs)) = rest.split_once('=') else {
            continue;
        };
        let rhs = rhs.split(';').next().unwrap_or_default().trim();
        if !rhs.is_empty() {
            found.push((name, rhs));
        }
    }
    found
}

/// 扫出源文件里 `needle` 之后紧跟的字符串字面量**内容**（`needle` 自带开引号）。
fn string_literals_after<'a>(source: &'a str, needle: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find(needle) {
            let tail = &rest[at + needle.len()..];
            let Some(end) = tail.find('"') else {
                break;
            };
            found.push(&tail[..end]);
            rest = &tail[end + 1..];
        }
    }
    found
}

/// 从一组字面量里挑出**像版本号**的（首字符是 ASCII 数字）⇒ `== "2.0"` 会被留下，
/// 而同文件里其它无关的字符串比较不会误伤本守卫。
fn version_like<'a>(literals: &[&'a str]) -> BTreeSet<&'a str> {
    literals
        .iter()
        .copied()
        .filter(|literal| literal.starts_with(|c: char| c.is_ascii_digit()))
        .collect()
}

/// 某常量在**唯一**定义点上的右值文本；0 处或多于 1 处 ⇒ 显式失败。
fn expect_single_rhs<'a>(source: &'a str, name: &str) -> &'a str {
    let found: Vec<&str> = const_definitions(source)
        .into_iter()
        .filter(|(defined, _)| *defined == name)
        .map(|(_, rhs)| rhs)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "`const {name}:` 的定义行有 {} 处（期望恰 1 处）——0 处 ⇒ 常量被移走/改名，\
         本守卫扫成空集后全绿是假的；多于 1 处 ⇒ 同一个量又长出了第二份手写实现",
        found.len()
    );
    found[0]
}

/// 从给定源文本表里扫出错误码两族的**全部**定义；重复定义 ⇒ 立刻红
/// （同一个码有两份定义，比对的是哪一份就无从确定）。
fn code_table_from(sources: &[(&'static str, &'static str)]) -> CodeTable {
    let mut table = CodeTable::new();
    for (file, source) in sources {
        for (name, rhs) in const_definitions(source) {
            if !name.starts_with(RESERVED_PREFIX) && !name.starts_with(APPLICATION_PREFIX) {
                continue;
            }
            assert!(
                table.insert(name, (*file, rhs)).is_none(),
                "错误码常量 {name} 被定义了不止一次（后一处 {file}）——两张定义让比对失去唯一解"
            );
        }
    }
    assert!(
        !table.is_empty(),
        "错误码定义表扫成了空集——扫描器失效，本守卫后面所有比对都会变成空对空"
    );
    table
}

/// 生产定义表（`CODE_SOURCES`：编译期绑定的三个源文件）。
fn code_table() -> CodeTable {
    code_table_from(&CODE_SOURCES)
}

/// 合成样本的定义表（只喂一段源码文本，不动生产代码）。
fn sample_table(source: &'static str) -> CodeTable {
    code_table_from(&[("sample.rs", source)])
}

/// 解出错误码常量的最终取值：右值是整数字面量则直接返回；是**另一个常量名**则沿别名链
/// 继续解（`host_router.rs` 的 `ACP_ERROR_HOST_CONNECTION_CLOSED` 走的正是这一支）。
/// 认不出（既非整数也非已知常量名）或别名成环 ⇒ `None` ⇒ 上层**显式失败**。
fn resolve_code_value(table: &CodeTable, name: &str) -> Option<i64> {
    let mut current = name.to_string();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    loop {
        if visited.contains(current.as_str()) {
            return None;
        }
        visited.insert(current.clone());
        let (_, rhs) = *table.get(current.as_str())?;
        if let Some(value) = parse_i64(rhs) {
            return Some(value);
        }
        if !is_identifier(rhs) {
            return None;
        }
        current = rhs.to_string();
    }
}

/// 解一个**必须解出来**的错误码；解不出即 panic。
fn expect_code_value(table: &CodeTable, name: &str) -> i64 {
    expect_some(
        resolve_code_value(table, name),
        &format!("错误码 {name}（定义缺失 / 右值既非整数也非已知常量名 / 别名成环）"),
    )
}

/// 某前缀族的**全部** `(常量名, 解引用后的取值)`，按名字排序。
fn family_values(table: &CodeTable, prefix: &str) -> Vec<(&'static str, i64)> {
    let mut found: Vec<(&str, i64)> = table
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, _)| (*name, expect_code_value(table, name)))
        .collect();
    found.sort_unstable();
    found
}

/// 某前缀族的取值集合。
fn family_codes(table: &CodeTable, prefix: &str) -> BTreeSet<i64> {
    family_values(table, prefix)
        .iter()
        .map(|(_, value)| *value)
        .collect()
}

/// 某前缀族的常量名集合。
fn family_names(table: &CodeTable, prefix: &str) -> BTreeSet<&'static str> {
    family_values(table, prefix)
        .iter()
        .map(|(name, _)| *name)
        .collect()
}

/// 被检查源文件的绝对路径（`CARGO_MANIFEST_DIR` + 相对路径）。
fn source_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// 递归收集目录下的全部 `.rs` 文件；目录读不到 ⇒ panic（不得当空目录）。
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读不到目录 {}: {e}——全目录扫描没发生", dir.display()));
    for entry in entries {
        let path = entry.expect("目录项读取失败").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// ① **保留码取值精确**：四个常量名一个不少、取值与标准一致，且**全部**落在
/// `[-32768, -32000)` ⇒ 任何手误（如 `-3260` 少一位）直接红。
#[test]
fn reserved_codes_have_the_spec_values() {
    let table = code_table();
    let expected: BTreeSet<&str> = EXPECTED_RESERVED.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        family_names(&table, RESERVED_PREFIX),
        expected,
        "保留码集合与期望不符——少一个 ⇒ 有码被移走/改名，本守卫少守一处"
    );
    for (name, expected) in EXPECTED_RESERVED {
        assert_eq!(
            expect_code_value(&table, name),
            expected,
            "`{name}` 取值不标准——保留码取值改了，客户端就分不清协议级与应用级错误"
        );
    }
    for (name, value) in family_values(&table, RESERVED_PREFIX) {
        assert!(
            in_reserved_band(value),
            "`{name}` = {value} 不落在保留区 [{RESERVED_MIN}, {RESERVED_MAX_EXCLUSIVE}) 内"
        );
    }
}

/// ② **应用码全部落在实现自定义区**：`router.rs` 两个字面量 + `host_router.rs` 那个
/// **别名**都要被解到同一个区间里。
#[test]
fn application_codes_live_in_the_implementation_defined_band() {
    let table = code_table();
    let expected: BTreeSet<&str> = EXPECTED_APPLICATION.into_iter().collect();
    assert_eq!(
        family_names(&table, APPLICATION_PREFIX),
        expected,
        "应用码集合与期望不符——少一个（尤其别名那处）⇒ 本守卫少守一处"
    );
    for (name, value) in family_values(&table, APPLICATION_PREFIX) {
        assert!(
            in_server_error_band(value),
            "`{name}` = {value} 不落在实现自定义区 [{SERVER_ERROR_MIN}, {SERVER_ERROR_MAX}] 内——\
             落进保留区会让客户端把「应用级错误（应重连）」读成「协议级错误」"
        );
    }
    // 别名必须**解到**与它所指常量同一个值（只扫字面量会把别名那处漏过去）。
    assert_eq!(
        expect_code_value(&table, "ACP_ERROR_HOST_CONNECTION_CLOSED"),
        expect_code_value(&table, "ACP_ERROR_CONNECTION_CLOSED"),
        "别名与所指常量发散——同一种连接关闭有了两个码"
    );
}

/// ③ **两组集合互斥**：保留码 ∩ 应用码 == ∅。重合即「一个码同时意味着协议级与应用级
/// 错误」，fail-closed 判定失去唯一解。
#[test]
fn the_reserved_and_application_code_sets_are_disjoint() {
    let table = code_table();
    let reserved = family_codes(&table, RESERVED_PREFIX);
    let application = family_codes(&table, APPLICATION_PREFIX);
    assert!(
        !reserved.is_empty() && !application.is_empty(),
        "两组里出现了空集——空集与任何集合都「不重叠」，本断言会退化成恒真"
    );
    let overlap: Vec<i64> = reserved.intersection(&application).copied().collect();
    assert!(
        overlap.is_empty(),
        "保留码与应用码重合 {overlap:?}——客户端无法据它决定「重试」还是「重连」"
    );
    assert!(
        family_names(&table, RESERVED_PREFIX)
            .is_disjoint(&family_names(&table, APPLICATION_PREFIX)),
        "有常量名同时挂在两个族下——前缀口径失效"
    );
}

/// ④ **协议固定量钉死**：单行上限恰为 8 MiB 且为 2 的幂；`DEFAULT_PROTOCOL_VERSION`
/// 存在且可解析；`SYS_PING_METHOD` 恰为 `"sys:ping"`。
#[test]
fn the_wire_limits_and_protocol_pins_are_intact() {
    let line_bytes = expect_some(
        parse_unsigned_product(expect_single_rhs(CODEC_SOURCE, "DEFAULT_MAX_LINE_BYTES")),
        "codec.rs 的 DEFAULT_MAX_LINE_BYTES",
    );
    assert_eq!(
        line_bytes, EXPECTED_MAX_LINE_BYTES,
        "单行上限被改动会改变「对端多长的行才被判超限」，属跨端 wire 承诺"
    );
    assert!(
        line_bytes.is_power_of_two(),
        "`DEFAULT_MAX_LINE_BYTES` = {line_bytes} 不是 2 的幂——缓冲扩容处处按 2 的幂推导"
    );

    let version = expect_some(
        parse_unsigned(expect_single_rhs(
            HOST_BASELINE_SOURCE,
            "DEFAULT_PROTOCOL_VERSION",
        )),
        "host_baseline.rs 的 DEFAULT_PROTOCOL_VERSION",
    );
    assert!(
        version >= 1,
        "`DEFAULT_PROTOCOL_VERSION` = {version}——代际 0 不是已发布的版本"
    );

    let ping = expect_some(
        parse_string_literal(expect_single_rhs(HOST_BASELINE_SOURCE, "SYS_PING_METHOD")),
        "host_baseline.rs 的 SYS_PING_METHOD",
    );
    assert_eq!(
        ping, EXPECTED_SYS_PING_METHOD,
        "`SYS_PING_METHOD` 实测 {ping:?}——它按「名称同形可直接转发」被当探针投出"
    );
}

/// ⑤ **`JsonRpcVersion` 只接受 `"2.0"`**：反序列化的判等与序列化的写入**只有**这一个
/// 版本字面量 ⇒ 任一侧被改成 `"2.1"` / `"1.0"` 都会被咬住。
#[test]
fn the_jsonrpc_version_accepts_only_two_dot_zero() {
    let comparisons = string_literals_after(MESSAGE_SOURCE, EQUALITY_NEEDLE);
    assert!(
        !comparisons.is_empty(),
        "扫不到任何 `== \"…\"` 比较字面量——本断言会变成空对空"
    );
    assert_eq!(
        version_like(&comparisons),
        BTreeSet::from([VERSION_LITERAL]),
        "反序列化接受的版本字面量只该有 {VERSION_LITERAL:?}——多一个 ⇒ 不支持的版本仍被收下"
    );

    let serialized = string_literals_after(MESSAGE_SOURCE, SERIALIZE_NEEDLE);
    assert!(
        !serialized.is_empty(),
        "扫不到 `serialize_str(\"…\")`——写出那处写法变了"
    );
    assert_eq!(
        version_like(&serialized),
        BTreeSet::from([VERSION_LITERAL]),
        "序列化写出的版本字面量只该有 {VERSION_LITERAL:?}"
    );
}

/// ⑥ **只有预期的那几个文件定义这两个族 + 文件仍在该处**：全目录扫描 ⇒ 新开一个文件
/// 写 `ACP_ERROR_X = -32700`（绕过 `include_str!` 绑定）会被立刻发现；盘上真读到的名字
/// 还须与**编译期绑定文本**扫出的一致 ⇒ 绑定指错文件也立刻红。
#[test]
fn only_the_expected_files_define_the_two_code_families() {
    let dir = source_path(ACP_SRC_DIR);
    let mut files = Vec::new();
    collect_rs_files(&dir, &mut files);
    assert!(
        files.len() >= MIN_SCANNED_FILES,
        "只读到 {} 个 .rs 文件（期望 ≥ {MIN_SCANNED_FILES}）——全目录扫描没发生或提前停下",
        files.len()
    );

    // 盘上每个文件里定义的错误码常量：`(文件名, 常量名)`。
    let mut on_disk: Vec<(String, String)> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("读不了 {}: {e}", path.display()));
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (name, _) in const_definitions(&text) {
            if name.starts_with(RESERVED_PREFIX) || name.starts_with(APPLICATION_PREFIX) {
                on_disk.push((file.clone(), name.to_string()));
            }
        }
    }

    let files_with_codes: BTreeSet<&str> = on_disk.iter().map(|(file, _)| file.as_str()).collect();
    let expected: BTreeSet<&str> = ["message.rs", "router.rs", "host_router.rs"]
        .into_iter()
        .collect();
    assert_eq!(
        files_with_codes, expected,
        "定义这两个族的文件与期望不符——多一个 ⇒ 有码在 `include_str!` 绑定之外，区间检查漏掉了它"
    );

    // 盘上真读到的名字必须与 `include_str!` 编译期绑定文本扫出的名字一致 ⇒ 绑定指错文件即红。
    let disk_names: BTreeSet<&str> = on_disk.iter().map(|(_, name)| name.as_str()).collect();
    let bound_names: BTreeSet<&str> = code_table().keys().copied().collect();
    assert_eq!(
        disk_names, bound_names,
        "盘上扫到的常量名与编译期绑定文本扫到的不一致——绑定指错了文件"
    );
}

/// ⑦ **fail-closed（负向锚 + 合成样本）**：扫描器认不出的东西必须返回空 / `None`
/// 而**不是**「当没看见」，且把错值塞进合成样本时区间断言真的会咬。
#[test]
fn the_scanners_refuse_rubbish_instead_of_going_green() {
    // 扫不到 ⇒ 空集（空集在上层会被非空断言变成红，不存在「空对空全绿」）。
    assert!(
        const_definitions("/// pub const JSONRPC_ERROR_PARSE: i64 = -1;\n").is_empty(),
        "注释行里的常量被当成了定义——被注释掉的旧取值会冒充现行定义"
    );
    assert!(
        const_definitions("    let x = JSONRPC_ERROR_PARSE;\n").is_empty(),
        "对常量的**引用**被当成了定义——常量名匹配成了子串匹配"
    );

    // 认不出的取值 ⇒ `None`，绝不默认成 0。
    assert_eq!(parse_i64("abc"), None, "非数字被解析成了值");
    assert_eq!(parse_i64(""), None, "空串被解析成了值");
    assert_eq!(
        parse_unsigned_product("8 * OOPS"),
        None,
        "非法表达式被解析成了值"
    );
    assert_eq!(
        parse_string_literal("\"2.0"),
        None,
        "引号不成对被当成了字面量"
    );

    // 别名**能**解引用（否则 ② 里别名那处是空对空）；成环与未知名一律 `None`。
    let aliased = sample_table(
        "pub const ACP_ERROR_A: i64 = -32000;\npub const ACP_ERROR_B: i64 = ACP_ERROR_A;\n",
    );
    assert_eq!(
        resolve_code_value(&aliased, "ACP_ERROR_B"),
        Some(-32000),
        "别名没被解引用——`host_router.rs` 那处定义会被当成解不出的码"
    );
    assert_eq!(
        resolve_code_value(&aliased, "NOPE"),
        None,
        "不存在的名字解出了取值"
    );
    let cyclic = sample_table(
        "pub const ACP_ERROR_A: i64 = ACP_ERROR_B;\npub const ACP_ERROR_B: i64 = ACP_ERROR_A;\n",
    );
    assert_eq!(
        resolve_code_value(&cyclic, "ACP_ERROR_A"),
        None,
        "别名成环时解出了取值"
    );

    // 错值真的会被咬：往应用区塞保留码 ⇒ ② 会红；把保留码写成自定义区码 ⇒ ① 会红。
    let wrong_app = sample_table("pub const ACP_ERROR_X: i64 = -32700;\n");
    let app_band = family_values(&wrong_app, APPLICATION_PREFIX);
    assert_eq!(app_band.len(), 1, "合成样本没扫出那一个应用码");
    assert!(
        !in_server_error_band(app_band[0].1),
        "-32700 写进应用码没被咬住——② 是摆设"
    );
    let wrong_reserved = sample_table("pub const JSONRPC_ERROR_PARSE: i64 = -32000;\n");
    let reserved_band = family_values(&wrong_reserved, RESERVED_PREFIX);
    assert_eq!(reserved_band.len(), 1, "合成样本没扫出那一个保留码");
    assert!(
        !in_reserved_band(reserved_band[0].1),
        "-32000 写成保留码没被咬住——① 是摆设"
    );
}
