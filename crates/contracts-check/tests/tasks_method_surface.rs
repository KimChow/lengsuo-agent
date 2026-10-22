//! ACP **任务族方法面**断言（W2 · lane proto）。
//!
//! `agent-acp/src/tasks.rs` 的 `tasks::method` 模块登记了 27 册 §2 的四个任务
//! 方法（`tasks/list` / `tasks/get` / `tasks/cancel` / `tasks/result`）。本文件
//! 把契约侧 [`ACP_TASKS_METHODS`] 与实现侧源码文本钉在一起。
//!
//! 为什么走**源码文本扫描**：`contracts-check` 的 dev-dependencies 里**没有**
//! `agent-acp`，而 `Cargo.toml` 属共享区（本卡**不得**改）。故与
//! `tests/method_ledger.rs` 同法，读 `CARGO_MANIFEST_DIR` 相对路径。
//!
//! ⚠️ 全部断言**只守关系、不锁数量/取值**：下一张卡正当新增 `tasks/xxx` 并登记
//! 后，本文件不得因此变红。

use std::collections::BTreeSet;
use std::path::PathBuf;

use contracts_check::{ACP_METHODS, ACP_TASKS_METHODS};

/// `../agent-acp/src/<file>`（不写绝对路径）。
fn agent_acp_src(file: &str) -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-acp")
        .join("src")
        .join(file)
}

/// 从源码文本里抽出所有 `pub const <NAME>: &str = "<value>";` 的字面量值。
///
/// 与 `tests/method_ledger.rs::scan_method_consts` 同口径：只认「常量名 +
/// `: &str` + `=` + 字符串」形态；文档注释里的反引号方法名不含 `pub const`，
/// 天然不匹配。返回按源码顺序排列，调用方用集合比对（顺序无关）。
fn scan_method_consts(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in source.split("pub const ").skip(1) {
        let Some((_, after_colon)) = chunk.split_once(':') else {
            continue;
        };
        if !matches!(
            after_colon
                .trim_start()
                .chars()
                .take(4)
                .collect::<String>()
                .as_str(),
            "&str" | "& st" // 容忍 `&str` / `& str` 两种空白
        ) {
            continue;
        }
        let Some(start) = after_colon.find('"') else {
            continue;
        };
        let rest = &after_colon[start + 1..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        out.push(rest[..end].to_owned());
    }
    out
}

/// 读文件并扫描；顺带防空（读到 0 项说明路径/形态变了，先核对再放行）。
fn scan_file(file: &str) -> Vec<String> {
    let path = agent_acp_src(file);
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));
    let scanned = scan_method_consts(&source);
    assert!(
        !scanned.is_empty(),
        "从 {file} 扫到 0 个 `pub const …: &str`；扫描器或源文件形态变了，先核对扫描器"
    );
    scanned
}

fn as_set(items: &[String]) -> BTreeSet<&str> {
    items.iter().map(String::as_str).collect()
}

fn assert_no_duplicates(name: &str, items: &[&str]) {
    let mut seen = BTreeSet::new();
    for item in items {
        assert!(seen.insert(*item), "{name} 出现重复项：{item}");
    }
}

/// ① **双向差集**：`ACP_TASKS_METHODS` == `tasks.rs` 扫出的常量集合。
#[test]
fn task_method_consts_match_acp_tasks_methods() {
    let scanned = scan_file("tasks.rs");
    let scanned = as_set(&scanned);
    let registered: BTreeSet<&str> = ACP_TASKS_METHODS.iter().copied().collect();

    let missing: Vec<&&str> = registered.difference(&scanned).collect();
    let extra: Vec<&&str> = scanned.difference(&registered).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "tasks.rs 与 ACP_TASKS_METHODS 不一致：常量有而实现缺 {missing:?}；实现有而常量缺 {extra:?}"
    );
}

/// ② **类级反裸奔守卫**（本卡核心）：`methods.rs` + `tasks.rs` 的每一个
/// `pub const …: &str` 都必须出现在 `ACP_METHODS ∪ ACP_TASKS_METHODS` 里。
///
/// 以后有人在任一文件新增方法常量却不登记，这里即红。
#[test]
fn every_method_const_is_registered() {
    let registry: BTreeSet<&str> = ACP_METHODS
        .iter()
        .chain(ACP_TASKS_METHODS.iter())
        .copied()
        .collect();

    let mut unregistered = Vec::new();
    for file in ["methods.rs", "tasks.rs"] {
        for value in scan_file(file) {
            if !registry.contains(value.as_str()) {
                unregistered.push(format!("{file}: {value}"));
            }
        }
    }
    assert!(
        unregistered.is_empty(),
        "发现未登记的方法常量（既不属 ACP_METHODS 也不属 ACP_TASKS_METHODS）：{unregistered:?}"
    );
}

/// ③ **命名空间不相交**：任务族与标准方法族不得重叠。
#[test]
fn tasks_namespace_is_disjoint_from_standard_methods() {
    let tasks: BTreeSet<&str> = ACP_TASKS_METHODS.iter().copied().collect();
    let standard: BTreeSet<&str> = ACP_METHODS.iter().copied().collect();

    let overlap: Vec<&&str> = tasks.intersection(&standard).collect();
    assert!(
        overlap.is_empty(),
        "ACP_TASKS_METHODS 与 ACP_METHODS 重叠（同一方法不得双族登记）：{overlap:?}"
    );
}

/// ④ **无重复**：各常量内部无重复项。
#[test]
fn task_and_standard_method_consts_are_unique() {
    assert_no_duplicates("ACP_TASKS_METHODS", ACP_TASKS_METHODS);
    assert_no_duplicates("ACP_METHODS", ACP_METHODS);
}

/// ⑤ **形态**：任务族每项都以 `"tasks/"` 开头（相对口径）。
#[test]
fn tasks_methods_are_in_tasks_namespace() {
    for method in ACP_TASKS_METHODS {
        assert!(
            method.starts_with("tasks/"),
            "ACP_TASKS_METHODS 成员 {method} 不以 \"tasks/\" 开头——任务族命名空间漂移"
        );
    }
}
