//! prompt 变量的契约登记：`PromptVars` 字段 ↔ `prompt_vars()` 赋值 ↔ 模板引用。
//!
//! # 为什么要有这张登记
//!
//! `agent-prompts` 的 `PromptVars` 是模板变量的**唯一真源**，`agent-core` 的
//! `prompt_vars()` 把运行时事实填进去，`agent-prompts` 的模板再按变量名引用。三处
//! 都是**字符串层面**的耦合：加字段忘接线、模板改名失配、模板引用不存在的变量，三种
//! 漂移**都不会编译报错**，只会让某段模板静默不渲染。
//!
//! # 只守关系，不锁数量
//!
//! ⛔ 刻意**不写**「`PromptVars` 有几个字段」「模板引用了几个变量」这类锁规模的断言
//! ——它们会在正当扩充时必红（仓内既有口径见 `provider_usage_surface.rs` 头注）。
//! 本文件只守 4 条关系：
//!
//! - **R1**：`PromptVars` 的**每个字段**都在 `prompt_vars()` 里被显式赋值；
//! - **R2**：`prompt_vars()` 里**不出现** `..Default::default()` 这类批量兜底
//!   （兜底 = 漏填的隐身衣）；
//! - **R3**：模板里**引用的**变量名都在 `PromptVars` 里存在；
//! - **R4**：`prompt_vars()` 里赋的字段名都**存在**于 `PromptVars`（防拼写错 / 防
//!   残留旧名）。R1 与 R4 合起来即「变量名」关系的**双向差集为空**。
//!
//! # 怎么扫
//!
//! 源码用 `include_str!` 在**编译期**绑定（改动被扫文件即触发重编，不靠运行期读盘）；
//! 扫描只做**结构**提取，不引入正则依赖。模板侧的变量引用逐个形态认（形态与取哪一段
//! 见 [`VAR_FORMS`]）：占位符 `{{ai_name}}`、调用形态 `Segment::Var("soul")` /
//! `flag("is_windows")` / `Cond::Eq("identity_mode", "onboarding")`、切片形态
//! `if_filled(&["tone_style"], …)`、文件块形态 `file_block("SOUL.md", "soul_path",
//! "soul")`、分支键 `"soul" => Some(…)`。
//!
//! ⛔ **只提变量名，不提字面量**：普通文本里的字符串（`"onboarding"`、`"craft"`…）不是
//! 变量引用，一律不提；文档注释里的 `{{ PluginAgentPrompt }}` 也不是 ⇒ 扫描前先剥掉
//! 注释。
//!
//! ⛔ `format!` 的**转义**形态（`{{{{name}}}}`）与**格式参数**形态（`{{{{{name}}}}}`）
//! 都**不**算模板变量引用：前者是格式串里的字面花括号，后者的名字是 `format!` 的格式
//! 参数（如 `path_key`）⇒ 扫占位符时只认**恰好两对**花括号。
//!
//! # 护栏自证
//!
//! 所有断言都走同一组 `assert_r*` 函数。合成漂移样本调**同一个**函数，`catch_unwind`
//! **必须捕获到 panic**：捕获不到 = 护栏恒绿 = 本卡白做。

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};

/// 被扫描的 `PromptVars` 定义（编译期绑定）。
const PROMPT_VARS_SOURCE: &str = include_str!("../../agent-prompts/src/model.rs");
/// 被扫描的 `prompt_vars()` 接线源码（编译期绑定）。
const CONTEXT_SOURCE: &str = include_str!("../../agent-core/src/runtime/context.rs");

/// 登记被扫描的模板面源码：`(相对 `agent-prompts/src/` 的路径, 源码)`。
///
/// ⚠️ `include_str!` 不能通配 ⇒ 清单是手写的。新增模板文件**必须**同步登记，否则它的
/// 变量引用不会被扫到（`template_sources_cover_every_template_file` 兜住）。
macro_rules! template_sources {
    ($($rel:literal),* $(,)?) => {
        const TEMPLATE_SOURCES: &[(&str, &str)] = &[
            $(($rel, include_str!(concat!("../../agent-prompts/src/", $rel, ".rs"))),)*
        ];
    };
}
template_sources! {
    "templates/mod",
    "templates/ask",
    "templates/craft",
    "templates/expert",
    "templates/identity",
    "templates/plan",
    "templates/quick",
    "templates/common/mod",
    "templates/common/delivery",
    "templates/common/modes",
    "templates/common/policy",
    "templates/common/product",
    "templates/common/tooling",
    "compose",
}

// ── 源码扫描基元 ──

/// 归一化空白：跨行写法与单行写法等价（不引入正则依赖）。
fn normalized(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 是不是变量名形态：首字符非数字，其余为字母 / 数字 / 下划线。
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 剥掉注释、把字符串与字符字面量的**内容**涂成空格，并收集字符串字面量清单。
///
/// 返回 `(处理后的源码, 字符串字面量清单)`。处理后的源码与原文**等长**（被涂的部分换成
/// 等长空格），故字节偏移在两者间通用。字符串**内容**另存在清单里：模板占位符
/// `{{var}}` 就住在字符串里。
fn lex(source: &str) -> (String, Vec<(usize, String)>) {
    let bytes = source.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut lits: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // 行注释：涂成空格（换行符保留）。
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            // 块注释：Rust 允许嵌套，按深度配平。
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < bytes.len() {
                    if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        depth += 1;
                        out.push(b' ');
                        out.push(b' ');
                        i += 2;
                    } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        depth -= 1;
                        out.push(b' ');
                        out.push(b' ');
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        out.push(if bytes[i] == b'\n' { b'\n' } else { b' ' });
                        i += 1;
                    }
                }
            }
            // 字符串字面量：内容涂成空格（占位符另存进清单），引号保留。
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                let end = j.min(bytes.len());
                lits.push((start, source[start..end].to_string()));
                out.push(b'"');
                out.resize(out.len() + (end - start), b' ');
                if end < bytes.len() {
                    out.push(b'"');
                }
                i = end + 1;
            }
            // 字符字面量（`'x'` / `'\n'`）：整体涂掉；生命周期（`'static`）只涂引号。
            b'\'' => {
                let is_char = bytes.get(i + 1) == Some(&b'\\') || bytes.get(i + 2) == Some(&b'\'');
                if is_char {
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j] != b'\'' {
                        j += if bytes[j] == b'\\' { 2 } else { 1 };
                    }
                    let end = j.min(bytes.len());
                    out.resize(out.len() + (end - i + 1), b' ');
                    i = end + 1;
                } else {
                    out.push(b'\'');
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    (String::from_utf8_lossy(&out).into_owned(), lits)
}

/// 从 `open`（指向 `(` / `[` / `{`）起，返回配对闭括号**之内**的字节区间。
///
/// 字符串字面量的**内容**已被 `lex` 涂成空格 ⇒ 其里的括号不参与配平。
fn balanced_span(source: &str, open: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 把 `source[start..end)` 按**顶层**逗号切成若干段（返回每段的字节区间）。
fn split_top_level(source: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let bytes = source.as_bytes();
    let mut args = Vec::new();
    let mut depth = 0i32;
    let mut seg_start = start;
    let mut i = start;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                args.push((seg_start, i));
                seg_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if seg_start < end {
        args.push((seg_start, end));
    }
    args
}

/// 找出所有 `needle` 的调用点（`needle` 以 `(` 结尾 ⇒ 返回该 `(` 的位置）。
///
/// 前一个字符若是标识符字符则跳过（避免 `my_flag(` 命中 `flag(`）。
fn find_calls(code: &str, needle: &str) -> Vec<usize> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = code[from..].find(needle) {
        let at = from + rel;
        let prev = if at > 0 { bytes[at - 1] } else { b' ' };
        if !(prev.is_ascii_alphanumeric() || prev == b'_') {
            out.push(at + needle.len() - 1);
        }
        from = at + needle.len();
    }
    out
}

// ── `PromptVars` 字段面 ──

/// `PromptVars` 的字段名集合（从结构体定义抽；**不锁字段数量**）。
fn prompt_vars_fields(model_src: &str) -> BTreeSet<String> {
    let (code, _) = lex(model_src);
    let start = code
        .find("pub struct PromptVars {")
        .expect("model.rs 必须定义 `pub struct PromptVars`");
    let open = start + "pub struct PromptVars {".len() - 1;
    let (body_start, body_end) = balanced_span(&code, open).expect("PromptVars 结构体必须闭合");
    let mut fields = BTreeSet::new();
    for (a, b) in split_top_level(&code, body_start, body_end) {
        let item = code[a..b].trim();
        let Some(rest) = item.strip_prefix("pub ") else {
            continue;
        };
        let name = rest.split(':').next().unwrap_or("").trim();
        if is_identifier(name) {
            fields.insert(name.to_string());
        }
    }
    fields
}

/// `prompt_vars()` 里 `PromptVars { … }` 结构体字面量的**内容**区间。
fn prompt_vars_literal(code: &str) -> Option<(usize, usize)> {
    let fn_at = code.find("fn prompt_vars(")?;
    let params_open = fn_at + "fn prompt_vars".len();
    let (_, params_close) = balanced_span(code, params_open)?;
    let body_open = code[params_close..].find('{')? + params_close;
    let (body_start, body_end) = balanced_span(code, body_open)?;
    let lit = code[body_start..body_end].find("PromptVars {")? + body_start;
    balanced_span(code, lit + "PromptVars {".len() - 1)
}

/// `prompt_vars()` 里被**显式赋值**的字段名集合。
fn assigned_fields(context_src: &str) -> BTreeSet<String> {
    let (code, _) = lex(context_src);
    let (start, end) = prompt_vars_literal(&code)
        .expect("context.rs 必须保留 prompt_vars() 的 `PromptVars { … }`");
    let mut fields = BTreeSet::new();
    for (a, b) in split_top_level(&code, start, end) {
        if let Some(name) = field_name(&code[a..b]) {
            fields.insert(name.to_string());
        }
    }
    fields
}

/// 取一段赋值项的**字段名**（`标识符` + `:`）；不是字段形态则返回 `None`。
fn field_name(item: &str) -> Option<&str> {
    let item = item.trim();
    let end = item.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    let name = &item[..end];
    let rest = item[end..].trim_start();
    if !rest.starts_with(':') || rest.starts_with("::") || name.is_empty() {
        return None;
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(name)
}

// ── 模板面变量引用 ──

/// 变量引用形态：`(调用前缀, 第几个参数, 该参数是不是字符串切片)`。
///
/// 只认**确实接收变量名**的调用点；普通文本里的字面量不算变量引用。
const VAR_FORMS: &[(&str, usize, bool)] = &[
    ("Segment::Var(", 0, false),
    ("Cond::True(", 0, false),
    ("Cond::Eq(", 0, false),
    ("Cond::Contains(", 0, false),
    ("flag(", 0, false),
    ("contains(", 0, false),
    ("if_filled(", 0, true),
    ("file_block(", 1, false),
    ("file_block(", 2, false),
];

/// 模板面引用的变量名 → 出现该引用的文件。
fn template_refs(sources: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (label, source) in sources {
        let (code, lits) = lex(source);
        let mut found: BTreeSet<String> = BTreeSet::new();
        collect_placeholder_refs(&lits, &mut found);
        collect_form_refs(&code, &lits, &mut found);
        collect_match_arm_refs(&code, &lits, &mut found);
        for name in found {
            out.entry(name).or_default().insert((*label).to_string());
        }
    }
    out
}

/// 提取 `{{name}}` 形态的变量引用（只在**字符串字面量**里找）。
///
/// ⛔ 只认**恰好两对花括号**的形态：`format!` 的转义写法（`{{{{name}}}}`）与格式参数
/// 写法（`{{{{{name}}}}}`）整体跳过——后者的名字是 `format!` 的格式参数（如
/// `path_key`），不是模板变量名。
fn collect_placeholder_refs(lits: &[(usize, String)], out: &mut BTreeSet<String>) {
    for (_, content) in lits {
        scan_placeholders(content, out);
    }
}

/// 在一段文本里扫 `{{name}}` 形态的变量引用。
fn scan_placeholders(content: &str, out: &mut BTreeSet<String>) {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'{' || bytes[i + 1] != b'{' {
            i += 1;
            continue;
        }
        // 名字只允许标识符字符；开括号多于两对 ⇒ 转义 / 格式参数形态，跳过。
        let mut name_start = i + 2;
        while name_start < bytes.len() && bytes[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let mut j = name_start;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        let mut k = j;
        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        let closes = bytes[k..].iter().take_while(|b| **b == b'}').count();
        let name = &content[name_start..j];
        if closes == 2 && is_identifier(name) {
            out.insert(name.to_string());
            i = k + 2;
        } else {
            i = j.max(i + 2);
        }
    }
}

/// 提取**调用形态**的变量引用（`Segment::Var("x")` / `flag("x")` / …）。
fn collect_form_refs(code: &str, lits: &[(usize, String)], out: &mut BTreeSet<String>) {
    for (prefix, arg_index, is_slice) in VAR_FORMS {
        for open in find_calls(code, prefix) {
            let Some((start, end)) = balanced_span(code, open) else {
                continue;
            };
            let args = split_top_level(code, start, end);
            let Some(&(a, b)) = args.get(*arg_index) else {
                continue;
            };
            let found: Vec<&String> = lits
                .iter()
                .filter(|(pos, _)| *pos > a && *pos < b)
                .map(|(_, content)| content)
                .collect();
            let picked: Vec<&String> = if *is_slice {
                found
            } else {
                found.into_iter().take(1).collect()
            };
            for name in picked {
                if is_identifier(name) {
                    out.insert(name.clone());
                }
            }
        }
    }
}

/// 提取 `match` 分支键形态的变量引用（`"x" => …`）。
fn collect_match_arm_refs(code: &str, lits: &[(usize, String)], out: &mut BTreeSet<String>) {
    for (pos, content) in lits {
        // 字面量紧跟 `=>` ⇒ 这是 `match` 分支的键（变量名）。
        let Some(tail) = code[pos + content.len()..].strip_prefix('"') else {
            continue;
        };
        if tail.trim_start().starts_with("=>") && is_identifier(content) {
            out.insert(content.clone());
        }
    }
}

// ── 断言出口 ──

/// **R1**：`PromptVars` 的**每个字段**都在 `prompt_vars()` 里被显式赋值。
fn assert_r1_fields_assigned(model_src: &str, context_src: &str) {
    let fields = prompt_vars_fields(model_src);
    assert!(
        !fields.is_empty(),
        "必须能从 model.rs 抽出 PromptVars 的字段清单"
    );
    let assigned = assigned_fields(context_src);
    let missing: Vec<&String> = fields.difference(&assigned).collect();
    assert!(
        missing.is_empty(),
        "PromptVars 字段未在 prompt_vars() 里显式赋值：{missing:?}（漏赋值 ⇒ 走默认值 ⇒ 模板段静默不渲染）"
    );
}

/// **R2**：`prompt_vars()` 里没有 `..Default::default()` / `..PromptVars::default()`
/// 这类**批量兜底**（兜底会把漏填的字段藏起来）。
fn assert_r2_no_batch_default(context_src: &str) {
    let (code, _) = lex(context_src);
    let (start, end) = prompt_vars_literal(&code)
        .expect("context.rs 必须保留 prompt_vars() 的 `PromptVars { … }`");
    for (a, b) in split_top_level(&code, start, end) {
        let item = code[a..b].trim();
        assert!(
            !item.starts_with(".."),
            "prompt_vars() 不得用 `{item}` 批量兜底：兜底会把漏填的字段藏起来"
        );
    }
    // 去掉全部空白后再比对 ⇒ 跨行写法与单行写法等价。
    let squashed: String = code[start..end].split_whitespace().collect();
    assert!(
        !squashed.contains("..Default::default()") && !squashed.contains("..PromptVars::default()"),
        "prompt_vars() 不得用 `..Default::default()` / `..PromptVars::default()` 批量兜底"
    );
}

/// **R3**：模板引用的变量名都存在于 `PromptVars`。
fn assert_r3_template_refs_exist(model_src: &str, sources: &[(&str, &str)]) {
    let fields = prompt_vars_fields(model_src);
    let refs = template_refs(sources);
    let unknown: Vec<(&String, &BTreeSet<String>)> = refs
        .iter()
        .filter(|(name, _)| !fields.contains(*name))
        .collect();
    assert!(
        unknown.is_empty(),
        "模板引用了 PromptVars 里没有的变量：{unknown:?}（未知键 ⇒ 该模板段静默不渲染）"
    );
}

/// **R4**：`prompt_vars()` 里赋的字段名都存在于 `PromptVars`（防拼写错 / 残留旧名）。
fn assert_r4_assigned_fields_known(model_src: &str, context_src: &str) {
    let fields = prompt_vars_fields(model_src);
    let assigned = assigned_fields(context_src);
    let unknown: Vec<&String> = assigned.difference(&fields).collect();
    assert!(
        unknown.is_empty(),
        "prompt_vars() 赋了 PromptVars 里不存在的字段：{unknown:?}（改名 / 拼写错 ⇒ 编译期不报错）"
    );
}

// ── 合成漂移样本（短：不复制整份真实源码）──

/// 合成结构体：两个字段。
const DRIFT_MODEL: &str = "pub struct PromptVars { pub alpha: String, pub beta: String }";

/// 合成接线：少赋一个字段（`beta` 没赋值）。
const DRIFT_CONTEXT_MISSING: &str =
    "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: String::new() } }";

/// 合成接线：用 `..Default::default()` 批量兜底。
const DRIFT_CONTEXT_DEFAULT: &str =
    "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: String::new(), ..Default::default() } }";

/// 合成接线：赋了一个 `PromptVars` 里没有的字段。
const DRIFT_CONTEXT_UNKNOWN: &str = "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: \
     String::new(), beta: String::new(), gamma: String::new() } }";

/// 合成模板：引用了一个不存在的变量。
const DRIFT_TEMPLATE: &str = "text(\"{{no_such_variable}}\")";

/// 合成模板：探针形态（占位符 + 调用形态各一个）。
const DRIFT_TEMPLATE_PROBE: &str = "text(\"{{probe_text}}\")\nflag(\"probe_flag\")";

/// 跑一个闭包并把 panic 捕获下来（静默 panic hook，避免污染测试输出）。
fn catch_panic<F: FnOnce()>(f: F) -> std::thread::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous);
    outcome
}

/// 取出 panic payload 里的文本。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// 断言「护栏确实红了」：必须捕获到 panic，且 panic 文案带上标签。
fn assert_panic_labeled(outcome: std::thread::Result<()>, needle: &str) {
    let payload = outcome.expect_err("护栏必须能红：漂移样本竟未触发 panic");
    let message = panic_message(payload.as_ref());
    assert!(
        message.contains(needle),
        "panic 文案应包含 `{needle}`，实际：{message}"
    );
}

// ── 用例 ──

/// ① R1：`PromptVars` 每个字段都在 `prompt_vars()` 里被显式赋值（**不锁字段总数**）。
#[test]
fn prompt_vars_fields_are_all_explicitly_assigned() {
    assert!(normalized(PROMPT_VARS_SOURCE).contains("pub struct PromptVars {"));
    assert_r1_fields_assigned(PROMPT_VARS_SOURCE, CONTEXT_SOURCE);
}

/// ② R2：`prompt_vars()` 里没有 `..Default::default()` / `..PromptVars::default()` 兜底。
#[test]
fn prompt_vars_has_no_batch_default_fallback() {
    assert_r2_no_batch_default(CONTEXT_SOURCE);
}

/// ③ R3：模板引用的变量名都存在于 `PromptVars`。
#[test]
fn template_variable_refs_all_exist_in_prompt_vars() {
    assert_r3_template_refs_exist(PROMPT_VARS_SOURCE, TEMPLATE_SOURCES);
}

/// ④ R4：`prompt_vars()` 里赋的字段名都存在于 `PromptVars`。
#[test]
fn assigned_field_names_all_exist_in_prompt_vars() {
    assert_r4_assigned_fields_known(PROMPT_VARS_SOURCE, CONTEXT_SOURCE);
}

/// ⑤ 护栏自证：合成漂移样本（删掉一个字段赋值）⇒ R1 **必须**红。
#[test]
fn guard_detects_missing_field_assignment() {
    let outcome = catch_panic(|| assert_r1_fields_assigned(DRIFT_MODEL, DRIFT_CONTEXT_MISSING));
    assert_panic_labeled(outcome, "未在 prompt_vars() 里显式赋值");
}

/// ⑥ 护栏自证：合成漂移样本（模板引用不存在的变量）⇒ R3 **必须**红。
#[test]
fn guard_detects_unknown_template_variable() {
    let outcome =
        catch_panic(|| assert_r3_template_refs_exist(DRIFT_MODEL, &[("drift.rs", DRIFT_TEMPLATE)]));
    assert_panic_labeled(outcome, "PromptVars 里没有的变量");
}

/// ⑦ 护栏自证：合成漂移样本（批量兜底）⇒ R2 **必须**红。
#[test]
fn guard_detects_batch_default_fallback() {
    let outcome = catch_panic(|| assert_r2_no_batch_default(DRIFT_CONTEXT_DEFAULT));
    assert_panic_labeled(outcome, "批量兜底");
}

/// ⑧ 护栏自证：合成漂移样本（赋了不存在的字段）⇒ R4 **必须**红。
#[test]
fn guard_detects_unknown_assigned_field() {
    let outcome =
        catch_panic(|| assert_r4_assigned_fields_known(DRIFT_MODEL, DRIFT_CONTEXT_UNKNOWN));
    assert_panic_labeled(outcome, "不存在的字段");
}

/// ⑨ 提取器自证：占位符形态与调用形态各必须命中一个（形态写坏 ⇒ 必红）。
#[test]
fn template_ref_extractor_recognizes_every_form() {
    let refs = template_refs(&[("probe.rs", DRIFT_TEMPLATE_PROBE)]);
    assert!(
        refs.contains_key("probe_text"),
        "占位符形态 `{{var}}` 失效：{refs:?}"
    );
    assert!(
        refs.contains_key("probe_flag"),
        "调用形态 `flag(\"var\")` 失效：{refs:?}"
    );
}

/// ⑩ 提取器不得在真实模板上空转（空转 ⇒ R3 恒真）。
///
/// 双重自证：真实模板必须提出非空引用面；往**真实模板源**里注入一个不存在的变量引用
/// 后，R3 必须红（证明真实文件确实被扫到，而非只扫合成样本）。
#[test]
fn template_ref_extractor_does_not_spin_on_real_templates() {
    let refs = template_refs(TEMPLATE_SOURCES);
    assert!(
        !refs.is_empty(),
        "模板变量提取器在真实模板上不得空转（否则 R3 恒真）"
    );
    let (label, source) = TEMPLATE_SOURCES[0];
    let injected = format!("{source}\ntext(\"{{{{injected_unknown_var}}}}\")\n");
    let outcome = catch_panic(|| {
        assert_r3_template_refs_exist(PROMPT_VARS_SOURCE, &[(label, injected.as_str())])
    });
    assert_panic_labeled(outcome, "injected_unknown_var");
}

/// ⑪ 模板文件清单必须覆盖 `templates/**` 下的全部 `.rs`（防「新增模板漏登记」）。
#[test]
fn template_sources_cover_every_template_file() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../agent-prompts/src");
    let mut found: Vec<String> = Vec::new();
    collect_rs_files(&root.join("templates"), "templates/", &mut found);
    found.sort();
    assert!(!found.is_empty(), "templates/ 目录必须存在");
    for path in &found {
        let rel = path.strip_suffix(".rs").unwrap_or(path);
        assert!(
            TEMPLATE_SOURCES.iter().any(|(label, _)| *label == rel),
            "模板文件 {path} 未登记进 TEMPLATE_SOURCES：新增模板必须登记，否则它的变量引用不会被扫到"
        );
    }
}

/// 递归收集目录下全部 `.rs` 文件的相对路径。
fn collect_rs_files(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            collect_rs_files(&path, &format!("{prefix}{name}/"), out);
        } else if name.ends_with(".rs") {
            out.push(format!("{prefix}{name}"));
        }
    }
}
