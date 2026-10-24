//! 凭据过滤族常量「不缩水」文本扫描守卫（W2 · lane acp）。
//!
//! `agent-acp/src/stdio_env.rs` 是 spawn 前子进程环境的 **scrub 规则**
//! （fail-closed 安全基线），其中两组常量决定哪些变量名属凭据族、一律不得外传：
//! - `CREDENTIAL_PATTERNS`：大写**包含**匹配（`KEY`/`TOKEN`/… 共 8 项）；
//! - `CREDENTIAL_PREFIXES`：大写**前缀**匹配（`AWS_`/`AZURE_`/… 共 5 项）；
//! - `SYSTEM_ENV_KEEP`：`pub` 白名单，allowlist 模式的唯一保留集。
//!
//! 判定方向是 fail-closed：`前缀命中 || 包含命中` ⇒ 拒传。所以把 `"AZURE_"`
//! 从前缀表里删掉，`AZURE_TENANT_ID` 之类的云凭据就被原样传进子进程，而全仓
//! 唯一的既有行为测试只钉了 6 个名，未守 `AZURE_`/`GCP_`/`ALIBABA_CLOUD_`
//! 仅前缀家族与 `COOKIE`/`CREDENTIAL`/`PASSWD`/`AUTH` 仅模式路径 ⇒
//! 删除/漂移即**静默 fail-open**。本文件把冻结基线钉死：**缺一项即红**。
//!
//! 为什么走**源码文本扫描**：`contracts-check` 的 dev-dependencies 里**没有**
//! `agent-acp`，而 `Cargo.toml` 属共享区不得改。故照同族先例
//! （`wire_tag_enum_trio.rs` 等）用 `include_str!` 在**编译期**绑定源文件——
//! 源文件被移动/改名即编译期红，天然防「文件挪走、守卫空转」。
//!
//! 本文件只守**不缩水**、**不锁总数**——往凭据族里扩成员是正当收紧，
//! 缺项才是静默外传（口径同 `method_ledger.rs` 的 `BASELINE_ACTIONS`）。

/// 冻结基线（2026-10-24，base 0870c4fa 逐字）：凭据族**包含**模式，8 项。
const FROZEN_PATTERNS: [&str; 8] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "COOKIE",
    "AUTH",
];

/// 冻结基线（同上）：凭据族**前缀**，5 项，逐字含尾部下划线。
const FROZEN_PREFIXES: [&str; 5] = ["AWS_", "AZURE_", "GCP_", "GOOGLE_", "ALIBABA_CLOUD_"];

const ENV_SOURCE: &str = include_str!("../../agent-acp/src/stdio_env.rs");

// ---- 扫描器：最小状态机，定位定义 → 越 `=` 取 `&[` → 深度配对 → 抽字面量 ----

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 定位 `const <name>` / `pub const <name>` 的**定义处**字节偏移。
///
/// needle 本身以 `const` 开头，天然排除 `NAME.iter()` 使用点与文档里的
/// `A_PATTERNS` 提及；左右两侧都不得是标识符字符，排除 `myconst` 与
/// `NAME_EXTRA` 这类同前缀常量。
fn const_def_at(src: &str, name: &str) -> usize {
    let def = format!("const {name}");
    let mut from = 0usize;
    while let Some(at) = src[from..].find(def.as_str()) {
        let abs = from + at;
        let ident_end = abs + def.len();
        let left_ok = !src[..abs].chars().next_back().is_some_and(is_ident_char);
        let right_ok = !src[ident_end..].chars().next().is_some_and(is_ident_char);
        if left_ok && right_ok {
            return abs;
        }
        from = ident_end;
    }
    panic!("源码里找不到 `const {name}` 的定义——扫描器或源文件形态变了");
}

/// 从 `&[` 起按方括号深度配对，返回数组体（不含两端括号）。
fn bracket_matched(from_amp_bracket: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in from_amp_bracket.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return &from_amp_bracket[2..i];
                }
            }
            _ => {}
        }
    }
    panic!("数组没有配对的 `]`——源文件形态变了");
}

/// 抽出块内所有 `"..."` 字面量（成员是环境变量名常量，无转义形态）。
fn string_literals_in(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find('"') {
        let Some(len) = rest[start + 1..].find('"') else {
            break;
        };
        out.push(rest[start + 1..start + 1 + len].to_owned());
        rest = &rest[start + 1 + len + 1..];
    }
    out
}

/// 从源码文本里摘出 `const <name>: &[&str] = &[ ... ];` 的字面量集合。
///
/// 定位 `name` 的定义 → 找到该处 `=` 后的 `&[` → 向后累积到配对的 `]`。
/// 多行数组、尾随逗号、`pub const`/`const`、行内单行形态均兼容；
/// 找不到定义/数组即 panic（显式红），绝不扫成空集后静默通过。
fn array_literals(src: &str, name: &str) -> Vec<String> {
    let abs = const_def_at(src, name);
    let tail = &src[abs..];
    let eq = tail
        .find('=')
        .unwrap_or_else(|| panic!("`{name}` 没有 `=` 赋值号——源文件形态变了"));
    let after_eq = &tail[eq + 1..];
    let open = after_eq
        .find("&[")
        .unwrap_or_else(|| panic!("`{name}` 的值不是 `&[` 数组——源文件形态变了"));
    string_literals_in(bracket_matched(&after_eq[open..]))
}

/// 截取 `fn <signature>` 起的函数体（花括号深度配对），用于接线检查。
fn fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
    let at = src
        .find(signature)
        .unwrap_or_else(|| panic!("源码里找不到 `{signature}`——扫描器或源文件形态变了"));
    let open_rel = src[at..]
        .find('{')
        .unwrap_or_else(|| panic!("`{signature}` 没有函数体"));
    let body_start = at + open_rel;
    let mut depth = 0usize;
    for (i, c) in src[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[body_start..=body_start + i];
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` 的函数体没有配对的 `}}`");
}

// ---- 判定逻辑：纯函数，供合成样本自测复用 ----

/// 基线里扫描集缺的项（缺项 ⇒ 该名不再被判为凭据 ⇒ 静默外传 fail-open）。
fn missing_from_baseline<'a>(scanned: &[String], baseline: &[&'a str]) -> Vec<&'a str> {
    let view: Vec<&str> = scanned.iter().map(String::as_str).collect();
    baseline
        .iter()
        .copied()
        .filter(|b| !view.contains(b))
        .collect()
}

/// 白名单与凭据族的重叠项 `(name, 命中原因)`（大小写按源码口径统一转大写）。
/// 判定顺序与 `is_credential_env` 一致：先前缀、后包含。
fn keep_conflicts(
    keep: &[String],
    prefixes: &[String],
    patterns: &[String],
) -> Vec<(String, String)> {
    keep.iter()
        .filter_map(|name| {
            let upper = name.to_ascii_uppercase();
            if let Some(p) = prefixes.iter().find(|p| upper.starts_with(p.as_str())) {
                return Some((name.clone(), format!("前缀命中 {p}")));
            }
            patterns
                .iter()
                .find(|p| upper.contains(p.as_str()))
                .map(|p| (name.clone(), format!("包含命中 {p}")))
        })
        .collect()
}

// ---- 断言 0：扫描器自检（合成样本，不动真源） ----

#[test]
fn array_literals_handles_const_forms_and_layouts() {
    // 同前缀更长的 decoy 靠词边界跳过；文档提及不是定义；`pub const`/`const`、
    // 多行/单行/尾随逗号都得扫出来。
    let src = r#"
/// 见 A_PATTERNS（文档提及不算定义）
const A_PATTERNS_EXTRA: &[&str] = &["DECOY"];
pub const A_PATTERNS: &[&str] = &[
    "X1",
    "X2",
];
const B_PREFIXES: &[&str] = &["Y1", "Y2"];
const C_KEEP: &[&str] = &["Z1", "Z2",];
"#;
    assert_eq!(
        array_literals(src, "A_PATTERNS"),
        ["X1", "X2"],
        "须命中真定义而非 A_PATTERNS_EXTRA 的 decoy"
    );
    assert_eq!(array_literals(src, "B_PREFIXES"), ["Y1", "Y2"]);
    assert_eq!(array_literals(src, "C_KEEP"), ["Z1", "Z2"]);
}

// ---- 断言 1：防空扫 ----

#[test]
fn scanned_families_are_non_empty() {
    for name in [
        "CREDENTIAL_PATTERNS",
        "CREDENTIAL_PREFIXES",
        "SYSTEM_ENV_KEEP",
    ] {
        let scanned = array_literals(ENV_SOURCE, name);
        assert!(
            !scanned.is_empty(),
            "`{name}` 扫成空集——扫描器或源文件形态变了，守卫不得空转：{scanned:?}"
        );
    }
}

// ---- 断言 2/3：凭据族不缩水（只守 ⊇，不锁总数） ----

#[test]
fn credential_patterns_superset_of_frozen_baseline() {
    let scanned = array_literals(ENV_SOURCE, "CREDENTIAL_PATTERNS");
    let missing = missing_from_baseline(&scanned, &FROZEN_PATTERNS);
    assert!(
        missing.is_empty(),
        "凭据模式缩水 ⇒ 敏感变量静默外传（fail-open）：CREDENTIAL_PATTERNS 缺 {missing:?}（现余 {scanned:?}）"
    );
}

#[test]
fn credential_prefixes_superset_of_frozen_baseline() {
    let scanned = array_literals(ENV_SOURCE, "CREDENTIAL_PREFIXES");
    let missing = missing_from_baseline(&scanned, &FROZEN_PREFIXES);
    assert!(
        missing.is_empty(),
        "凭据前缀缩水 ⇒ 云凭据静默外传（fail-open）：CREDENTIAL_PREFIXES 缺 {missing:?}（现余 {scanned:?}）"
    );
}

// ---- 断言 4：常量被真正接线（防「留着但不引用」的空转） ----

#[test]
fn credential_constants_are_wired_into_is_credential_env() {
    let body = fn_body(ENV_SOURCE, "fn is_credential_env(");
    for name in ["CREDENTIAL_PREFIXES", "CREDENTIAL_PATTERNS"] {
        let needle = format!("{name}.iter()");
        assert!(
            body.contains(&needle),
            "`{name}` 定义了但 `is_credential_env` 体内不再引用（`{needle}` 缺失）\
             ⇒ 该族整体空转，删除成员不会有任何判定变严"
        );
    }
}

// ---- 断言 5：白名单与凭据族不相交（fail-closed 一致性） ----

#[test]
fn system_env_keep_is_disjoint_from_credential_family() {
    let keep = array_literals(ENV_SOURCE, "SYSTEM_ENV_KEEP");
    let conflicts = keep_conflicts(
        &keep,
        &array_literals(ENV_SOURCE, "CREDENTIAL_PREFIXES"),
        &array_literals(ENV_SOURCE, "CREDENTIAL_PATTERNS"),
    );
    assert!(
        conflicts.is_empty(),
        "SYSTEM_ENV_KEEP 与凭据族相交 ⇒ allowlist 模式会把凭据原样保留（fail-open）：{conflicts:?}"
    );
}

// ---- 断言 6：合成样本自证扫描器真的能红（只喂副本字符串，真源只读） ----

#[test]
fn scanner_catches_prefix_shrinkage_on_synthetic_copy() {
    let shrunk = ENV_SOURCE.replace("\"AZURE_\", ", "");
    assert_ne!(shrunk, ENV_SOURCE, "合成样本没改成功——源文件形态变了");
    let scanned = array_literals(&shrunk, "CREDENTIAL_PREFIXES");
    let missing = missing_from_baseline(&scanned, &FROZEN_PREFIXES);
    assert!(
        missing.contains(&"AZURE_"),
        "摘掉 AZURE_ 后判定逻辑必须报缺：missing={missing:?}，scanned={scanned:?}"
    );
}

#[test]
fn scanner_detects_whitelist_overlap_on_synthetic_copy() {
    let poisoned = ENV_SOURCE.replace("\"PATH\",", "\"PATH\", \"PATH_TOKEN\",");
    assert_ne!(poisoned, ENV_SOURCE, "合成样本没改成功——源文件形态变了");
    let conflicts = keep_conflicts(
        &array_literals(&poisoned, "SYSTEM_ENV_KEEP"),
        &array_literals(&poisoned, "CREDENTIAL_PREFIXES"),
        &array_literals(&poisoned, "CREDENTIAL_PATTERNS"),
    );
    assert_eq!(
        conflicts.len(),
        1,
        "只应新增 PATH_TOKEN 一处重叠：{conflicts:?}"
    );
    assert_eq!(conflicts[0].0, "PATH_TOKEN");
    assert!(
        conflicts[0].1.contains("TOKEN"),
        "命中原因应指出 TOKEN 模式：{}",
        conflicts[0].1
    );
}

#[test]
fn scanner_accepts_prefix_expansion_on_synthetic_copy() {
    let grown = ENV_SOURCE.replace(
        "\"ALIBABA_CLOUD_\"",
        "\"ALIBABA_CLOUD_\", \"EXTRA_FUTURE_PREFIX_\"",
    );
    assert_ne!(grown, ENV_SOURCE, "合成样本没改成功——源文件形态变了");
    let scanned = array_literals(&grown, "CREDENTIAL_PREFIXES");
    let missing = missing_from_baseline(&scanned, &FROZEN_PREFIXES);
    assert!(
        missing.is_empty(),
        "扩容是正当收紧，不得误报：missing={missing:?}，scanned={scanned:?}"
    );
}
