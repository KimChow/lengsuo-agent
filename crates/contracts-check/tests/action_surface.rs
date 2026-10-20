//! 动作面穷举：`ACTIONS` 里每个动作都必须**已注册**，未登记的必须 404。
//!
//! 双向红的机制：
//! - 「文档列了但代码没实现」→ 该动作落进 `run()` 的 `_ => 404` 兜底，测试红。
//! - 「代码实现但文档没登记」→ 本测试覆盖不到；靠 `registered_action_surface_is_frozen`
//!   反向断言「`ACTIONS` 之外的域:动作也全部 404」，实现里多注册一个域外动作就红。
use contracts_check::ACTIONS;

mod support;
use support::{request, start_gateway};

/// 每个动作的探测 payload：一律给**非法** payload，只要它被路由到（错误码不是
/// 鉴权/未注册）即证明「已注册」。
///
/// 关键：`agent-gateway` 的未知动作与 payload 类型错都会落进状态码 404 一族
/// （`unknown action`）或参数错 400，所以用 `code` 猜不出来，只能**按动作分别**
/// 判定「已注册」可接受的状态码集合。
fn payload_for(action: &str) -> serde_json::Value {
    use serde_json::json;
    match action {
        // 无 session 上下文即可返回成功。
        "sys:ping" | "session:list" | "config:get" | "tool:list" => json!({}),
        // 写入一个必然合法的配置键。
        "config:set" => json!({"key": "defaultMode", "value": "quick"}),
        // 缺 id → 网关参数错 400（**不可能是 404**，因为动作已注册）。
        "session:get" | "session:stop" | "session:send" | "session:cancel" => json!({}),
        // 缺 afterSeq → 网关显式 `bad()` 400；`session:replay` 已在 `run()` 注册。
        "session:replay" => json!({}),
        // 缺 approvalId → 网关参数错 400。
        "approval:respond" => json!({}),
        // 请求体里带 isolation → 被拒 403（服务端绑定身份），动作本身已注册。
        "session:create" => json!({"isolation": {"user_id": "x", "tenant_id": "x"}}),
        other => panic!("ACTIONS 里的动作没有探测 payload：{other}"),
    }
}

/// 常量必须与「`AGENT-KICKOFF.md` 第 24 行冻结的 11 个动作 + 台账登记的追加项」
/// **逐个**对上。
///
/// 这条挡住「常量被删项」：删掉 `ACTIONS` 里的某项会让上面的循环少跑一圈、
/// 悄无声息地漏测。冻结项与追加项都写死在这里，任何增删都要同时改测试。
/// 刻意**不**写 `assert_eq!(ACTIONS.len(), N)`：数量锁会让每次正当扩充必红，
/// 下面正反两向的集合比对已同时挡住删项与私增项。
#[test]
fn frozen_action_list_matches_kickoff_and_ledger() {
    // `AGENT-KICKOFF.md` 第 24 行冻结的 11 个（`session:replay` 为
    // 2026-10-19「只增不改」追加，同见该行）。
    let frozen_line_24 = [
        "session:list",
        "session:create",
        "session:get",
        "session:send",
        "session:stop",
        "session:replay",
        "approval:respond",
        "config:get",
        "config:set",
        "tool:list",
        "sys:ping",
    ];
    // `08b9f40` 新增、经 `docs/contracts/acp-method-ledger.md` §1 补登记的追加项。
    let ledger_appendix = ["session:cancel"];

    for name in frozen_line_24 {
        assert!(
            ACTIONS.contains(&name),
            "`AGENT-KICKOFF.md` 第 24 行冻结的动作 {name} 不在 ACTIONS 里"
        );
    }
    for name in ledger_appendix {
        assert!(
            ACTIONS.contains(&name),
            "台账登记的追加动作 {name} 不在 ACTIONS 里"
        );
    }
    // 反向：常量里不得有既非第 24 行冻结、又非台账登记追加项的动作。
    for action in ACTIONS {
        assert!(
            frozen_line_24.contains(action) || ledger_appendix.contains(action),
            "常量里的 {action} 既未在 `AGENT-KICKOFF.md` 第 24 行登记，也不是台账登记的追加项"
        );
    }
}

#[tokio::test]
async fn every_frozen_action_is_registered() {
    let (g, dir) = start_gateway().await;
    // 先确认握手文件与句柄一致：端口/token 都从文件面取，模拟真实客户端。
    let hs = support::read_handshake(dir.path());
    assert_eq!(hs["port"], g.port());
    assert_eq!(hs["token"].as_str().unwrap(), g.token());

    for action in ACTIONS {
        let (status, env) = request(g.port(), g.token(), action, payload_for(action)).await;
        // 断言不是「未注册」/「鉴权拒绝」：
        // - 未知动作在 `agent-gateway` 里返回 `code == 404`；
        // - 鉴权拒绝返回 `code == 403`（本用例 token 正确，理论上不会出现）。
        let code = env["code"].as_u64().unwrap() as u16;
        assert_ne!(status, 403, "动作 {action} 被鉴权拒（token 正确）: {env}");
        assert_ne!(
            code, 404,
            "动作 {action} 未注册（文档列了但代码没实现）: {env}"
        );
        assert!(
            matches!(code, 0 | 400 | 403 | 408),
            "动作 {action} 返回未冻结的错误码 {code}: {env}"
        );
    }

    g.shutdown().await.unwrap();
}

#[tokio::test]
async fn nonexistent_action_is_404() {
    let (g, _dir) = start_gateway().await;
    let (status, env) = request(
        g.port(),
        g.token(),
        "nonexistent:action",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, 200, "未注册动作走的是 200 + 信封 404: {env}");
    assert_eq!(env["code"], 404, "未注册动作必须 404: {env}");
    assert_eq!(env["msg"], "unknown action");
    assert!(env["data"].is_null());
    g.shutdown().await.unwrap();
}

/// 反向断言：`ACTIONS` 之外的**同名域**其他动作也应当未注册。
///
/// 这条挡住「代码实现但文档没登记：悄悄多注册一个动作」。本仓 v0 每个域的
/// 动作面都是封闭枚举，任取一个域外名字都必须 404。
#[tokio::test]
async fn registered_action_surface_is_frozen() {
    let (g, _dir) = start_gateway().await;
    let unregistered = [
        "session:delete",
        "session:rename",
        "approval:list",
        "config:unset",
        "tool:call",
        "sys:version",
    ];
    for action in unregistered {
        let (_, env) = request(g.port(), g.token(), action, serde_json::json!({})).await;
        assert_eq!(
            env["code"], 404,
            "动作 {action} 未登记在合同里却已实现（文档缺口）: {env}"
        );
    }
    g.shutdown().await.unwrap();
}

/// 从 `crates/agent-gateway/src/rpc.rs` 源码文本里抽出所有形如 `"域:动作"` 的
/// 字符串字面量：**含且仅含一个冒号、冒号两侧均为 ASCII 标识符**。
///
/// 思路同 `tests/method_ledger.rs::scan_method_consts`：只读文件、手写字符
/// 过滤，不新增依赖。这是**全文件**扫描——既覆盖 `run()` 的 `match action`
/// 动作分支，也顺带覆盖 `ACP_FORWARD_ACTIONS` 白名单等处的动作名字面量（它们
/// 同样必须是契约内的动作名）。payload 键名（`"sessionId"`、`"afterSeq"` 等）
/// 不含冒号，天然被过滤掉。
fn scan_action_literals(source: &str) -> Vec<String> {
    fn is_ascii_ident(s: &str) -> bool {
        let mut chars = s.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find('"') {
        let Some(end) = rest[start + 1..].find('"') else {
            break;
        };
        let lit = &rest[start + 1..start + 1 + end];
        if let Some((domain, action)) = lit.split_once(':') {
            if !action.contains(':') && is_ascii_ident(domain) && is_ascii_ident(action) {
                out.push(lit.to_owned());
            }
        }
        rest = &rest[start + 1 + end + 1..];
    }
    out
}

/// 关系不变量（本卡的核心守点）：`rpc.rs` 实现里出现的 `"域:动作"` 字面量
/// 集合 ⊆ [`ACTIONS`]。
///
/// 这条把「实现里有的动作 ⇒ 契约里必有」变成可执行断言，挡住「代码实现了、
/// 契约漏登记」（如 `08b9f40` 新增的 `session:cancel` 先落在实现里、契约迟到）。
/// 刻意**只守关系、不锁数量**——数量口径归 `method_ledger.rs`；正当扩充动作面
/// 时，先把新动作登记进 `ACTIONS` 与台账，本断言自然变绿。
#[test]
fn rpc_action_literals_are_subset_of_actions() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-gateway")
        .join("src")
        .join("rpc.rs");
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));

    let scanned_names = scan_action_literals(&source);
    let scanned: std::collections::BTreeSet<&str> =
        scanned_names.iter().map(String::as_str).collect();
    let frozen: std::collections::BTreeSet<&str> = ACTIONS.iter().copied().collect();

    let unregistered: Vec<&&str> = scanned.difference(&frozen).collect();
    assert!(
        unregistered.is_empty(),
        "rpc.rs 实现了但 ACTIONS 未登记的动作（差集）{unregistered:?}——契约落后于实现，先补登记再合并"
    );
}
