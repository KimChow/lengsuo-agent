//! 动作分派面断言（W2 · lane proto）：`/rpc` 的一个动作**由谁应答**。
//!
//! 台账：`docs/contracts/acp-method-ledger.md` §7。分派顺序（`rpc.rs`）：
//! `run()`（130）第一句 `acp_forward(...)?`（113）命中即返回，否则落到
//! `match action`（131）本地实现。⇒ **转发旁路优先于本地实现**。
//!
//! 本文件只守**关系**，刻意**不**锁数量、不锁取值：
//! - 不写 `assert_eq!(LOCAL_DISPATCH_ACTIONS.len(), N)`；
//! - 不写 `assert_eq!(ACP_FORWARDED_ACTIONS, &["sys:ping"])`——下一张卡正当扩白名单
//!   时只同步镜像常量即可，本文件不用改。
//!
//! 源码用 `include_str!` 在**编译期**绑定，不用运行期找路径：改动 `rpc.rs` 即触发
//! 本测试重编；扫描失败一律报出扫描器/源文件形态，避免「扫出空集 ⇒ 断言空转通过」。

use contracts_check::{
    ACP_FORWARDED_ACTIONS, ACP_FORWARD_WHITELIST_SNAPSHOT, ACP_METHODS, ACTIONS, DUAL_PATH_ACTIONS,
    LOCAL_DISPATCH_ACTIONS,
};

const RPC_SOURCE: &str = include_str!("../../agent-gateway/src/rpc.rs");

fn assert_unique(name: &str, items: &[&str]) {
    for (i, item) in items.iter().enumerate() {
        assert!(!items[..i].contains(item), "{name} 出现重复项：{item}");
    }
}

/// 对 `assert!(cond)` 的补充：失败时把越界项打出来，定位「谁多谁少」。
fn assert_subset(name: &str, sub: &[&str], sup: &[&str]) {
    let outside: Vec<&str> = sub.iter().copied().filter(|x| !sup.contains(x)).collect();
    assert!(
        outside.is_empty(),
        "{name} 越界：{outside:?} 不在 {sup:?} 里"
    );
}

/// 非注释行：`trim_start()` 不以 `///` / `//!` / `//` / `*` 开头。
fn is_code_line(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//") || t.starts_with('*'))
}

/// 从一行代码里抽出**恰好一个** `"域:动作"` 形状的字面量（`[a-z]+:[a-z_]+`）。
///
/// 手写字符过滤，不引依赖。形状之外的一律不认：`"core::mem"`（冒号两侧取空）、
/// `"http://x"`（域含 `:` 与 `/`）、`"*/group:subgroup"`（域含 `*`）。
fn action_literals_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let Some(end) = rest[start + 1..].find('"') else {
            break;
        };
        let lit = &rest[start + 1..start + 1 + end];
        let ok_shape = lit.split_once(':').is_some_and(|(d, a)| {
            !d.is_empty()
                && !a.is_empty()
                && d.chars().all(|c| c.is_ascii_lowercase())
                && a.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        });
        if ok_shape {
            out.push(lit.to_owned());
        }
        rest = &rest[start + 1 + end + 1..];
    }
    out
}

/// 抽出源码里所有**代码行**上的 `"域:动作"` 字面量（文档注释里的示例不算实现面）。
fn scan_action_literals(source: &str) -> Vec<String> {
    source
        .lines()
        .filter(|line| is_code_line(line))
        .flat_map(action_literals_in)
        .collect()
}

/// 截取 `fn run(` 内 `match action {` 起的臂列表（到与 `match` 同缩进的 `}` 为止）。
///
/// ⚠️ 不能取「第一个 `"域:动作"` 到第一个 `}`」：`"sys:ping" => json!({...})`
/// 臂体自带花括号，会过早截断。故按**花括号深度**配对 `match action` 的块。
fn local_match_block(source: &str) -> &str {
    let fn_at = source
        .find("fn run(")
        .unwrap_or_else(|| panic!("rpc.rs 里找不到 `fn run(`——扫描器或源文件形态变了"));
    let head = &source[fn_at..];
    let open = head
        .find("match action {")
        .unwrap_or_else(|| panic!("`fn run(` 里找不到 `match action {{`——扫描器或源文件形态变了"));
    let body_start = fn_at + open + "match action ".len();
    let mut depth = 0usize;
    for (i, c) in source[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[body_start..body_start + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("`match action` 块没有配对的 `}}`");
}

/// 截取 `pub const ACP_FORWARD_ACTIONS: ... = &[ ... ];` 的数组体。
fn forward_const_block(source: &str) -> &str {
    let at = source
        .find("pub const ACP_FORWARD_ACTIONS")
        .unwrap_or_else(|| panic!("rpc.rs 里找不到 `pub const ACP_FORWARD_ACTIONS`"));
    let tail = &source[at..];
    // 类型标注也是 `&[&str]`，故先越过 `=` 再找数组体。
    let eq = tail
        .find('=')
        .expect("ACP_FORWARD_ACTIONS 没有 `=` 赋值号——源文件形态变了");
    let after_eq = &tail[eq..];
    let start = after_eq
        .find("&[")
        .expect("ACP_FORWARD_ACTIONS 的值不是 `&[` 数组")
        + 2;
    let end = after_eq[start..]
        .find(']')
        .expect("ACP_FORWARD_ACTIONS 数组没有收尾 `]`");
    &after_eq[start..start + end]
}

#[test]
fn scanner_extracts_action_literals_from_code_lines_only() {
    // 扫描器自检：文档注释里的示例不算，非 `域:动作` 形状不算。
    let src = r#"
/// 示例：`"sys:ping"` 只是文档
// "session:list" 注释
    "sys:ping" => json!({"pid":std::process::id()}),
    let path = "core::mem";
    let url = "http://x";
    let group = "*/group:subgroup";
"#;
    assert_eq!(
        scan_action_literals(src),
        vec!["sys:ping".to_owned()],
        "只认代码行上的 `域:动作` 字面量"
    );
}

#[test]
fn local_match_block_is_brace_matched() {
    // 臂体自带 `{...}` 时不得截断（`json!({...})` 是 `sys:ping` 的真实形态）。
    let src = r#"fn run(app: &App) -> u8 {
    Ok(match action {
        "sys:ping" => json!({"pid":1}),
        "x:y" => { 1 }
        _ => 0,
    })
}
"#;
    let block = local_match_block(src);
    let names = scan_action_literals(block);
    assert_eq!(names, vec!["sys:ping", "x:y"], "块截取错位：{block}");
    assert!(block.trim_end().ends_with('}'), "块必须收在 match 的 `}}`");
}

// ---- 断言 1：白名单（镜像）⊆ 动作面，防自创名 ----

#[test]
fn forwarded_actions_are_subset_of_actions() {
    // 白名单改了但契约忘了同步 ⇒ 这里先红（源码镜像漂移由断言 3 抓）。
    assert_subset("ACP_FORWARDED_ACTIONS", ACP_FORWARDED_ACTIONS, ACTIONS);
    assert_subset(
        "ACP_FORWARD_WHITELIST_SNAPSHOT",
        ACP_FORWARD_WHITELIST_SNAPSHOT,
        ACTIONS,
    );
    // 两份白名单镜像必须同值：否则一个守源码、一个守动作面，中间漏一段。
    assert_subset(
        "ACP_FORWARD_WHITELIST_SNAPSHOT",
        ACP_FORWARD_WHITELIST_SNAPSHOT,
        ACP_FORWARDED_ACTIONS,
    );
    assert_subset(
        "ACP_FORWARDED_ACTIONS",
        ACP_FORWARDED_ACTIONS,
        ACP_FORWARD_WHITELIST_SNAPSHOT,
    );
    assert_unique("ACP_FORWARDED_ACTIONS", ACP_FORWARDED_ACTIONS);
}

// ---- 断言 2：源码 `match action` 块 ⊆ LOCAL_DISPATCH_ACTIONS ----

#[test]
fn local_match_arms_are_subset_of_local_dispatch_actions() {
    let block = local_match_block(RPC_SOURCE);
    let arms: Vec<String> = block
        .lines()
        .filter(|line| is_code_line(line) && line.trim_start().starts_with('"'))
        .flat_map(action_literals_in)
        .collect();
    assert!(
        !arms.is_empty(),
        "`match action` 块里没扫到任何动作臂——扫描器或源文件形态变了：{block}"
    );
    let arm_refs: Vec<&str> = arms.iter().map(String::as_str).collect();
    assert_unique("match action 臂", &arm_refs);
    let outside: Vec<&str> = arm_refs
        .iter()
        .copied()
        .filter(|arm| !LOCAL_DISPATCH_ACTIONS.contains(arm))
        .collect();
    assert!(
        outside.is_empty(),
        "本地实现出现未登记动作 {outside:?}——先登记进 LOCAL_DISPATCH_ACTIONS 与台账 §7"
    );
}

// ---- 断言 3：源码 ACP_FORWARD_ACTIONS 常量 == ACP_FORWARDED_ACTIONS（镜像漂移） ----

#[test]
fn forward_const_literals_match_forwarded_actions() {
    let block = forward_const_block(RPC_SOURCE);
    let scanned = scan_action_literals(block);
    assert!(
        !scanned.is_empty(),
        "`ACP_FORWARD_ACTIONS` 数组里没扫到动作名——扫描器或源文件形态变了：{block}"
    );
    let scanned: Vec<&str> = scanned.iter().map(String::as_str).collect();
    let missing: Vec<&str> = scanned
        .iter()
        .copied()
        .filter(|x| !ACP_FORWARDED_ACTIONS.contains(x))
        .collect();
    let extra: Vec<&str> = ACP_FORWARDED_ACTIONS
        .iter()
        .copied()
        .filter(|x| !scanned.contains(x))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "rpc.rs::ACP_FORWARD_ACTIONS 与契约镜像漂移：源码有而镜像缺 {missing:?}；镜像有而源码缺 {extra:?}"
    );
}

// ---- 断言 4：双路径动作两侧都必须有实现 ----

#[test]
fn dual_path_actions_have_impl_on_both_paths() {
    assert_subset(
        "DUAL_PATH_ACTIONS",
        DUAL_PATH_ACTIONS,
        LOCAL_DISPATCH_ACTIONS,
    );
    assert_subset(
        "DUAL_PATH_ACTIONS",
        DUAL_PATH_ACTIONS,
        ACP_FORWARDED_ACTIONS,
    );
    assert_unique("DUAL_PATH_ACTIONS", DUAL_PATH_ACTIONS);

    // 实证补充：每个双路径动作必须真的能在 `match action` 块里看到分支
    // （上两条只守「登记表之间」的关系；这里守「登记 vs 源码」）。
    let block = local_match_block(RPC_SOURCE);
    let arms = scan_action_literals(block);
    for action in DUAL_PATH_ACTIONS {
        assert!(
            arms.iter().any(|arm| arm == action),
            "双路径动作 {action} 在 `match action` 里没有本地分支——未挂 acp 的网关会静默回落 -32601"
        );
    }
}

// ---- 断言 5：转发动作名与 ACP wire 方法名不相交（两套命名空间） ----

#[test]
fn forwarded_actions_do_not_collide_with_acp_methods() {
    let overlap: Vec<&str> = ACP_FORWARDED_ACTIONS
        .iter()
        .copied()
        .filter(|action| ACP_METHODS.contains(action))
        .collect();
    assert!(
        overlap.is_empty(),
        "转发动作名同时是 ACP wire 方法名 {overlap:?}——两套命名空间不得混用"
    );
}

// ---- 登记表与动作面的整体关系（集合相等） ----

#[test]
fn local_dispatch_actions_equal_actions() {
    assert_unique("LOCAL_DISPATCH_ACTIONS", LOCAL_DISPATCH_ACTIONS);
    let missing: Vec<&str> = ACTIONS
        .iter()
        .copied()
        .filter(|a| !LOCAL_DISPATCH_ACTIONS.contains(a))
        .collect();
    let extra: Vec<&str> = LOCAL_DISPATCH_ACTIONS
        .iter()
        .copied()
        .filter(|a| !ACTIONS.contains(a))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "LOCAL_DISPATCH_ACTIONS 与 ACTIONS 不相等：在 ACTIONS 但无本地分支 {missing:?}；有本地登记但不在 ACTIONS {extra:?}"
    );
}
