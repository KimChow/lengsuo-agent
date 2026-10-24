//! `SessionUpdate` 变体 wire tag 拼写面守卫（W2）。
//!
//! # 这条守卫治什么病
//!
//! `crates/agent-acp/src/methods.rs` 的 `SessionUpdate`（`session/update` 通知载荷
//! `SessionNotification.update` 的本体）是会话增量事件的**判别面**：
//!
//! ```text
//! #[serde(tag = "sessionUpdate", rename_all = "snake_case")]
//! pub enum SessionUpdate { AgentMessageChunk(..), …, AvailableCommands{..} }
//! ```
//!
//! serde 属性自带 wire 契约：判别键 `sessionUpdate` 与 8 个变体的 tag 值
//! （`agent_message_chunk` / `agent_thought_chunk` / `user_message_chunk` /
//! `tool_call` / `tool_call_update` / `plan` / `current_mode` / `available_commands`）
//! 都是外部 ACP client 直接消费的线上词。三个结构体变体（Plan / CurrentMode /
//! AvailableCommands）另挂 `rename_all = "camelCase"`，但它只作用于**字段名**，
//! 不影响 tag 值。
//!
//! 实测缺口（2026-10-24，base bd0f75c）：
//!
//! - `crates/contracts-check/` 全 crate 对字面量 `sessionUpdate` **零命中**——契约侧
//!   不知道这个判别键的存在；control 信封的 tag 面已有 `tests/acp_envelope_tag_surface`
//!   守，`session/update` 的增量事件 tag 面是对称缺口。
//! - 唯一的 tag 拼写断言是 `agent-acp` 测试样例 JSON 里的
//!   `"sessionUpdate":"agent_message_chunk"`——8 个变体只有 1 个被字面量钉住。
//! - `golden_vectors.rs::session_update_all_variants_round_trip` 对 8 个变体做
//!   round-trip，但**两边同走 serde**：tag 拼写漂移（改 rename_all、给某变体加
//!   `rename`）时序列化/反序列化依然自洽、测试不红；只有跨进程对端会炸。
//!
//! # 做法与口径
//!
//! `contracts-check` 的 dev-dependencies **没有** `agent-acp`，而 `Cargo.toml` 属
//! 共享区不得改，故照 `tests/task_status_drift_guard.rs` / `tests/tasks_method_surface.rs`
//! 的手法走**源码文本扫描**：`include_str!` 编译期绑定四份源文件文本（源文件被移动
//! = 编译期红），另用 `CARGO_MANIFEST_DIR` 相对路径断言文件仍在该处。
//!
//! - **只守关系、不锁行数**；变体集 ⇄ 期望表双向集合相等是本守卫的**目的**
//!   （新变体登记 tag 后才算接入 wire），不算「锁数量必红」反例。
//! - 扫描器**防空**：枚举找不到 / 花括号不配平 ⇒ `Err`（fail-closed 红），不许静默漏判。
//! - 合成样本自证：`synthetic_samples_catch_three_drifts` 用假源码证明「tag 键被改 /
//!   变体加 rename / 变体删除」三种漂移都真能红（真源不为测试而改）。

use std::collections::BTreeSet;
use std::path::Path;

/// 被扫源文件文本（**编译期**绑定：源文件改动即触发重编）。
const METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");
/// round-trip 覆盖面所在文件。
const GOLDEN_VECTORS_SOURCE: &str = include_str!("../../agent-acp/tests/golden_vectors.rs");
/// wire 样例锚 1。
const HOST_ROLE_SOURCE: &str = include_str!("../../agent-acp/tests/host_role.rs");
/// wire 样例锚 2。
const REPLAY_TYPES_SOURCE: &str = include_str!("../../agent-acp/tests/replay_types.rs");

/// (标签, 相对 `CARGO_MANIFEST_DIR` 路径)：存在性断言用（防 include_str! 绑到旧路径后假绿）。
const SOURCE_FILES: &[(&str, &str)] = &[
    ("methods.rs", "../agent-acp/src/methods.rs"),
    ("golden_vectors.rs", "../agent-acp/tests/golden_vectors.rs"),
    ("host_role.rs", "../agent-acp/tests/host_role.rs"),
    ("replay_types.rs", "../agent-acp/tests/replay_types.rs"),
];

/// 枚举声明头（含 `{`，供行级定位）。
const ENUM_HEADER: &str = "pub enum SessionUpdate {";
/// round-trip 覆盖面所在的测试函数名。
const ROUND_TRIP_FN: &str = "session_update_all_variants_round_trip";
/// 构造器前缀。
const CONSTRUCTOR_PREFIX: &str = "SessionUpdate::";
/// wire 键名字面量（样例 JSON 与 `u.get(…)` 共用此拼写）。
const KEY_LITERAL: &str = "\"sessionUpdate\"";

/// **钉死的判别键**（session/update 载荷里增量事件的内部标签字段名）。
const TAG_KEY: &str = "sessionUpdate";
/// **钉死的 tag 派生规则**。
const RENAME_RULE: &str = "snake_case";

/// **期望表**：变体名 ⇒ wire tag（8 项，ACP 19 册 §2 事件目录）。
///
/// 这张表就是契约：正当扩充新变体 = 同步加一行表项（不红）；
/// 单侧改动（源加了变体不登记 / 表登记了不存在的变体）⇒ 红。
const EXPECTED_WIRE_TAGS: &[(&str, &str)] = &[
    ("AgentMessageChunk", "agent_message_chunk"),
    ("AgentThoughtChunk", "agent_thought_chunk"),
    ("UserMessageChunk", "user_message_chunk"),
    ("ToolCall", "tool_call"),
    ("ToolCallUpdate", "tool_call_update"),
    ("Plan", "plan"),
    ("CurrentMode", "current_mode"),
    ("AvailableCommands", "available_commands"),
];

// ---- 扫描器（只依赖源码文本；真源断言与合成样本自证走同一批函数）----

/// 枚举声明头的行号（0 基）；找不到 ⇒ None（fail-closed 由调用方变红）。
fn enum_header_at(source: &str) -> Option<usize> {
    source
        .lines()
        .position(|l| l.trim_start().starts_with(ENUM_HEADER))
}

/// 枚举体逐行展开：`(进入该行前的花括号深度, trim 后的行)`，深度**相对枚举体**
/// 计（变体级属性行/变体声明为 0，结构体变体的字段为 ≥1），吃到配平的收尾 `}`
/// 为止（该行不入列）；行数耗尽仍未配平（或多出 `}`）⇒ Err——花括号不配平必须红。
fn enum_body_lines(source: &str) -> Result<Vec<(usize, &str)>, String> {
    let header_at = enum_header_at(source)
        .ok_or_else(|| format!("源码里找不到 `{ENUM_HEADER}`——methods.rs 形态变了"))?;
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
fn scan_variants(source: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (depth, t) in enum_body_lines(source)? {
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

/// ② 扫枚举体内**变体级** `#[serde(rename = "…")]`（只看深度 0 的属性行）。
///
/// 变体级 `rename` 会让该变体 tag 偏离 snake_case 规则、绕过期望表 ⇒ 禁止；
/// 变体级 `rename_all` 作用于**字段**、不碰 tag ⇒ 允许。两者按 `rename` 后
/// 紧跟 `=` 还是 `_` 区分（`has_bare_rename_attr`）。
fn scan_variant_level_renames(source: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (depth, t) in enum_body_lines(source)? {
        if depth > 0 || !t.starts_with("#[") {
            continue; // 字段级属性在深度 ≥1：改名只动字段，不偏离 tag，不看
        }
        if has_bare_rename_attr(t) {
            out.push(t.to_owned());
        }
    }
    Ok(out)
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

/// ③ 从属性行取 `<key> = "<value>"` 的 value（引号内精确文本；键名前不许是
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

/// 扫 `pub enum SessionUpdate` 上方属性行里的 `(tag, rename_all)` 取值。
///
/// 属性行找不到 ⇒ Err（fail-closed：判别键被摘掉/形态变了都算红）。
fn scan_tag_attr(source: &str) -> Result<(String, String), String> {
    let header_at = enum_header_at(source)
        .ok_or_else(|| format!("源码里找不到 `{ENUM_HEADER}`——methods.rs 形态变了"))?;
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
            return Ok((tag, attr_value(t, "rename_all").unwrap_or_default()));
        }
    }
    Err(format!(
        "`{ENUM_HEADER}` 上方找不到带 `tag = \"…\"` 的 serde 属性行——判别键被摘或形态变了"
    ))
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

/// 抽 `fn <name>` 的函数体（首个 `{` 起、花括号配平到收尾 `}`，不含两端括号）。
fn fn_body<'a>(source: &'a str, name: &str) -> Result<&'a str, String> {
    let marker = format!("fn {name}");
    let at = source
        .find(&marker)
        .ok_or_else(|| format!("源码里找不到 `{marker}`——golden_vectors.rs 形态变了"))?;
    let open = source[at..]
        .find('{')
        .ok_or_else(|| format!("`{marker}` 之后没有 `{{`"))?;
    let body_start = at + open;
    let mut depth = 0i32;
    for (i, c) in source[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&source[body_start + 1..body_start + i]);
                }
            }
            _ => {}
        }
    }
    Err(format!("`{marker}` 函数体花括号不配平"))
}

/// ④ 抽 round-trip 测试函数体代码行上的所有 `SessionUpdate::<Ident>` 构造器名
/// （注释里的示例不算构造面）。
fn scan_round_trip_variants(source: &str) -> Result<Vec<String>, String> {
    let body = fn_body(source, ROUND_TRIP_FN)?;
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(CONSTRUCTOR_PREFIX) {
            let at = from + rel + CONSTRUCTOR_PREFIX.len();
            let ident = leading_ident(line.get(at..).unwrap_or(""));
            if !ident.is_empty() {
                out.push(ident);
            }
            from = at;
            if from >= line.len() {
                break;
            }
        }
    }
    Ok(out)
}

/// ⑤ 抽文本里所有 `"sessionUpdate": "<value>"` 键值对的 value（引号内精确文本）。
/// `u.get("sessionUpdate")` 这类只引用键名、后面没有 `: "…"` 的，自然跳过。
fn scan_session_update_values(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(KEY_LITERAL) {
            let after = from + rel + KEY_LITERAL.len();
            if let Some(rest) = line.get(after..).map(str::trim_start) {
                if let Some(v) = rest.strip_prefix(':').map(str::trim_start) {
                    if let Some(v) = v.strip_prefix('"') {
                        if let Some(end) = v.find('"') {
                            out.push(v[..end].to_owned());
                        }
                    }
                }
            }
            from = after;
            if from >= line.len() {
                break;
            }
        }
    }
    out
}

// ---- 集合判定（真源断言与合成样本自证共用）----

/// 期望表的变体名集合。
fn expected_variant_set() -> BTreeSet<&'static str> {
    EXPECTED_WIRE_TAGS.iter().map(|(v, _)| *v).collect()
}

/// 期望表的 tag 值集合。
fn expected_tag_set() -> BTreeSet<&'static str> {
    EXPECTED_WIRE_TAGS.iter().map(|(_, t)| *t).collect()
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

// ---- 断言面 ----

/// ① **tag 键钉死**：`SessionUpdate` 的 serde 属性行必须精确声明
/// `tag = "sessionUpdate"` 且 `rename_all = "snake_case"`（引号内字面量精确匹配；
/// 属性行找不到 ⇒ Err 红，fail-closed）。
#[test]
fn tag_key_and_rename_rule_are_pinned() {
    let (tag, rename_all) =
        scan_tag_attr(METHODS_SOURCE).expect("SessionUpdate 的 serde 属性行必须能扫到");
    assert_eq!(
        tag, TAG_KEY,
        "session/update 载荷的判别键漂了（实为 {tag:?}）——外部 ACP client 按 \
         {TAG_KEY:?} 取增量事件，键一改全线失聪"
    );
    assert_eq!(
        rename_all, RENAME_RULE,
        "tag 值派生规则漂了（实为 {rename_all:?}）——期望表按 snake_case 手写，\
         规则一变表即整体失真，须同步改表"
    );
}

/// ② **变体集 ⇄ 期望表双向集合相等**：实现面变体集与 EXPECTED_WIRE_TAGS 的
/// 变体名侧必须一致（正当扩充新变体 = 同步加一行表项，不红；单侧改动 ⇒ 红）。
#[test]
fn variants_match_expected_table_bidirectionally() {
    assert_eq!(
        expected_variant_set().len(),
        EXPECTED_WIRE_TAGS.len(),
        "期望表变体名有重复"
    );
    assert_eq!(
        expected_tag_set().len(),
        EXPECTED_WIRE_TAGS.len(),
        "期望表 tag 值有重复（两个变体撞同一个 wire 词，反解歧义）"
    );

    let scanned = scan_variants(METHODS_SOURCE).expect("枚举体必须能解析");
    let (extra, missing) = bidirectional_diff(&scanned, &expected_variant_set());
    assert!(
        extra.is_empty() && missing.is_empty(),
        "SessionUpdate 变体与期望表不同步：实现有而表缺 {extra:?}；表有而实现缺 \
         {missing:?}——sessionUpdate tag 是外部 client 消费的线上词，正当扩充新变体须\
         同步登记 EXPECTED_WIRE_TAGS"
    );
    assert_eq!(
        scanned.len(),
        EXPECTED_WIRE_TAGS.len(),
        "变体数与表项数不一致（有变体或表项重复）"
    );
}

/// ③ **期望表与派生规则互证**：表是手写的，这条证明每个表项 tag ==
/// serde_snake_case(变体)，即表与测试①钉死的容器级规则同源——变体级 rename
/// 绕行被测试④禁止后，表即 wire 真值。
#[test]
fn expected_tags_follow_snake_case_derivation() {
    for (variant, tag) in EXPECTED_WIRE_TAGS {
        assert_eq!(
            &serde_snake_case(variant),
            tag,
            "期望表里 {variant} ⇒ {tag} 与 serde snake_case 派生规则不一致——\
             表与规则须同源（规则由测试①钉死、变体级 rename 被测试④禁止）"
        );
    }
}

/// ④ **变体级 rename 禁令**：枚举体内不得出现变体级 `#[serde(rename = "…")]`
/// （它会让该变体 tag 偏离 snake_case 规则、绕过期望表）；变体级 `rename_all`
/// 作用于字段，允许，两者按 `rename` vs `rename_all` 区分。
#[test]
fn no_variant_level_rename_in_enum_body() {
    let offenders = scan_variant_level_renames(METHODS_SOURCE).expect("枚举体必须能解析");
    assert!(
        offenders.is_empty(),
        "SessionUpdate 变体上出现 `rename = …`（{offenders:?}）——变体级 rename 令该\
         变体 tag 偏离规则、绕过期望表；tag 拼写只能整体随规则或同步改表"
    );
}

/// ⑤ **round-trip 覆盖不缩水**：`session_update_all_variants_round_trip` 函数体里
/// 的 `SessionUpdate::Ident` 构造器集合 ⊇ 期望表变体集（防 round-trip 静默删样例、
/// 双向 serde 自洽测试失去代表面）。
#[test]
fn round_trip_sample_covers_every_expected_variant() {
    let ctors =
        scan_round_trip_variants(GOLDEN_VECTORS_SOURCE).expect("round-trip 函数体必须能解析");
    let (_, missing) = bidirectional_diff(&ctors, &expected_variant_set());
    assert!(
        missing.is_empty(),
        "session_update_all_variants_round_trip 不再覆盖全部期望变体（缺 {missing:?}）\
         ——round-trip 是唯一的全变体自洽测试，删样例须先核对契约面"
    );
}

/// ⑥ **wire 样例交叉锚**：host_role.rs 与 replay_types.rs 至少各有一处
/// `"sessionUpdate"` 字面量，且其值全部 ∈ 期望表值集（证明样例与表同源；
/// `u.get("sessionUpdate")` 这类无值键引用自然跳过）。
#[test]
fn wire_samples_anchor_expected_tag_values() {
    for (label, source) in [
        ("host_role.rs", HOST_ROLE_SOURCE),
        ("replay_types.rs", REPLAY_TYPES_SOURCE),
    ] {
        let values = scan_session_update_values(source);
        assert!(
            !values.is_empty(),
            "{label} 里扫不到任何带值的 \"sessionUpdate\" 样例——交叉锚失效或样例被删"
        );
        let stranger: Vec<&String> = values
            .iter()
            .filter(|v| !expected_tag_set().contains(v.as_str()))
            .collect();
        assert!(
            stranger.is_empty(),
            "{label} 里出现期望表之外的 sessionUpdate 值 {stranger:?}——样例与表必须同源，\
             新值须先登记 EXPECTED_WIRE_TAGS"
        );
    }
}

/// ⑦ **路径存在性钉死**：四份源文件必须仍在该处（include_str! 绑定的是编译期
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

/// ⑧ **合成样本自证**：用假源码证明三类漂移都真能红（先例：v0_action_run_mirror.rs）——
/// tag 键被改 / 变体加 rename / 变体删除，外加花括号不配平必须 Err（防空）。
#[test]
fn synthetic_samples_catch_three_drifts() {
    // 基线假源码：形如真枚举（含一个挂变体级 rename_all 的结构体变体，合法）。
    let base = "\
#[serde(tag = \"sessionUpdate\", rename_all = \"snake_case\")]
pub enum SessionUpdate {
    /// 正文流式增量。
    AgentMessageChunk(ContentBlock),
    /// plan 模式方案更新。
    #[serde(rename_all = \"camelCase\")]
    Plan {
        entries: Value,
    },
}
";
    // 基线本身必须读得出钉死的键与规则，且无变体级 rename（负对照）。
    let (tag, rule) = scan_tag_attr(base).expect("基线样本应有 serde 属性行");
    assert_eq!((tag.as_str(), rule.as_str()), (TAG_KEY, RENAME_RULE));
    assert!(
        scan_variant_level_renames(base)
            .expect("基线可解析")
            .is_empty(),
        "变体级 rename_all（作用于字段）不得误报成 rename"
    );

    // 漂移 A：tag 键被改 ⇒ 扫到的键不同；键被整个摘掉 ⇒ Err（fail-closed）。
    let key_changed = base.replace("tag = \"sessionUpdate\"", "tag = \"event\"");
    assert_ne!(key_changed, base, "替换没生效，样本构造有误");
    let (changed, _) = scan_tag_attr(&key_changed).expect("属性行仍在");
    assert_ne!(changed, TAG_KEY, "tag 键被改后必须扫出不同的键");
    let no_tag = base.replace("tag = \"sessionUpdate\", ", "");
    assert!(
        scan_tag_attr(&no_tag).is_err(),
        "tag 属性被摘后必须 Err 红，不许静默通过"
    );
    let rule_changed = base.replace("rename_all = \"snake_case\"", "rename_all = \"kebab-case\"");
    let (_, changed_rule) = scan_tag_attr(&rule_changed).expect("属性行仍在");
    assert_ne!(
        changed_rule, RENAME_RULE,
        "派生规则被改后必须扫出不同的规则"
    );

    // 漂移 B：变体挂上 `rename = "…"` ⇒ 必须抓出；rename_all 不许误伤。
    let renamed = base.replace(
        "    Plan {",
        "    #[serde(rename = \"planV2\")]\n    Plan {",
    );
    assert_ne!(renamed, base, "替换没生效，样本构造有误");
    let offenders = scan_variant_level_renames(&renamed).expect("renamed 可解析");
    assert_eq!(
        offenders.len(),
        1,
        "变体级 rename 必须被抓出：{offenders:?}"
    );

    // 漂移 C：变体删除 ⇒ 双向差集必须报缺失（与真源断言共用 bidirectional_diff）。
    let removed = base.replace("    AgentMessageChunk(ContentBlock),\n", "");
    assert_ne!(removed, base, "替换没生效，样本构造有误");
    let variants = scan_variants(&removed).expect("removed 可解析");
    let (_, missing) = bidirectional_diff(&variants, &expected_variant_set());
    assert!(
        missing.contains(&"AgentMessageChunk"),
        "删变体后双向差集必须报缺失：{missing:?}"
    );

    // 防空：枚举体花括号不配平（吞掉结构体变体的收尾 `}`）⇒ Err，不许静默漏判。
    let unbalanced = base.replace("    },\n", "");
    assert!(
        scan_variants(&unbalanced).is_err(),
        "枚举体花括号不配平必须 Err 红"
    );

    // 真源必须扫得到东西（否则上面各集合断言全是空对空）。
    assert!(!scan_variants(METHODS_SOURCE)
        .expect("真源可解析")
        .is_empty());
}
