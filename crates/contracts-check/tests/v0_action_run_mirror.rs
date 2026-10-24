//! 动作面清单 `V0_ACTIONS` 与 `run()` match 臂的双向集合相等断言。
//!
//! 三方集合：
//! - `V0_ACTIONS`（agent-gateway 权威，agent-gateway/src/actions.rs）
//! - `run()` match 臂（扫描自 agent-gateway/src/rpc.rs）
//! - `LOCAL_DISPATCH_ACTIONS`（contracts-check 镜像，contracts-check/src/actions.rs）
//!
//! 三者必须两两相等。刻意不锁数量，只守集合相等关系。

use agent_gateway::V0_ACTIONS;
use contracts_check::LOCAL_DISPATCH_ACTIONS;

const RPC_SOURCE: &str = include_str!("../../agent-gateway/src/rpc.rs");

// ---- 扫描器（逐字复制自 crates/contracts-check/tests/action_dispatch_surface.rs，
// 测试文件间不可 import，故在此复制）----

/// 非注释行：`trim_start()` 不以 `///` / `//!` / `//` / `*` 开头。
fn is_code_line(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//") || t.starts_with('*'))
}

/// 从一行代码里抽出**恰好一个** `"域:动作"` 形状的字面量（`[a-z]+:[a-z_]+`）。
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

/// 从 match 块里扫出动作臂名（只取以 `"` 开头的代码行上的字面量）。
fn scan_match_arms(block: &str) -> Vec<String> {
    block
        .lines()
        .filter(|line| is_code_line(line) && line.trim_start().starts_with('"'))
        .flat_map(action_literals_in)
        .collect()
}

/// 判定核心：`a` 里不在 `b` 里的项（`a ⊆ b` 成立时为空）。
/// 真源断言与合成样本自证走**同一个**函数，样本红了才证明这条判定真的会红。
fn outside(a: &[&str], b: &[&str]) -> Vec<String> {
    a.iter()
        .filter(|x| !b.contains(x))
        .map(|x| (*x).to_owned())
        .collect()
}

/// 把扫出的 arm 字符串转为 &str 切片，便于与 V0_ACTIONS / LOCAL_DISPATCH_ACTIONS 比较。
fn arm_refs(arms: &[String]) -> Vec<&str> {
    arms.iter().map(String::as_str).collect()
}

// ---- 断言 1：扫描器防空扫 ----

#[test]
fn scanner_extracts_non_empty_arms_and_v0_actions() {
    let block = local_match_block(RPC_SOURCE);
    let arms: Vec<String> = scan_match_arms(block);
    assert!(
        !arms.is_empty(),
        "`match action` 块里没扫到任何动作臂——扫描器或源文件形态变了：{block}"
    );
    assert!(
        !V0_ACTIONS.is_empty(),
        "`V0_ACTIONS` 为空——权威清单不可为空，否则双向包容会空转通过"
    );
}

// ---- 断言 2：V0_ACTIONS ⊆ 臂（冻结动作必须在 run() 里有实现臂）----

#[test]
fn v0_actions_subset_of_match_arms() {
    let block = local_match_block(RPC_SOURCE);
    let arms: Vec<String> = scan_match_arms(block);
    let arm_refs = arm_refs(&arms);
    let missing = outside(V0_ACTIONS, &arm_refs);
    assert!(
        missing.is_empty(),
        "冻结动作的实现臂缺失 => 该动作会落 404 unknown action，契约面撒谎。\
         missing: {:?}；当前臂: {:?}",
        missing,
        arm_refs
    );
}

// ---- 断言 3：臂 ⊆ V0_ACTIONS（不许有未登记的实现）----

#[test]
fn match_arms_subset_of_v0_actions() {
    let block = local_match_block(RPC_SOURCE);
    let arms: Vec<String> = scan_match_arms(block);
    let arm_refs = arm_refs(&arms);
    let extra = outside(&arm_refs, V0_ACTIONS);
    assert!(
        extra.is_empty(),
        "`run()` 新增动作须先登记进 `V0_ACTIONS` + 台账 §1。extra: {:?}",
        extra
    );
}

// ---- 断言 4：LOCAL_DISPATCH_ACTIONS ⊆ 臂（镜像不许有死项）----

#[test]
fn local_dispatch_actions_subset_of_match_arms() {
    let block = local_match_block(RPC_SOURCE);
    let arms: Vec<String> = scan_match_arms(block);
    let arm_refs = arm_refs(&arms);
    let dead = outside(LOCAL_DISPATCH_ACTIONS, &arm_refs);
    assert!(
        dead.is_empty(),
        "`LOCAL_DISPATCH_ACTIONS` 里有死项（实现已删、镜像没删）。dead: {:?}",
        dead
    );
}

// ---- 断言 5：合成样本自证扫描器真的能红（判定抽成纯函数，不改真源）----

#[test]
fn synthetic_sample_detects_missing_and_extra() {
    let real_block = local_match_block(RPC_SOURCE);
    let real_arms: Vec<String> = scan_match_arms(real_block);
    let first = real_arms.first().expect("真实臂非空，见断言 1").clone();

    // 合成样本 A：拿真实块文本，把某个臂名改成 "zzz:nope"
    // => 第 3 条判定（arms ⊆ V0_ACTIONS）必须报 extra。
    let replaced = real_block.replace(&format!("\"{first}\""), "\"zzz:nope\"");
    assert_ne!(replaced, real_block, "替换没生效：{first} 不在块里");
    let arms_a: Vec<String> = scan_match_arms(&replaced);
    let extra_a = outside(&arm_refs(&arms_a), V0_ACTIONS);
    assert!(
        extra_a.iter().any(|x| x == "zzz:nope"),
        "把臂名改成 \"zzz:nope\" 后，extra 判定必须捕获；实际 extra: {:?}",
        extra_a
    );

    // 合成样本 B：从真实块文本里删掉一个臂
    // => 第 2 条判定（V0_ACTIONS ⊆ arms）必须报 missing。
    let removed: String = real_block
        .lines()
        .filter(|line| !line.contains(&format!("\"{first}\"")))
        .collect::<Vec<_>>()
        .join("\n");
    let arms_b: Vec<String> = scan_match_arms(&removed);
    let missing_b = outside(V0_ACTIONS, &arm_refs(&arms_b));
    assert!(
        missing_b.iter().any(|x| x == &first),
        "删掉臂 {first} 后，missing 判定必须捕获；实际 missing: {:?}",
        missing_b
    );
}

// ---- 断言 6：注释/常量行不参与臂扫描（ACP_FORWARD_ACTIONS、文档注释里的动作名不算）----

#[test]
fn comment_and_constant_lines_do_not_participate() {
    // 构造合成块：里面夹带注释里的动作名，只保留代码行上的臂。
    let synthetic = r#"
        // "session:list" 只是注释，不算
        /// "config:get" 文档示例，不算
        "sys:ping" => json!({"pid":1}),
        /* "tool:list" 块注释，不算 */
        "config:set" => json!({}),
    "#;
    let arms: Vec<String> = scan_match_arms(synthetic);
    let names: Vec<&str> = arms.iter().map(String::as_str).collect();
    assert_eq!(
        names,
        vec!["sys:ping", "config:set"],
        "只取代码行上的引号开头臂，注释/常量行不参与：{:?}",
        names
    );

    // 合成块臂必须都在 V0_ACTIONS 内。
    let arm_refs = arm_refs(&arms);
    assert!(
        arm_refs.iter().all(|a| V0_ACTIONS.contains(a)),
        "合成块臂必须都在 V0_ACTIONS 内：{:?}",
        arm_refs
    );

    // 常量行 `ACP_FORWARD_ACTIONS = &["sys:ping"]` 是**代码行**，全量扫描会认它，
    // 文档注释里的动作名不会——正因如此臂扫描才要再加 `starts_with('"')` 收窄。
    let synthetic_src = r#"
/// 文档示例 "tool:list"
pub const ACP_FORWARD_ACTIONS: &[&str] = &["sys:ping"];
"#;
    assert_eq!(
        scan_action_literals(synthetic_src),
        vec!["sys:ping".to_owned()],
        "全量代码行扫描只认代码行上的字面量，文档注释里的不算"
    );
}
