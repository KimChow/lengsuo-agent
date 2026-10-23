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
//!
//! # 本目录分组说明
//!
//! - `registration.rs`：字段面 / 模板引用 / 赋值接线的 R1–R4 断言与对应用例；
//! - `guards.rs`：护栏自证（合成漂移样本 + panic 捕获）；
//! - `extractors.rs`：变量引用提取器自身的自证。
//!
//! 共享的源码扫描基元（`lex` / `balanced_span` / `split_top_level` / `find_calls`）
//! 与 `include_str!` 常量留在本文件（target 根），子模块用 `use super::…` 取用。

/// 被扫描的 `PromptVars` 定义（编译期绑定）。
const PROMPT_VARS_SOURCE: &str = include_str!("../../../agent-prompts/src/model.rs");
/// 被扫描的 `prompt_vars()` 接线源码（编译期绑定）。
const CONTEXT_SOURCE: &str = include_str!("../../../agent-core/src/runtime/context.rs");

/// 登记被扫描的模板面源码：`(相对 `agent-prompts/src/` 的路径, 源码)`。
///
/// ⚠️ `include_str!` 不能通配 ⇒ 清单是手写的。新增模板文件**必须**同步登记，否则它的
/// 变量引用不会被扫到（`template_sources_cover_every_template_file` 兜住）。
macro_rules! template_sources {
    ($($rel:literal),* $(,)?) => {
        const TEMPLATE_SOURCES: &[(&str, &str)] = &[
            $(($rel, include_str!(concat!("../../../agent-prompts/src/", $rel, ".rs"))),)*
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

mod extractors;
mod guards;
mod registration;
