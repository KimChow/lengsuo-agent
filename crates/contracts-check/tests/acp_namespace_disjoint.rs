//! ACP 三套命名空间的**归一化同名**登记与「同名不同面」绑定断言（W2 · lane proto）。
//!
//! 三处互不相干的手写派发面，各自决定「一帧归到哪一类」：事件流 kind 面（`EVENT_KINDS`
//! ← `agent-core/src/schema.rs` 的 `enum EventKind`）、ACP control subtype 面
//! （`ACP_CONTROL_SUBTYPES` ← `agent-acp/src/control.rs` 的 `enum ControlSubtype`）、
//! ACP wire method 面（`ACP_METHODS` ← `agent-acp/src/methods.rs` 的 `pub mod method`）。
//!
//! 归一化（**只**斜杠→下划线）后三面**共有同一个名** `session_update`；而三面是**三台
//! 各自独立的派发机**（事件流走网关 `/events`；control 与 wire method 走 ACP 通道，
//! 字面量漂移 ⇒ 落兜底臂 ⇒ 抛 `MalformedMessage` ⇒ **整帧被拒**）。
//!
//! 「同名不同面」本身**不是 bug**（`session/update` 与 `session_update` 同层不同义）⇒
//! 本文件**不**断言三面互不相交。真正的风险是**后人把两者归一化成同一个 key** ⇒
//! **静默串台**：一条 control 面帧被当成事件流 kind 处理。此前仓内**没有任何断言**把
//! 这件事挡住。
//!
//! 真相来源用 `include_str!` **编译期**绑定后**扫文本**（`agent-acp` 不是本 crate 的依赖、
//! `Cargo.toml` 属共享区本卡不得改 ⇒ 不能 `use agent_acp::…`）；扫描手写字符、不新增
//! 依赖、不用正则库。**只守关系，不锁数量/取值**（教训同款：`tests/method_ledger.rs`
//! 的 `assert_eq!(ACP_METHODS.len(), 13)`）。

use contracts_check::{ACP_CONTROL_SUBTYPES, ACP_METHODS, EVENT_KINDS};
use std::collections::{BTreeMap, BTreeSet};

/// 三个面的标识（登记表的第二列取值）。
const FACE_EVENT_KIND: &str = "event_kind";
const FACE_CONTROL_SUBTYPE: &str = "control_subtype";
const FACE_ACP_METHOD: &str = "acp_method";

/// 三面归一化后的**同名绑定关系**登记：`(归一化名, 面标识)`。一项 = 「该归一化名在这一面
/// 上出现过」；出现在两个及以上面上就有**多项**。⛔ **只登记绑定关系，不登记「哪些名字
/// 是冲突的」**——冲突与否**由断言现算**：否则登记面与实现面各写一份、必然漂移。
/// ⛔ **不锁表的长度和取值**。⛔ **放在测试文件里而非 `src/lib.rs`**：`lib.rs` 已有 688
/// 行，追加本表会越过 700 行硬限（红线 7）⇒ 按红线 7 规定外移。
pub const ACP_NAMESPACE_NORMALIZED_ALIASES: &[(&str, &str)] = &[
    ("session_update", FACE_EVENT_KIND), // agent-core EventKind::SessionUpdate
    ("session_update", FACE_CONTROL_SUBTYPE), // agent-acp ControlSubtype::SessionUpdate
    ("session_update", FACE_ACP_METHOD), // agent-acp method::SESSION_UPDATE
    ("elicitation_create", FACE_CONTROL_SUBTYPE), // agent-acp ControlSubtype::ElicitationCreate
    ("elicitation_create", FACE_ACP_METHOD), // agent-acp method::ELICITATION_CREATE
    ("elicitation_complete", FACE_CONTROL_SUBTYPE), // agent-acp ControlSubtype::ElicitationComplete
    ("elicitation_complete", FACE_ACP_METHOD), // agent-acp method::ELICITATION_COMPLETE
    ("session_request_permission", FACE_CONTROL_SUBTYPE), // agent-acp ControlSubtype::SessionRequestPermission
    ("session_request_permission", FACE_ACP_METHOD), // agent-acp method::SESSION_REQUEST_PERMISSION
];

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
const SCHEMA_SOURCE: &str = include_str!("../../agent-core/src/schema.rs");
const CONTROL_SOURCE: &str = include_str!("../../agent-acp/src/control.rs");
const METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");

/// 扫描出的一组 `(变体名/常量名, 字面量)` 对。
type Arms = Vec<(String, String)>;

// ---- 扫描器（手写字符扫描，不用正则库） ----

/// 是不是注释行（`//` / `///` / `//!`）——注释里的字面量**不算**实现面。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 一行的花括号净增减量（**注释行不算**——注释里的花括号不得影响块配对）。
fn brace_delta(line: &str) -> i32 {
    if is_comment_line(line) {
        return 0;
    }
    line.matches('{').count() as i32 - line.matches('}').count() as i32
}

/// 取右值里的字符串字面量（`=> "x"` 与 `= "x";` 两种收尾都吃）。
fn rhs_literal(rhs: &str) -> Option<&str> {
    let s = rhs
        .trim()
        .trim_end_matches(',')
        .trim()
        .trim_end_matches(';')
        .trim();
    s.strip_prefix('"')?.strip_suffix('"')
}

/// 是不是标识符或路径（`Foo` / `Foo::Bar`）——带引号的字面量**不是**。
fn is_word(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

/// serde/heck 的 `snake_case` 派生（**逐字复刻**：大写字母前切一刀，连续大写只在**最后一
/// 枚**前切 ⇒ `HTTPServer` ⇒ `http_server`；已有 `_` 不重复插入）。
fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut snake = String::new();
    for (i, ch) in chars.iter().enumerate() {
        if i > 0 && ch.is_uppercase() && chars[i - 1] != '_' {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied().unwrap_or(' ');
            if !prev.is_uppercase() || next.is_lowercase() {
                snake.push('_');
            }
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

/// **归一化**：**只**把斜杠换成下划线（口径被 `normalization_only_replaces_slashes` 钉死
/// ——口径若悄悄扩了，同名集合会**静默变大**）。
fn normalize(name: &str) -> String {
    name.replace('/', "_")
}

/// 截取 `header` 行**之后**、到该块**配对收尾**为止的块体（按花括号深度）。深度基线 = 1：
/// `header` 行**已含**一个未闭合的 `{` ⇒ 净深度回到 0 时正是块的收尾。
fn block_body<'a>(source: &'a str, header: &str) -> Vec<&'a str> {
    let start = source
        .lines()
        .position(|l| l.trim_start().starts_with(header))
        .unwrap_or_else(|| panic!("源文件里找不到 `{header}`——源文件形态变了"));
    let mut depth = 1i32;
    let mut out = Vec::new();
    for line in source.lines().skip(start + 1) {
        depth += brace_delta(line);
        out.push(line);
        if depth <= 0 {
            break; // 块收尾
        }
    }
    out
}

/// 扫出一个块里的 `(名字, 字面量)` 臂——**两种形态都吃**：① `变体 => "字面量"`，模式侧必须
/// 是**路径形态**（`EventKind::Thinking` / `Self::Foo`）⇒ `as_str()` 的臂收进来，`parse()`
/// 里 `"thinking" => …` 那类**字面量 => 变体**的臂（模式侧带引号）**不收**；② `pub const
/// <名字>: &str = "字面量";`。名字一律取路径的**末段**。
fn scan_arms(lines: &[&str]) -> Arms {
    let mut out = Vec::new();
    for line in lines {
        let t = line.trim();
        if is_comment_line(t) {
            continue;
        }
        if let Some(rest) = t.strip_prefix("pub const ") {
            let Some((name, rhs)) = rest.split_once(": &str") else {
                continue; // 只收 `&str` 常量
            };
            let Some(eq) = rhs.find('=') else {
                continue;
            };
            let Some(lit) = rhs_literal(&rhs[eq + 1..]) else {
                continue;
            };
            out.push((name.trim().to_owned(), lit.to_owned()));
            continue;
        }
        let Some((pattern, rhs)) = t.split_once("=>") else {
            continue;
        };
        let pattern = pattern.trim();
        if !is_word(pattern) {
            continue;
        }
        let Some(lit) = rhs_literal(rhs) else {
            continue;
        };
        let variant = pattern.rsplit("::").next().unwrap_or(pattern);
        out.push((variant.to_owned(), lit.to_owned()));
    }
    out
}

/// 扫出一个枚举面：`(参与此面的变体名, 参与此面的臂, 被剔除的兜底变体)`。`#[serde(other)]`
/// 兜底变体**不属于**该面：它的 wire 拼写是占位串、不可还原原文，注册侧明确拒绝注册它
/// （见 `ACP_CONTROL_SUBTYPES` 的文档注释）。
fn enum_face_scan(source: &str, enum_name: &str) -> (Vec<String>, Arms, BTreeSet<String>) {
    let mut names = Vec::new();
    let mut fallback = BTreeSet::new();
    let mut pending_other = false;
    for line in block_body(source, &format!("pub enum {enum_name} {{")) {
        let t = line.trim();
        if t.is_empty() || is_comment_line(t) {
            continue; // 空行 / 文档注释
        }
        if t.starts_with("#[") {
            pending_other = t.contains("serde(other)");
            continue; // 属性行本身不是变体
        }
        let name = t.trim_end_matches(',').trim();
        if is_word(name) {
            names.push(name.to_owned());
            if pending_other {
                fallback.insert(name.to_owned());
            }
        }
        pending_other = false;
    }
    let variants: Vec<String> = names
        .into_iter()
        .filter(|n| !fallback.contains(n))
        .collect();
    let arms: Arms = scan_arms(&block_body(source, &format!("impl {enum_name} {{")))
        .into_iter()
        .filter(|(n, _)| !fallback.contains(n))
        .collect();
    (variants, arms, fallback)
}

// ---- 三个面的**实现面**（从真相来源源码扫出） ----

/// 三个面的实现面（`面标识 -> 字面量集合`）：两个枚举的 `as_str()` 拼写 +
/// `pub mod method` 里的 `pub const` 值。
fn implementation_faces() -> BTreeMap<&'static str, BTreeSet<String>> {
    let literals = |arms: Arms| arms.into_iter().map(|(_, l)| l).collect::<BTreeSet<_>>();
    let event = scan_arms(&block_body(SCHEMA_SOURCE, "impl EventKind {"));
    let control = enum_face_scan(CONTROL_SOURCE, "ControlSubtype").1;
    let method = scan_arms(&block_body(METHODS_SOURCE, "pub mod method {"));
    BTreeMap::from([
        (FACE_EVENT_KIND, literals(event)),
        (FACE_CONTROL_SUBTYPE, literals(control)),
        (FACE_ACP_METHOD, literals(method)),
    ])
}

// ---- 归一化同名关系 ----

/// 归一化后**同时**出现在两个及以上面里的 `(归一化名, 面标识)` 对。
fn multi_face_pairs(
    faces: &BTreeMap<&'static str, BTreeSet<String>>,
) -> BTreeSet<(String, String)> {
    let mut owners: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for (face, names) in faces {
        for name in names {
            owners.entry(normalize(name)).or_default().insert(*face);
        }
    }
    owners
        .into_iter()
        .filter(|(_, fs)| fs.len() >= 2) // 至少出现在两个面上
        .flat_map(|(name, fs)| {
            fs.into_iter()
                .map(move |f| (name.clone(), f.to_owned()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 登记面里的绑定关系（`(归一化名, 面标识)` 集合）。
fn registered_alias_pairs() -> BTreeSet<(String, String)> {
    ACP_NAMESPACE_NORMALIZED_ALIASES
        .iter()
        .map(|(name, face)| ((*name).to_owned(), (*face).to_owned()))
        .collect()
}

/// **判定函数**：返回**未被登记**的 `(归一化名, 面标识)` 对（空 = 已如实登记）。
/// ⚠️ 真实断言（③）与「判定能咬人」的自证用例**走的是同一个函数**。
fn unregistered_pairs(
    faces: &BTreeMap<&'static str, BTreeSet<String>>,
    registered: &BTreeSet<(String, String)>,
) -> Vec<(String, String)> {
    multi_face_pairs(faces)
        .into_iter()
        .filter(|(name, face)| !registered.contains(&(name.clone(), face.clone())))
        .collect()
}

/// **枚举面**的扫描器自证：面**非空** + 臂与声明的变体**一一对应** + 每对臂的「派生名 ==
/// 归一化后的字面量」。最后一条是**最强的自证**：①扫描器把**同一行**的名字与值读对了、
/// ②字面量拼写与 serde 的 `snake_case` 派生**逐项吻合**、③归一化口径没被扩。
fn assert_enum_face_scanned_whole(label: &str, variants: &[String], arms: &Arms) {
    assert!(
        !variants.is_empty() && !arms.is_empty(),
        "{label} 面一个也没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    let declared: BTreeSet<&str> = variants.iter().map(String::as_str).collect();
    let scanned: BTreeSet<&str> = arms.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        scanned, declared,
        "{label} 面扫到的臂与声明的变体对不上（扫到的 {scanned:?} vs 声明的 {declared:?}）——扫描口径坏了，断言会假绿"
    );
    for (name, literal) in arms {
        assert_eq!(
            snake_case(name),
            normalize(literal),
            "{label} 面的 {name} 与其字面量 {literal} 对不上（派生名 {} vs 归一化名 {}）——扫描器把名字与值读串了，或字面量的拼写漂了",
            snake_case(name),
            normalize(literal)
        );
    }
}

// ---- 断言 ----

/// **① 三个面各自的「实现面 ⊆ 登记面」**。红了说明实现新增了 kind / subtype / method 而
/// 契约没登记 ⇒ 先补登记，再合并。
#[test]
fn each_implementation_face_is_a_subset_of_its_registered_surface() {
    let faces = implementation_faces();
    for (label, face, registered) in [
        (FACE_EVENT_KIND, &faces[FACE_EVENT_KIND], EVENT_KINDS),
        (
            FACE_CONTROL_SUBTYPE,
            &faces[FACE_CONTROL_SUBTYPE],
            ACP_CONTROL_SUBTYPES,
        ),
        (FACE_ACP_METHOD, &faces[FACE_ACP_METHOD], ACP_METHODS),
    ] {
        let outside: Vec<&str> = face
            .iter()
            .map(String::as_str)
            .filter(|name| !registered.contains(name))
            .collect();
        assert!(
            outside.is_empty(),
            "{label} 面出现了未登记的名字 {outside:?}——未登记 ⇒ 该名字在对应的派发机上要么被**拒收**（事件流侧 `UnknownKind`、ACP 侧落兜底臂抛 `MalformedMessage`），要么在对端是**方法不存在**"
        );
    }
}

/// **② 扫描器自证**：扫到的必须是**整个面**，不是扫了个空集（否则后面全绿是假的）。① 每个
/// 面都扫到了东西；② 臂与**枚举变体**一一对应；③ 每一对臂的「派生名 == 归一化字面量」。
/// wire method 面没有「变体」可对，改守「扫到的字面量**覆盖每一个已登记方法**」——扫空
/// 或漏扫一个都会红。
#[test]
fn the_scanners_actually_see_each_whole_face() {
    // ① 事件流 kind 面
    let (ev_variants, ev_arms, _) = enum_face_scan(SCHEMA_SOURCE, "EventKind");
    assert_enum_face_scanned_whole("事件流 kind", &ev_variants, &ev_arms);

    // ② ACP control subtype 面（`#[serde(other)]` 兜底变体不参与此面）
    let (cs_variants, cs_arms, cs_fallback) = enum_face_scan(CONTROL_SOURCE, "ControlSubtype");
    assert!(
        !cs_fallback.is_empty(),
        "连 `#[serde(other)]` 兜底变体都没扫到——扫描口径坏了，断言会假绿"
    );
    assert_enum_face_scanned_whole("ACP control subtype", &cs_variants, &cs_arms);

    // ③ ACP wire method 面：扫到的字面量必须覆盖每一个已登记方法。
    let method_lits: BTreeSet<String> = scan_arms(&block_body(METHODS_SOURCE, "pub mod method {"))
        .into_iter()
        .map(|(_, v)| v)
        .collect();
    assert!(
        !method_lits.is_empty(),
        "ACP wire method 面一个也没扫到——扫描口径坏了，断言会假绿"
    );
    for method in ACP_METHODS {
        assert!(
            method_lits.contains(*method),
            "登记的 ACP 方法 {method} 在 methods.rs 的 `pub mod method` 里扫不到——扫描口径坏了"
        );
    }
}

/// **③ 「同名不同面」关系被如实记录**（**双向相等**）。现算出的「多面同名」绑定关系必须
/// **恰好**等于登记面里的绑定关系：漏登记 ⇒ 红；多登记 / 登记过时 ⇒ 红。不锁名单长度：新增
/// 同名时登记新的一项即可。
#[test]
fn computed_multi_face_names_equal_the_registered_alias_table() {
    let faces = implementation_faces();
    let computed = multi_face_pairs(&faces);
    // **防空扫描**：三面若哪天变得互不相交，本断言会**空转通过**（两侧都空 ⇒
    // 「相等」成立）⇒ 单独守一条「多面同名确实存在」。
    assert!(
        !computed.is_empty(),
        "三面归一化后一个同名都没有——要么实现变了，要么扫描口径坏了，本断言会空转通过（假的绿）"
    );
    let registered = registered_alias_pairs();
    let missing = unregistered_pairs(&faces, &registered);
    let stale: Vec<&(String, String)> = registered.difference(&computed).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "归一化后的多面同名关系与登记面不一致：现算有而登记缺 {missing:?}；登记有而现算无 {stale:?}——「同名不同面」是**静默串台**的来源（control 面帧被当事件流 kind 处理），必须逐面登记"
    );
}

/// **④ 三面两两的「同名不同面」关系断言**（本卡的核心护栏）。对每一对面：归一化后**同名**
/// 的，必须在绑定表里**两个面都**登记了。⛔ **不**断言「不同名就不许共存」——下划线的
/// `session_update` 与斜杠的 `session/update` 是**同层不同义**。
/// 三面两两的面对见下。
#[test]
fn every_pairwise_same_name_is_bound_to_both_faces() {
    let faces = implementation_faces();
    let registered = registered_alias_pairs();
    for (a, b) in [
        (FACE_EVENT_KIND, FACE_CONTROL_SUBTYPE),
        (FACE_EVENT_KIND, FACE_ACP_METHOD),
        (FACE_CONTROL_SUBTYPE, FACE_ACP_METHOD),
    ] {
        let a_norm: BTreeSet<String> = faces[a].iter().map(|n| normalize(n)).collect();
        let b_norm: BTreeSet<String> = faces[b].iter().map(|n| normalize(n)).collect();
        let same: Vec<String> = a_norm.intersection(&b_norm).cloned().collect();
        for name in same {
            for face in [a, b] {
                assert!(
                    registered.contains(&(name.clone(), face.to_owned())),
                    "归一化名 {name} 同时出现在 {a} 面与 {b} 面，却没在 {face} 面的绑定关系里登记——这正是「同名不同面」被当成同一个名字的入口，先登记再合并"
                );
            }
        }
    }
}

/// **⑤ 「归一化」口径自身被钉住**：**只**做斜杠→下划线这一件事。口径若悄悄扩了 ⇒ 同名集合
/// 会**静默变大** ⇒ ③ 与 ④ 守的就不是同一个关系了。
#[test]
fn normalization_only_replaces_slashes() {
    assert_eq!(normalize("session/update"), "session_update");
    assert_eq!(normalize("session_update"), "session_update");
    assert_eq!(normalize("a/b/c"), "a_b_c");
    assert_eq!(normalize("session/update/"), "session_update_");
    // 以下五处**不动**：口径只许换斜杠。
    assert_eq!(normalize("session.update"), "session.update");
    assert_eq!(normalize("Session_Update"), "Session_Update");
    assert_eq!(normalize("session:update"), "session:update");
    assert_eq!(normalize("session update"), "session update");
    assert_eq!(normalize(""), "");
}

/// **判定真能咬人**（合成样本）：构造一个**假的**三面字面量集合，其中出现一个**新的**归一化
/// 名（`brand_new_kind`），判定函数**必须**把它报出来。⛔ 不改生产代码、不放宽断言口径——
/// 只把**假字面量集合**喂给**断言本身用的那个判定函数**。若这里报不出来 ⇒ 护栏只是**永远
/// 绿**的摆设。
#[test]
fn the_judge_detects_a_newly_appearing_same_name() {
    let fake = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
    let mut faces: BTreeMap<&'static str, BTreeSet<String>> = BTreeMap::new();
    faces.insert(FACE_EVENT_KIND, fake(&["session_update", "brand_new_kind"]));
    faces.insert(FACE_CONTROL_SUBTYPE, fake(&["session_update"]));
    faces.insert(FACE_ACP_METHOD, fake(&["session/update", "brand_new/kind"]));
    let gaps = unregistered_pairs(&faces, &registered_alias_pairs());
    assert!(
        !gaps.is_empty(),
        "新出现的归一化同名（brand_new_kind）没有被报出来——判定逻辑没生效的话，护栏只是**永远绿**的摆设"
    );
    for face in [FACE_EVENT_KIND, FACE_ACP_METHOD] {
        assert!(
            gaps.contains(&(String::from("brand_new_kind"), face.to_owned())),
            "报出的差异里没有 {face} 面的 brand_new_kind——判定逻辑坏了：{gaps:?}"
        );
    }
}
