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
        "session:get" | "session:stop" | "session:send" => json!({}),
        // 缺 afterSeq → 网关显式 `bad()` 400；`session:replay` 已在 `run()` 注册。
        "session:replay" => json!({}),
        // 缺 approvalId → 网关参数错 400。
        "approval:respond" => json!({}),
        // 请求体里带 isolation → 被拒 403（服务端绑定身份），动作本身已注册。
        "session:create" => json!({"isolation": {"user_id": "x", "tenant_id": "x"}}),
        other => panic!("ACTIONS 里的动作没有探测 payload：{other}"),
    }
}

/// 常量本身必须与 `AGENT-KICKOFF.md` 第 24 行列的 11 个动作**逐个**对上。
///
/// 这条挡住「常量被删项」：删掉 `ACTIONS` 里的某项会让上面的循环少跑一圈、
/// 悄无声息地漏测。计数与集合都写死在这里，任何增删都要同时改测试。
#[test]
fn frozen_action_list_matches_kickoff_line_24() {
    let expected = [
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
    assert_eq!(ACTIONS.len(), 11, "v0 动作面应为 11 个");
    for name in expected {
        assert!(
            ACTIONS.contains(&name),
            "`AGENT-KICKOFF.md` 第 24 行冻结的动作 {name} 不在 ACTIONS 里"
        );
    }
    // 反向：常量里不得有 §2 未登记的动作。
    for action in ACTIONS {
        assert!(
            expected.contains(action),
            "常量里的 {action} 未在 `AGENT-KICKOFF.md` 第 24 行登记"
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
