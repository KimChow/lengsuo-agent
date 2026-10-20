//! ACP 方法面台账断言（W2 · lane proto）。
//!
//! 把 `docs/contracts/acp-method-ledger.md` 的清单变成可执行断言：
//! - **动作面** `ACTIONS` 恰 11 项、无重复（与 `action_surface.rs` 同口径）；
//! - **方法面** `ACP_METHODS` 恰 13 项、无重复；
//! - **白名单 ⊆ 动作面**：快照表里每个名字都必须是 v0 冻结动作的**原名**（防自创名）——
//!   这条是**关系不变量**，**不**断言白名单成员数量（扩表时不得必红）；
//! - **反向一致性**：`agent-acp::method` 模块的 `pub const` 字面量集合与
//!   `ACP_METHODS` **相等**（防一侧加方法、另一侧漏登记）。
//!
//! 反向一致性为什么走**源码文本扫描**：`contracts-check` 的 dev-dependencies 里
//! **没有** `agent-acp`，而 `Cargo.toml` 属共享区（本卡**不得**改）。故直接读
//! `../agent-acp/src/methods.rs` 的源码文本、用正则抽出方法名，再与常量比对。
//! 路径用 `env!("CARGO_MANIFEST_DIR")` 拼接，**不写绝对路径**。

use contracts_check::{ACP_FORWARD_WHITELIST_SNAPSHOT, ACP_METHODS, ACTIONS};

/// 无重复：任一名字出现两次即红（防复制粘贴把 13 项凑成 14 项却看着一样）。
fn assert_unique(name: &str, items: &[&str]) {
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        assert!(seen.insert(*item), "{name} 出现重复项：{item}");
    }
}

#[test]
fn action_surface_is_exactly_11_unique() {
    assert_eq!(ACTIONS.len(), 11, "v0 动作面应为 11 个");
    assert_unique("ACTIONS", ACTIONS);
}

#[test]
fn acp_method_surface_is_exactly_13_unique() {
    assert_eq!(ACP_METHODS.len(), 13, "ACP 标准方法应为 13 个");
    assert_unique("ACP_METHODS", ACP_METHODS);
}

/// **关系不变量**（本卡的核心守点）：白名单快照的每一项都必须属于 `ACTIONS`。
///
/// 刻意**不**写 `assert_eq!(ACP_FORWARD_WHITELIST_SNAPSHOT.len(), 1)` 之类锁死
/// 当前决策状态的断言——下一张卡正当扩充白名单时会让它必红。这里守的是
/// 「转发只搬运既有动作、不自创动作名」这条**关系**。
#[test]
fn whitelist_snapshot_is_subset_of_actions() {
    for action in ACP_FORWARD_WHITELIST_SNAPSHOT {
        assert!(
            ACTIONS.contains(action),
            "白名单成员 {action} 不在 ACTIONS 里——转发不得自创动作名"
        );
    }
}

/// 佐证 `docs/contracts/acp-method-ledger.md` §4：本卡唯一白名单成员 `sys:ping`
/// 是对「名称同形」的**动作**转发，而**不是** ACP 标准方法——它**不在**
/// `ACP_METHODS` 内。这条同时守住「台账不把转发说成标准方法」。
#[test]
fn ping_is_a_forwarded_action_not_an_acp_method() {
    assert!(
        !ACP_METHODS.contains(&"sys:ping"),
        "sys:ping 是 v0 动作名，不应出现在 ACP 标准方法清单里"
    );
}

/// 从 `agent-acp/src/methods.rs` 的 `method` 模块源码文本里抽出所有
/// `pub const <NAME>: &str = "<value>";` 的字面量值。
///
/// 只认「常量名 + `: &str` + `=` + 字符串」这一形态；文档注释里的反引号方法名
/// （``` `session/new` ```）不含 `pub const`，天然不匹配。返回按源码顺序排列，
/// 调用方用集合比对（顺序无关）。
fn scan_method_consts(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in source.split("pub const ").skip(1) {
        // 常量名以 `:` 起（`INITIALIZE: &str = "initialize";`）。
        let Some((_, after_colon)) = chunk.split_once(':') else {
            continue;
        };
        // 必须是 `&str` 常量，且本段含 `="…";` 形态。
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
        // 取第一个 `"` 到下一个 `"` 之间的内容。
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

#[test]
fn agent_acp_method_consts_match_acp_methods() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-acp")
        .join("src")
        .join("methods.rs");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));

    assert_eq!(
        scan_method_consts(&source).len(),
        13,
        "methods.rs 的 `pub const … : &str` 应恰有 13 项；扫描器或源文件形态变了，先核对扫描器"
    );

    let scanned_names = scan_method_consts(&source);
    let scanned: std::collections::BTreeSet<&str> =
        scanned_names.iter().map(String::as_str).collect();
    let frozen: std::collections::BTreeSet<&str> = ACP_METHODS.iter().copied().collect();

    let missing: Vec<&&str> = frozen.difference(&scanned).collect();
    let extra: Vec<&&str> = scanned.difference(&frozen).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "agent-acp::method 与 ACP_METHODS 不一致：methods.rs 有而常量缺 {missing:?}；常量有而 methods.rs 缺 {extra:?}"
    );
}

#[test]
fn scanner_extracts_const_literals_only() {
    // 扫描器自检：只认 `pub const … : &str = "…";`，不认文档注释里的反引号名。
    let src = r#"
/// `session/new`（文档注释，不应被抽到）
pub const SESSION_NEW: &str = "session/new";
pub const LOGOUT: &str = "logout";
const PRIVATE: u8 = 3;
"#;
    assert_eq!(scan_method_consts(src), vec!["session/new", "logout"]);
}
