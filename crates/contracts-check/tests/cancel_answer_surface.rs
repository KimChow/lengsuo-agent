//! `session:cancel` 应答语义裁定闸（W2 · lane proto）。
//!
//! 裁定（台账 `docs/contracts/acp-method-ledger.md` §5）：`session:cancel`
//! **保持本地实现、不入 ACP 转发白名单**。依据：
//! - 转发机制是「请求-响应」（`rpc.rs::acp_forward` → `forward_action` 发
//!   Rpc 请求帧、按 id 等响应帧）；对端 `session/cancel` 是**通知、无响应**
//!   （`agent-acp/src/methods.rs`：`CancelNotification`「当前轮取消，无响应」）。
//!   原样转发 ⇒ 永远等不到响应 ⇒ 超时/断连 ⇒ 用户拿 `502`，而取消已发生。
//! - 参数命名空间无映射：v0 侧是本端 `Uuid`（`own()` 归属校验），ACP 侧
//!   `CancelNotification.session_id` 是对端会话 id，两侧无映射表。
//!
//! 本文件守**关系**（裁定闸），不锁数量/取值：下一张卡即使正当扩充白名单，
//! 也**不得**把本清单成员填进去——填了即这里红。源码用 `include_str!`
//! 编译期绑定，改动即触发重编。

use contracts_check::{
    ACP_FORWARDED_ACTIONS, ACP_FORWARD_WHITELIST_SNAPSHOT, ACTIONS, DUAL_PATH_ACTIONS,
    LOCAL_DISPATCH_ACTIONS, LOCAL_ONLY_ANSWER_ACTIONS,
};

const RPC_SOURCE: &str = include_str!("../../agent-gateway/src/rpc.rs");
const ACP_METHODS_SOURCE: &str = include_str!("../../agent-acp/src/methods.rs");

fn assert_unique(name: &str, items: &[&str]) {
    for (i, item) in items.iter().enumerate() {
        assert!(!items[..i].contains(item), "{name} 出现重复项：{item}");
    }
}

/// 非注释行：`trim_start()` 不以 `///` / `//!` / `//` / `*` 开头。
fn is_code_line(line: &str) -> bool {
    let t = line.trim_start();
    !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//") || t.starts_with('*'))
}

/// 从一行代码里抽出 `"域:动作"` 形状的字面量（口径同 `action_dispatch_surface.rs`）。
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

/// 截取 `fn run(` 内 `match action {` 块（按花括号深度配对，臂体自带 `{}` 不截断）。
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

// ---- 裁定闸 a）：LOCAL_ONLY_ANSWER_ACTIONS ∩ 转发白名单 == 空 ----

#[test]
fn local_only_actions_never_enter_the_forward_whitelist() {
    let overlap_snapshot: Vec<&str> = LOCAL_ONLY_ANSWER_ACTIONS
        .iter()
        .copied()
        .filter(|a| ACP_FORWARD_WHITELIST_SNAPSHOT.contains(a))
        .collect();
    let overlap_forwarded: Vec<&str> = LOCAL_ONLY_ANSWER_ACTIONS
        .iter()
        .copied()
        .filter(|a| ACP_FORWARDED_ACTIONS.contains(a))
        .collect();
    let overlap_dual: Vec<&str> = LOCAL_ONLY_ANSWER_ACTIONS
        .iter()
        .copied()
        .filter(|a| DUAL_PATH_ACTIONS.contains(a))
        .collect();
    assert!(
        overlap_snapshot.is_empty() && overlap_forwarded.is_empty() && overlap_dual.is_empty(),
        "裁定闸命中：{local_only:?} 不得进入转发白名单/双路径清单，越界 {overlap_snapshot:?} \
         / {overlap_forwarded:?} / {overlap_dual:?} ——该动作的 ACP 对端是通知/无响应，\
         转发（请求-响应机制）必然等不到响应：用户拿 502 而取消已在对端发生。\
         见 docs/contracts/acp-method-ledger.md §5 与 tests/cancel_answer_surface.rs",
        local_only = LOCAL_ONLY_ANSWER_ACTIONS,
    );
}

// ---- 裁定闸 b）：登记完备——本地应答的动作必须有本地分支 ----

#[test]
fn local_only_actions_are_locally_dispatched_and_known_v0_actions() {
    assert_unique("LOCAL_ONLY_ANSWER_ACTIONS", LOCAL_ONLY_ANSWER_ACTIONS);
    let outside_registry: Vec<&str> = LOCAL_ONLY_ANSWER_ACTIONS
        .iter()
        .copied()
        .filter(|a| !LOCAL_DISPATCH_ACTIONS.contains(a))
        .collect();
    assert!(
        outside_registry.is_empty(),
        "LOCAL_ONLY_ANSWER_ACTIONS 里 {outside_registry:?} 不在 LOCAL_DISPATCH_ACTIONS——\
         裁定为「本地应答」的动作必须真的有本地实现分支（先登记再裁定）"
    );
    let outside_actions: Vec<&str> = LOCAL_ONLY_ANSWER_ACTIONS
        .iter()
        .copied()
        .filter(|a| !ACTIONS.contains(a))
        .collect();
    assert!(
        outside_actions.is_empty(),
        "LOCAL_ONLY_ANSWER_ACTIONS 里 {outside_actions:?} 不在 ACTIONS——不得裁定未登记动作"
    );

    // 实证补充：`rpc.rs` 的 `match action` 块里必须真有该动作的分支
    //（与 action_dispatch_surface.rs 同口径：只认代码行上的 `域:动作` 字面量）。
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
    for action in LOCAL_ONLY_ANSWER_ACTIONS {
        assert!(
            arms.iter().any(|arm| arm == action),
            "本地应答动作 {action} 在 `rpc.rs` 的 `match action` 里没有本地分支——\
             本清单的裁定前提（本地实现存在且可达）被破坏"
        );
    }
}

// ---- 裁定闸 c）：形状依据固化——对端语义是「通知」不许被静默改成请求-响应 ----
//
// `agent_acp` 不是本 crate 的依赖（Cargo.toml 属共享区，本卡不改），故用**源码
// 文本断言**：`include_str!` 编译期绑定 `agent-acp/src/methods.rs`，断言
// `SESSION_CANCEL` 常量与其紧邻注释带「通知/无响应」锚点。锚点被删/改写 ⇒
// 本测试红 ⇒ 强制有人重新评估本清单（而不是清单对端语义变更毫不知情）。

#[test]
fn acp_session_cancel_remains_a_responseless_notification() {
    // 1) 常量行本身：`pub const SESSION_CANCEL: &str = "session/cancel";`
    //    （同时确认对端方法名仍是斜杠式 `session/cancel`——清单依据的那一行）。
    let const_line = ACP_METHODS_SOURCE
        .lines()
        .find(|line| line.contains("pub const SESSION_CANCEL"))
        .unwrap_or_else(|| {
            panic!(
                "agent-acp/src/methods.rs 里找不到 `pub const SESSION_CANCEL`——\
                 对端方法表形态变了，须重新核对 LOCAL_ONLY_ANSWER_ACTIONS 的裁定依据"
            )
        });
    assert!(
        const_line.contains("\"session/cancel\""),
        "SESSION_CANCEL 的方法名字面变了：{const_line}——本地清单登记的斜杠名不再成立"
    );

    // 2) 锚点：常量行（或其文档注释）附近必须仍写明「通知」属性。
    //    现形态是同行文档注释 `/// `session/cancel`（通知）`；日后若挪进多行
    //    注释或参数类型注释，锚点窗口放宽到前后 4 行，只要任一行带锚点即绿。
    let idx = ACP_METHODS_SOURCE
        .lines()
        .position(|line| line.contains("pub const SESSION_CANCEL"))
        .expect("已在上文确认存在");
    let window_start = idx.saturating_sub(4);
    let window_end = (idx + 4).min(ACP_METHODS_SOURCE.lines().count());
    let window: Vec<&str> = ACP_METHODS_SOURCE
        .lines()
        .skip(window_start)
        .take(window_end - window_start)
        .collect();
    assert!(
        window
            .iter()
            .any(|line| line.contains("通知") || line.contains("无响应")),
        "SESSION_CANCEL（{const_line}）附近的「通知/无响应」锚点消失了——\
         对端语义疑似从通知改成请求-响应：若属实，session:cancel 的裁定可以重开\
         （改回请求-响应后转发形状才成立），但必须先更新台账 §5 与 \
         LOCAL_ONLY_ANSWER_ACTIONS，而不是静默入白名单"
    );

    // 3) 参数类型佐证：`CancelNotification`（命名即通知族）仍存在，且其文档注释
    //    仍带「无响应」——这是「对端无响应帧」的第二独立锚点。
    assert!(
        ACP_METHODS_SOURCE.contains("pub struct CancelNotification"),
        "agent-acp/src/methods.rs 里找不到 `CancelNotification`——参数类型形态变了，\
         须重新核对 session/cancel 的通知语义"
    );
    let cancel_doc = ACP_METHODS_SOURCE.lines().find(|line| {
        line.contains("session/cancel") && (line.contains("通知") || line.contains("无响应"))
    });
    assert!(
        cancel_doc.is_some(),
        "methods.rs 里找不到带「通知/无响应」锚点的 session/cancel 行——对端语义变更未走契约流程"
    );
}
