//! `ToolCallUpdate` 载荷两个枚举的 wire 拼写面 + fail-closed 守卫（W2）。
//!
//! # 这条守卫治什么病
//!
//! `session/update` 载荷里 `ToolCallUpdate`（`tool_call` / `tool_call_update`
//! 事件的 payload 本体，`crates/agent-acp/src/methods.rs`）内嵌两个枚举：
//! `ToolKind`（8 变体）与 `ToolCallStatus`（4 变体），wire 拼写都由容器
//! `#[serde(rename_all = "snake_case")]` 派生。它们是外部 ACP client 直接消费的
//! 线上词，但 contracts-check 侧对两者**零钉住**（2026-10-24 实测：全 crate 对
//! `ToolKind` / `ToolCallStatus` 零命中；`session_schema_surface/json_keys.rs`
//! 只钉了 `toolCallId` 键、枚举值无钉；源码注释「未知 kind 反序列化失败即协议
//! 违例」的 fail-closed 语义无任何测试）。round-trip ≠ 契约钉住：双边同走 serde，
//! `rename_all` 被改、变体被改名、加 `#[serde(other)]` 兜底时自走测试照绿，炸的
//! 只有跨进程对端——其中**加兜底**最阴险：把「未知 kind = 协议违例」静默降级成
//! 「未知 kind = 吞进兜底变体（照常走）」。
//!
//! # 做法与口径
//!
//! `contracts-check` 的 dev-dependencies **没有** `agent-acp`，而 `Cargo.toml`
//! 属共享区不得改，故照 `tests/session_update_tag_surface.rs` /
//! `tests/wire_tag_enum_trio.rs` 的手法走**源码文本扫描**：`include_str!` 编译期
//! 绑定源文件文本（源文件被移动 = 编译期红），另用 `CARGO_MANIFEST_DIR` 相对路径
//! 断言文件仍在该处。期望表是**手写的冻结真相**；容器 `rename_all` 钉死 + 变体级
//! rename 禁令 + `#[serde(other)]` 兜底禁令钉住派生输入；表项再与派生规则互证。
//! 只守关系与拼写、不锁行数：变体集 ⇄ 表双向集合相等是目的（新变体登记后即绿）。
//! 扫描器**防空**：声明头找不到 / 花括号不配平 ⇒ `Err`；合成样本自证各漂移都真能红。
//!
//! # fail-closed 行为探针（对卡面「本文件内同构局部枚举」的一处偏差）
//!
//! 卡面要求用「本文件内同构局部枚举（复制相同 serde 属性）」实证未知值 ⇒ Err，
//! 但本 crate dev-dependencies 只有 `serde_json`、**没有 `serde`**（derive 宏与
//! trait 均不可命名），局部枚举写不出 `#[derive(Deserialize)]`。偏差方案：取
//! dev-dependency `agent-core` 的 `RunState`（公开、snake_case、全 unit 变体、
//! 含多词变体 `AwaitingInput`）作**编译期真实探针**，其源文本同样被本文件扫描
//! 钉死（同属性同构由机器断言）。探针证明的派生语义——未知值必 Err（unknown
//! variant）、报错预期集 = 全变体集（无兜底旁证）、拼写逐字精确、多词 camel→snake
//! 双向派生——对同一 serde 派生机器下的所有同属性枚举一致 ⇒ 结论迁移到
//! `ToolKind` / `ToolCallStatus`。补齐局部枚举探针 = 给 contracts-check dev-deps
//! 加 `serde = { version = "1", features = ["derive"] }`（共享区改动，移交基建卡）。

use std::collections::BTreeSet;
use std::path::Path;

use agent_core::RunState;
use serde_json::json;

/// agent-acp 源文本（**编译期**绑定：源文件改动即触发重编）。
const METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");
/// fail-closed 行为探针 `RunState` 的源文本（**编译期**绑定）。
const RUN_STATE_SOURCE: &str = include_str!("../../agent-core/src/run_status/types.rs");

/// (标签, 相对 `CARGO_MANIFEST_DIR` 路径)：存在性断言用（防 include_str! 绑到旧路径后假绿）。
const SOURCE_FILES: &[(&str, &str)] = &[
    ("methods.rs", "../agent-acp/src/methods.rs"),
    ("types.rs", "../agent-core/src/run_status/types.rs"),
];

/// 声明头（含 `{`，供行级定位；`pub enum ToolKind` 在别的 crate 另有同名物，
/// 但本文件只看 include_str! 绑定的文本，互不可见）。
const TOOL_KIND_HEADER: &str = "pub enum ToolKind {";
/// 声明头。
const TOOL_CALL_STATUS_HEADER: &str = "pub enum ToolCallStatus {";
/// 声明头。
const TOOL_CALL_UPDATE_HEADER: &str = "pub struct ToolCallUpdate {";
/// 探针声明头。
const RUN_STATE_HEADER: &str = "pub enum RunState {";

/// 两个枚举钉死的派生规则。
const SNAKE_CASE: &str = "snake_case";
/// `ToolCallUpdate` 容器钉死的字段派生规则（`toolCallId` 等 wire 键由它保证）。
const CAMEL_CASE: &str = "camelCase";
/// `meta` 字段的显式 wire 键（camelCase 派生会给 `meta`，须显式压住）。
const META_WIRE_KEY: &str = "_meta";

/// `ToolKind` 期望表：变体名 ⇒ wire kind（手写冻结真相）。
///
/// 这张表就是契约：正当扩充新变体 = 同步加一行表项（不红）；
/// 单侧改动（源加了变体不登记 / 表登记了不存在的变体）⇒ 红。
const TOOL_KIND_WIRE: &[(&str, &str)] = &[
    ("Read", "read"),
    ("Edit", "edit"),
    ("Delete", "delete"),
    ("Move", "move"),
    ("Search", "search"),
    ("Execute", "execute"),
    ("Think", "think"),
    ("Other", "other"),
];

/// `ToolCallStatus` 期望表：变体名 ⇒ wire status。
const TOOL_CALL_STATUS_WIRE: &[(&str, &str)] = &[
    ("Pending", "pending"),
    ("InProgress", "in_progress"),
    ("Completed", "completed"),
    ("Failed", "failed"),
];

/// 探针 `RunState` 期望表：变体名 ⇒ wire 值（多词变体证明 camel→snake 派生）。
const RUN_STATE_WIRE: &[(&str, &str)] = &[
    ("Idle", "idle"),
    ("Running", "running"),
    ("AwaitingInput", "awaiting_input"),
    ("Errored", "errored"),
    ("Terminated", "terminated"),
];

// ---- 扫描器（只依赖源码文本；真源断言与合成样本自证走同一批函数）----

/// 声明头的行号（0 基）；找不到 ⇒ None（fail-closed 由调用方变红）。
fn item_header_at(source: &str, header: &str) -> Option<usize> {
    source
        .lines()
        .position(|l| l.trim_start().starts_with(header))
}

/// 条目体逐行展开：`(相对条目体的花括号深度, trim 后的行)`，深度 0 = 变体 /
/// 字段 / 它们的属性行，≥1 = 结构体变体的字段；吃到配平的收尾 `}` 为止（该行
/// 不入列）；行数耗尽仍未配平（或多出 `}`）⇒ Err——不配平必须红。
fn item_body_lines<'a>(source: &'a str, header: &str) -> Result<Vec<(usize, &'a str)>, String> {
    let header_at = item_header_at(source, header)
        .ok_or_else(|| format!("源码里找不到 `{header}`——形态变了"))?;
    let mut out = Vec::new();
    let mut depth = 1usize; // 头行的 `{` 已开一层
    for line in source.lines().skip(header_at + 1) {
        let t = line.trim();
        let before = depth - 1;
        for c in t.chars() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => return Err("条目体花括号不配平：多出收尾 `}`".into()),
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth == 0 {
            return Ok(out); // 恰好吃到收尾 `}` 为止
        }
        out.push((before, t));
    }
    Err("条目体到文件尾仍未配平收尾 `}`——花括号不配平".into())
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

/// ① 扫条目体的**变体名**（保留源码顺序）；深度 0 代码行取不出前导标识符 ⇒ Err。
fn scan_variants(source: &str, header: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (depth, t) in item_body_lines(source, header)? {
        if depth > 0 || !is_code(t) {
            continue; // 结构体变体字段 / 空行 / 注释 / 属性行
        }
        let name = leading_ident(t);
        if name.is_empty() {
            return Err(format!("条目体深度 0 处出现解读不了的代码行：`{t}`"));
        }
        out.push(name);
    }
    Ok(out)
}

/// ② 扫条目体内深度 0 的属性行（变体级/字段级 serde 属性断言共用）。
fn scan_depth0_attrs(source: &str, header: &str) -> Result<Vec<String>, String> {
    Ok(item_body_lines(source, header)?
        .into_iter()
        .filter(|(depth, t)| *depth == 0 && t.starts_with("#["))
        .map(|(_, t)| t.to_owned())
        .collect())
}

/// ③ 扫**变体级** `#[serde(rename = "…")]`：会让该变体偏离容器 snake_case 规则、
/// 绕过期望表 ⇒ 禁止；变体级 `rename_all` 作用于字段、不碰 tag ⇒ 允许（按
/// `rename` 后紧跟 `=` 还是 `_` 区分，见 `has_bare_rename_attr`）。
fn scan_variant_level_renames(source: &str, header: &str) -> Result<Vec<String>, String> {
    Ok(scan_depth0_attrs(source, header)?
        .into_iter()
        .filter(|t| has_bare_rename_attr(t))
        .collect())
}

/// ④ 扫**变体级** `#[serde(other)]` 兜底旗标：把未知 wire 值静默吞进兜底变体，
/// 破坏「未知 kind 即协议违例」的 fail-closed 语义 ⇒ 禁止（`Other` 变体是普通
/// 变体、wire 词 `other`，与此无关）。
fn scan_serde_other_flags(source: &str, header: &str) -> Result<Vec<String>, String> {
    Ok(scan_depth0_attrs(source, header)?
        .into_iter()
        .filter(|t| has_serde_other_flag(t))
        .collect())
}

/// ⑤ 从声明头上方属性行取 `rename_all = "…"`；找不到 ⇒ Err（fail-closed）。
fn container_rename_all(source: &str, header: &str) -> Result<String, String> {
    let header_at = item_header_at(source, header)
        .ok_or_else(|| format!("源码里找不到 `{header}`——形态变了"))?;
    let above: Vec<&str> = source.lines().take(header_at).collect();
    for line in above.into_iter().rev() {
        let t = line.trim();
        if !t.starts_with("#[") {
            if t.starts_with("//") || t.is_empty() {
                continue; // 文档注释可垫在属性行之上
            }
            break; // 撞到其它代码即停（再往上不是本条目的属性了）
        }
        if let Some(v) = attr_value(t, "rename_all") {
            return Ok(v);
        }
    }
    Err(format!(
        "`{header}` 上方找不到带 `rename_all = \"…\"` 的 serde 属性行——派生规则被摘或形态变了"
    ))
}

/// ⑥ 条目体内深度 0 的**字段代码行**（去尾逗号；结构体接线断言用）。
fn field_lines(source: &str, header: &str) -> Result<Vec<String>, String> {
    Ok(item_body_lines(source, header)?
        .into_iter()
        .filter(|(depth, t)| *depth == 0 && is_code(t))
        .map(|(_, t)| t.trim_end_matches(',').to_owned())
        .collect())
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

/// 属性文本里是否有裸词 `other`（前后均非标识符字符）——`#[serde(other)]`
/// 兜底旗标；先去掉 `"…"` 字面量，防 `rename = "other"` 的值误报。
fn has_serde_other_flag(attr: &str) -> bool {
    let mut bare = String::new();
    let mut in_str = false;
    for c in attr.chars() {
        match c {
            '"' => in_str = !in_str,
            _ if !in_str => bare.push(c),
            _ => {}
        }
    }
    let bytes = bare.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = bare[from..].find("other") {
        let at = from + rel;
        let after = at + "other".len();
        let left_ok = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let right_ok =
            after >= bytes.len() || !(bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_');
        if left_ok && right_ok {
            return true;
        }
        from = after;
    }
    false
}

/// 从属性行取 `<key> = "<value>"` 的 value（引号内精确文本；键名前不许是
/// 标识符字符，防误吸 `xx_rename_all = "…"` 这类）。
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

/// serde `rename_all = "snake_case"` 的派生规则（逐字复刻 serde：每个**大写**
/// 字符前插 `_`，再转小写）。
fn serde_snake_case(variant: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in variant.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

// ---- 集合判定（真源断言与合成样本自证共用）----

/// 期望表的变体名集合。
fn table_variants(table: &[(&'static str, &'static str)]) -> BTreeSet<&'static str> {
    table.iter().map(|(v, _)| *v).collect()
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

/// 期望表自检：非空、变体名与 wire 值各自无重复（两个变体撞同一个线上词
/// 会让反解歧义，先在表侧红）。
fn assert_table_shape(table: &[(&'static str, &'static str)], label: &str) {
    assert!(!table.is_empty(), "{label} 期望表被清空——契约真相不许删");
    assert_eq!(
        table_variants(table).len(),
        table.len(),
        "{label} 期望表变体名有重复"
    );
    let tags: BTreeSet<&str> = table.iter().map(|(_, t)| *t).collect();
    assert_eq!(
        tags.len(),
        table.len(),
        "{label} 期望表 wire 值有重复（两个变体撞同一个线上词，反解歧义）"
    );
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
         wire 拼写是对端消费的线上词，正当扩充新变体须同步登记期望表"
    );
    assert_eq!(
        scanned.len(),
        table.len(),
        "{label} 变体数与表项数不一致（有变体或表项重复）"
    );
}

/// rename_all 枚举的统一断言面：容器规则钉死 + 变体集 ⇄ 期望表双向相等 +
/// 变体级 rename / `#[serde(other)]` 兜底禁令 + 表与派生规则互证。谁改了会红：
/// 改/摘 `rename_all`、改名/增删变体（不登记表）、挂显式 `rename`、加兜底变体。
fn assert_enum_wire_surface(
    source: &str,
    header: &str,
    table: &[(&'static str, &'static str)],
    label: &str,
) {
    assert_table_shape(table, label);

    let rename_all = container_rename_all(source, header)
        .unwrap_or_else(|e| panic!("{label} 的 serde 容器属性行必须能扫到：{e}"));
    assert_eq!(
        rename_all, SNAKE_CASE,
        "{label} 派生规则漂了（实为 {rename_all:?}）——期望表按 snake_case 手写，\
         规则一变表即整体失真，须同步改表"
    );

    let scanned =
        scan_variants(source, header).unwrap_or_else(|e| panic!("{label} 枚举体必须能解析：{e}"));
    assert_variants_match_table(&scanned, table, label);

    let offenders = scan_variant_level_renames(source, header).expect("枚举体必须能解析");
    assert!(
        offenders.is_empty(),
        "{label} 变体上出现 `rename = …`（{offenders:?}）——变体级 rename 令该变体 \
         偏离 snake_case 规则、绕过期望表；拼写只能整体随规则或同步改表"
    );

    let catch_alls = scan_serde_other_flags(source, header).expect("枚举体必须能解析");
    assert!(
        catch_alls.is_empty(),
        "{label} 出现 `#[serde(other)]` 兜底变体（{catch_alls:?}）——未知 wire 值会被 \
         静默吞进兜底变体，破坏「未知 kind 即协议违例」的 fail-closed 语义"
    );

    for (variant, wire) in table {
        assert_eq!(
            &serde_snake_case(variant),
            wire,
            "{label} 期望表里 {variant} ⇒ {wire} 与 serde snake_case 派生规则不一致——\
             表与规则须同源（容器规则已钉死、变体级 rename 与 other 兜底被禁）"
        );
    }
}

// ---- 断言面 ----

/// ① **ToolKind**：容器 snake_case + 变体集 ⇄ 期望表双向相等 + 变体级
/// rename / other 兜底禁令 + 表与派生互证（断言面见 `assert_enum_wire_surface`）。
#[test]
fn tool_kind_wire_surface_is_pinned() {
    assert_enum_wire_surface(METHODS_SOURCE, TOOL_KIND_HEADER, TOOL_KIND_WIRE, "ToolKind");
}

/// ② **ToolCallStatus**：与 ① 同构断言面。谁改了会红：同 ① 四类漂移。
#[test]
fn tool_call_status_wire_surface_is_pinned() {
    assert_enum_wire_surface(
        METHODS_SOURCE,
        TOOL_CALL_STATUS_HEADER,
        TOOL_CALL_STATUS_WIRE,
        "ToolCallStatus",
    );
}

/// ③ **ToolCallUpdate 载荷键钉住**：camelCase 容器规则（`toolCallId` 等 wire 键
/// 由它保证）+ 接线断言（两个枚举仍被载荷字段引用，防死代码；`tool_call_id`
/// 主键在，事件关联靠它）+ `_meta` 显式 rename 行在。谁改了会红：容器规则被
/// 改/摘、kind/status/tool_call_id 字段被删或改名、`_meta` rename 行被删。
#[test]
fn tool_call_update_payload_keys_are_pinned() {
    let rename_all = container_rename_all(METHODS_SOURCE, TOOL_CALL_UPDATE_HEADER)
        .expect("ToolCallUpdate 的 serde 容器属性行必须能扫到");
    assert_eq!(
        rename_all, CAMEL_CASE,
        "ToolCallUpdate 字段派生规则漂了（实为 {rename_all:?}）——wire 键 toolCallId / \
         rawInput 等由它保证，改了须先核对对端再同步改契约"
    );

    let fields = field_lines(METHODS_SOURCE, TOOL_CALL_UPDATE_HEADER)
        .expect("ToolCallUpdate 结构体必须能解析");
    for expected in [
        "pub tool_call_id: String",
        "pub kind: Option<ToolKind>",
        "pub status: Option<ToolCallStatus>",
    ] {
        assert!(
            fields.iter().any(|f| f == expected),
            "ToolCallUpdate 缺字段 `{expected}`（实有 {fields:?}）——载荷接线变了：\
             枚举失去引用即死代码，toolCallId 主键丢失即事件无法关联"
        );
    }

    let attrs = scan_depth0_attrs(METHODS_SOURCE, TOOL_CALL_UPDATE_HEADER)
        .expect("ToolCallUpdate 必须能解析");
    assert!(
        attrs.iter().any(|t| has_bare_rename_attr(t)
            && attr_value(t, "rename").as_deref() == Some(META_WIRE_KEY)),
        "ToolCallUpdate 里找不到 `rename = \"{META_WIRE_KEY}\"` 属性行——meta 字段的 \
         wire 键会退回 camelCase 派生的 `meta`，`codebuddy.ai/*` 命名空间载体丢失"
    );
}

/// ④ **fail-closed 行为探针**：dev-dep `agent-core::RunState` 编译进本测试，
/// 其源文本被 ⑤ 钉成与 agent-acp 两枚举同构。本测试实证该同构形态的 serde
/// 派生行为：未知值必 Err（报错预期集 = 全变体集，无兜底旁证）、拼写逐字精确、
/// 已知值可反解、多词变体 camel→snake 双向派生。同属性 ⇒ 结论迁移到
/// ToolKind / ToolCallStatus。
#[test]
fn unknown_wire_values_fail_closed_probe() {
    // 卡面样本：未知 kind 与未知 status。
    let sample = json!({"kind": "hacked_kind", "status": "done"});
    let err = serde_json::from_value::<RunState>(sample["kind"].clone())
        .expect_err("未知 wire 值必须反序列化失败——被兜底吞掉即协议违例静默化");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown variant"),
        "失败原因必须是 unknown variant（实为：{msg}）"
    );
    for (_, wire) in RUN_STATE_WIRE {
        assert!(
            msg.contains(&format!("`{wire}`")),
            "unknown variant 报错须列全预期集（缺 `{wire}`——若真有兜底，此处会缺词）：{msg}"
        );
    }
    assert!(
        serde_json::from_value::<RunState>(sample["status"].clone()).is_err(),
        "未知 status（\"done\"）也必须被拒"
    );

    // 拼写逐字精确：PascalCase 原名不是 wire 词，同拒（无大小写归一化）。
    assert!(
        serde_json::from_value::<RunState>(json!("AwaitingInput")).is_err(),
        "PascalCase 原名必须同拒——wire 词只能逐字精确匹配"
    );

    // 正对照：已知词可反解；多词变体 camel→snake 派生双向成立。
    assert_eq!(
        serde_json::from_value::<RunState>(json!("awaiting_input"))
            .expect("已知 wire 词必须可反解"),
        RunState::AwaitingInput
    );
    assert_eq!(json!(RunState::AwaitingInput), json!("awaiting_input"));
}

/// ⑤ **探针同构属性钉住**：探针 `RunState` 的源文本走与 ①② 完全相同的断言面
/// ——「探针与 ToolKind / ToolCallStatus 同属性」的机器证明，④ 的行为结论借此
/// 迁移；探针源若漂（加兜底、改 rename_all），本测试先红、④ 前提即失效。
#[test]
fn probe_isomorphic_attrs_are_pinned() {
    assert_enum_wire_surface(
        RUN_STATE_SOURCE,
        RUN_STATE_HEADER,
        RUN_STATE_WIRE,
        "RunState(探针)",
    );
}

/// ⑥ **路径存在性钉死**：两份源文件必须仍在该处（include_str! 绑定的是编译期
/// 路径，文件被移走会让本守卫整体静默失效）。
#[test]
fn source_files_still_exist() {
    for (label, rel) in SOURCE_FILES {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        assert!(
            path.exists(),
            "{label} 被移动了（期望在 {}）——include_str! 绑定的是旧路径，\
             本守卫会静默失效，须先核对路径再放行",
            path.display()
        );
    }
}

/// ⑦ **扫描器防空**：声明头被移走 ⇒ 扫描器 Err（fail-closed，不是 Ok(空) 假绿）；
/// 花括号不配平 ⇒ Err 而非静默漏判。全部扫描器共用 `item_body_lines` 的头定位
/// 与配平，`scan_depth0_attrs` 的两个包装器随其一荣俱荣，抽代表面即可。
#[test]
fn scanner_fails_closed_on_broken_source() {
    let base = "\
#[serde(rename_all = \"snake_case\")]
pub enum Demo {
    /// 已开始。
    Pending,
    /// 执行中。
    InProgress,
}
";
    let header = "pub enum Demo {";
    assert!(!scan_variants(base, header).expect("基线可解析").is_empty());
    assert_eq!(
        container_rename_all(base, header).expect("基线可解析"),
        SNAKE_CASE
    );

    // 声明头被改走 ⇒ 必须 Err，不许静默空过（三条代表路径 + 两个薄包装）。
    let header_gone = base.replace(header, "pub enum Demo2 {");
    assert!(
        scan_variants(&header_gone, header).is_err()
            && scan_depth0_attrs(&header_gone, header).is_err()
            && scan_variant_level_renames(&header_gone, header).is_err()
            && scan_serde_other_flags(&header_gone, header).is_err()
            && field_lines(&header_gone, header).is_err(),
        "声明头被移走后全部体扫描器必须 Err 红"
    );
    assert!(
        container_rename_all(&header_gone, header).is_err(),
        "声明头被移走后容器属性行扫描必须 Err 红"
    );

    // 花括号不配平（多开一个 `{` 永不闭合）⇒ 必须 Err。
    let unbalanced = base.replace("    InProgress,\n", "    InProgress {\n");
    assert_ne!(unbalanced, base, "替换没生效，样本构造有误");
    assert!(
        scan_variants(&unbalanced, header).is_err(),
        "花括号不配平必须 Err 红"
    );
}

/// ⑧ **合成样本自证**：用假源码证明各漂移都真能红（真源不为测试而改，先例：
/// wire_tag_enum_trio.rs）——派生规则被改/摘、变体改名、挂 rename、加 other
/// 兜底；结构体侧：字段被删、`_meta` rename 被删。
#[test]
fn synthetic_samples_catch_drifts() {
    // —— 枚举基线（ToolKind 形状：snake_case、无变体级 rename、无兜底）。
    let base = "\
#[serde(rename_all = \"snake_case\")]
pub enum ToolKind {
    /// 读。
    Read,
    /// 执行。
    Execute,
}
";
    let header = TOOL_KIND_HEADER;
    let table: &[(&str, &str)] = &[("Read", "read"), ("Execute", "execute")];
    assert_enum_wire_surface(base, header, table, "基线");

    // 漂移 A：rename_all 被改 ⇒ 扫出不同的规则；被整个摘掉 ⇒ Err（fail-closed）。
    let rule_changed = base.replace("snake_case", "kebab-case");
    assert_ne!(rule_changed, base, "替换没生效，样本构造有误");
    assert_ne!(
        container_rename_all(&rule_changed, header).expect("属性行仍在"),
        SNAKE_CASE,
        "派生规则被改后必须扫出不同的规则"
    );
    let attr_gone = base.replace("#[serde(rename_all = \"snake_case\")]\n", "");
    assert!(
        container_rename_all(&attr_gone, header).is_err(),
        "rename_all 被摘后必须 Err 红，不许静默通过"
    );

    // 漂移 B：变体改名 ⇒ 双向差集必须同时报「多余 + 缺失」。
    let renamed = base.replace("Read,", "Label,");
    assert_ne!(renamed, base, "替换没生效，样本构造有误");
    let variants = scan_variants(&renamed, header).expect("renamed 可解析");
    let (extra, missing) = bidirectional_diff(&variants, &table_variants(table));
    assert_eq!(extra, vec!["Label"], "改名后的变体必须作为多余项报出");
    assert_eq!(missing, vec!["Read"], "旧名必须作为缺失项报出");

    // 漂移 C：变体挂上显式 rename ⇒ 必须抓出。
    let with_rename = base.replace("    Read,", "    #[serde(rename = \"readV2\")]\n    Read,");
    assert_ne!(with_rename, base, "替换没生效，样本构造有误");
    let offenders = scan_variant_level_renames(&with_rename, header).expect("可解析");
    assert_eq!(
        offenders.len(),
        1,
        "变体级 rename 必须被抓出：{offenders:?}"
    );

    // 漂移 D：加 `#[serde(other)]` 兜底 ⇒ 必须抓出（fail-closed 的死穴）。
    let with_catch_all = base.replace("    Execute,", "    #[serde(other)]\n    Execute,");
    assert_ne!(with_catch_all, base, "替换没生效，样本构造有误");
    let catch_alls = scan_serde_other_flags(&with_catch_all, header).expect("可解析");
    assert_eq!(catch_alls.len(), 1, "other 兜底必须被抓出：{catch_alls:?}");

    // —— 结构体基线（ToolCallUpdate 形状：camelCase 容器 + _meta 显式 rename）。
    let upd = "\
#[serde(rename_all = \"camelCase\")]
pub struct ToolCallUpdate {
    /// 主键。
    pub tool_call_id: String,
    /// 类别。
    pub kind: Option<ToolKind>,
    /// 元数据。
    #[serde(default, rename = \"_meta\", skip_serializing_if = \"Option::is_none\")]
    pub meta: Option<Value>,
}
";
    let upd_header = TOOL_CALL_UPDATE_HEADER;
    let fields = field_lines(upd, upd_header).expect("基线可解析");
    for expected in ["pub tool_call_id: String", "pub kind: Option<ToolKind>"] {
        assert!(
            fields.iter().any(|f| f == expected),
            "基线必须含 `{expected}`：{fields:?}"
        );
    }
    assert_eq!(
        container_rename_all(upd, upd_header).expect("基线可解析"),
        CAMEL_CASE
    );
    let upd_attrs = scan_depth0_attrs(upd, upd_header).expect("基线可解析");
    assert!(
        upd_attrs.iter().any(|t| has_bare_rename_attr(t)
            && attr_value(t, "rename").as_deref() == Some(META_WIRE_KEY)),
        "基线必须含 _meta rename 行：{upd_attrs:?}"
    );

    // 漂移 E：字段被删 ⇒ 接线断言必须报缺。
    let field_gone = upd.replace("    pub kind: Option<ToolKind>,\n", "");
    assert_ne!(field_gone, upd, "替换没生效，样本构造有误");
    let gone_fields = field_lines(&field_gone, upd_header).expect("可解析");
    assert!(
        !gone_fields
            .iter()
            .any(|f| f == "pub kind: Option<ToolKind>"),
        "删字段后接线断言必须报缺：{gone_fields:?}"
    );

    // 漂移 F：`_meta` rename 被删 ⇒ 断言必须落空。
    let meta_gone = upd.replace("rename = \"_meta\", ", "");
    assert_ne!(meta_gone, upd, "替换没生效，样本构造有误");
    let gone_attrs = scan_depth0_attrs(&meta_gone, upd_header).expect("可解析");
    assert!(
        !gone_attrs.iter().any(|t| has_bare_rename_attr(t)
            && attr_value(t, "rename").as_deref() == Some(META_WIRE_KEY)),
        "删 _meta rename 后断言必须落空：{gone_attrs:?}"
    );
}
