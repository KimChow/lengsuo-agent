//! 变量引用提取器的自证：每种形态都认得，且真实模板上不空转。

use super::guards::{assert_panic_labeled, catch_panic, DRIFT_TEMPLATE_PROBE};
use super::registration::assert_r3_template_refs_exist;
use super::{
    balanced_span, find_calls, is_identifier, lex, split_top_level, PROMPT_VARS_SOURCE,
    TEMPLATE_SOURCES,
};
use std::collections::{BTreeMap, BTreeSet};

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
pub(super) fn template_refs(sources: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
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

// ── 用例 ──

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
