//! `session:replay` 的**语义**契约：半开区间 `(afterSeq, …]`、`limit` 夹取、
//! `lastSeq` 回退、未知会话 404、未注册动作 404。
//!
//! 动作面卡（`action_surface.rs`）只保证 `session:replay`「被路由到、不是 404」；
//! 本文件补上 `afterSeq` / `limit` / `lastSeq` / 返回键名的**可执行语义断言**。
//!
//! 全走**真实网关**（`support::start_gateway` + `support::request`），断言的是
//! 线格式（经 `json!` 序列化的 `Event`），不是内存里的 `agent-core` 结构体——
//! 契约边界在网关信封这一层。
//!
//! 真相来源：
//! - 网关分支 `crates/agent-gateway/src/rpc.rs` 的 `"session:replay"`；
//! - 存储语义 `crates/agent-core/src/event_log/jsonl.rs` 的 `replay()`；
//! - 上限常量 `crates/agent-core/src/event_log/mod.rs` 的 `DEFAULT_PAGE_LIMIT`。
use contracts_check::{REPLAY_DATA_KEYS, REPLAY_MAX_PAGE};
use serde_json::{json, Value};

mod support;
use support::{request, start_gateway};

/// 建会话并回发其 `id`（`session:create` 的 `data.id`）。
async fn create_session(g: &agent_gateway::GatewayHandle, title: &str) -> String {
    let (_, env) = request(
        g.port(),
        g.token(),
        "session:create",
        json!({"title": title}),
    )
    .await;
    assert_eq!(env["code"], 0, "建会话必须成功: {env}");
    env["data"]["id"].as_str().unwrap().to_owned()
}

/// `session:send` 一段文本（落 `user` + echo provider 的 thinking/text/done，共 4 条）。
async fn send(g: &agent_gateway::GatewayHandle, id: &str, text: &str) {
    let (_, env) = request(
        g.port(),
        g.token(),
        "session:send",
        json!({"id": id, "text": text}),
    )
    .await;
    assert_eq!(env["code"], 0, "发消息必须成功: {env}");
}

/// 回放并返回整个信封（成功时 `code==0`）。
async fn replay(g: &agent_gateway::GatewayHandle, payload: Value) -> Value {
    let (_, env) = request(g.port(), g.token(), "session:replay", payload).await;
    env
}

/// 从成功信封里取出 `data.events` 的 `seq` 序列。
fn seqs(env: &Value) -> Vec<u64> {
    env["data"]["events"]
        .as_array()
        .expect("events 必须是数组")
        .iter()
        .map(|e| e["seq"].as_u64().expect("每条事件必须有数字 seq"))
        .collect()
}

/// 建会话 → 发消息 → `afterSeq=0` 拉全量：`seq` 严格递增，`lastSeq == events.last().seq`。
#[tokio::test]
async fn full_replay_is_strictly_increasing_and_last_seq_matches_tail() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "replay").await;
    send(&g, &id, "hello").await;

    let env = replay(&g, json!({"id": id, "afterSeq": 0})).await;
    assert_eq!(env["code"], 0, "回放必须成功: {env}");
    let seqs = seqs(&env);
    assert!(!seqs.is_empty(), "建会话 + 发消息后至少应有一条事件: {env}");
    assert!(
        seqs.iter().all(|&s| s > 0),
        "afterSeq=0 应含全部事件，seq 从 1 起"
    );
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "事件必须按 seq 严格递增返回: {seqs:?}"
    );
    // lastSeq == 末条事件的 seq（非空页）。
    assert_eq!(
        env["data"]["lastSeq"].as_u64().unwrap(),
        *seqs.last().unwrap(),
        "lastSeq 必须等于末条事件的 seq: {env}"
    );
    // 全量回放下末条 seq 即会话最大 seq，与支持随机访问的 send 回执一致。
    assert_eq!(
        env["data"]["lastSeq"].as_u64().unwrap(),
        seqs.iter().max().copied().unwrap()
    );

    g.shutdown().await.unwrap();
}

/// 半开区间：以已存在的 `seq` 作 `afterSeq`，返回集**不含**该锚点，且全部严格大于它。
#[tokio::test]
async fn replay_interval_is_half_open_excluding_after_seq() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "anchor").await;
    send(&g, &id, "one").await;
    send(&g, &id, "two").await;

    // 取一个「中间」锚点：既不是 0（区间左界），也不是最大 seq（避免退化成空页）。
    let all = seqs(&replay(&g, json!({"id": id, "afterSeq": 0})).await);
    assert!(all.len() >= 3, "需要足够事件才能取到中间锚点: {all:?}");
    let anchor = all[1];

    let tail = replay(&g, json!({"id": id, "afterSeq": anchor})).await;
    assert_eq!(tail["code"], 0, "回放必须成功: {tail}");
    let tail_seqs = seqs(&tail);
    assert!(
        !tail_seqs.contains(&anchor),
        "afterSeq 本身必须落在开区间外（不含 {anchor}）: {tail_seqs:?}"
    );
    assert!(
        tail_seqs.iter().all(|&s| s > anchor),
        "所有返回事件必须严格大于 afterSeq={anchor}: {tail_seqs:?}"
    );
    // 半开区间下：返回集 == 全量里大于锚点的那一段（无丢无重）。
    let expected: Vec<u64> = all.iter().copied().filter(|&s| s > anchor).collect();
    assert_eq!(
        tail_seqs, expected,
        "开区间切片应恰好是全量的后缀: {tail_seqs:?}"
    );

    g.shutdown().await.unwrap();
}

/// 空页时 `lastSeq == afterSeq`：`afterSeq` 取当前最大 seq，`events` 为空。
///
/// 客户端据此判定「已追平、可终止增量拉取」。
#[tokio::test]
async fn empty_page_echoes_after_seq_back_as_last_seq() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "caught-up").await;
    send(&g, &id, "hi").await;

    let full = replay(&g, json!({"id": id, "afterSeq": 0})).await;
    let max = full["data"]["lastSeq"].as_u64().unwrap();

    let env = replay(&g, json!({"id": id, "afterSeq": max})).await;
    assert_eq!(env["code"], 0, "追平回放仍成功: {env}");
    assert!(seqs(&env).is_empty(), "锚点已是最大 seq，应返回空页: {env}");
    assert_eq!(
        env["data"]["lastSeq"].as_u64().unwrap(),
        max,
        "空页 lastSeq 必须回落为传入的 afterSeq: {env}"
    );

    g.shutdown().await.unwrap();
}

/// `limit` 夹取下界：`limit=0` 被夹到 `1`，至多（且恰，因事件足够）返回 1 条，不报错。
#[tokio::test]
async fn limit_zero_is_clamped_to_one_not_rejected() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "limit-zero").await;
    send(&g, &id, "hi").await;

    let env = replay(&g, json!({"id": id, "afterSeq": 0, "limit": 0})).await;
    assert_eq!(env["code"], 0, "limit=0 不得被拒/panic: {env}");
    let seqs = seqs(&env);
    assert!(seqs.len() <= 1, "limit=0 夹到 1，至多一条: {env}");
    assert_eq!(seqs.len(), 1, "存在多条事件时，夹到 1 应恰返回一条: {env}");
    // 夹取是「取最小的 limit 条」，应从区间左端起。
    assert_eq!(seqs[0], 1, "应返回区间内最小 seq（即 1）: {seqs:?}");

    g.shutdown().await.unwrap();
}

/// `limit` 夹取上界：给超大 `limit` 也不超过 `REPLAY_MAX_PAGE`，且钳制**实际生效**。
///
/// 为让断言非空转，这里造出**多于** `REPLAY_MAX_PAGE` 条事件，再断言恰好返回一页。
#[tokio::test]
async fn huge_limit_is_capped_at_page_max() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "big").await;
    // 建会话 1 条，每次 send 4 条；send 次数取到总事件数 > REPLAY_MAX_PAGE。
    let sends = REPLAY_MAX_PAGE / 4 + 5;
    for i in 0..sends {
        send(&g, &id, &format!("m{i}")).await;
    }

    let env = replay(&g, json!({"id": id, "afterSeq": 0, "limit": u64::MAX / 2})).await;
    assert_eq!(env["code"], 0, "超大 limit 不得报错: {env}");
    let seqs = seqs(&env);
    assert!(
        seqs.len() <= REPLAY_MAX_PAGE,
        "返回不得超过页上限 {REPLAY_MAX_PAGE}: {} 条",
        seqs.len()
    );
    assert_eq!(
        seqs.len(),
        REPLAY_MAX_PAGE,
        "事件数已超过页上限，钳制必须实际生效（应恰为一页）"
    );
    assert_eq!(
        env["data"]["lastSeq"].as_u64().unwrap(),
        REPLAY_MAX_PAGE as u64,
        "一页的 lastSeq 即页内最大 seq: {env}"
    );

    g.shutdown().await.unwrap();
}

/// 成功响应 `data` 恰含 `REPLAY_DATA_KEYS` 这两个键（`events` / `lastSeq`）。
#[tokio::test]
async fn success_data_has_frozen_replay_keys() {
    let (g, _dir) = start_gateway().await;
    let id = create_session(&g, "keys").await;

    let env = replay(&g, json!({"id": id, "afterSeq": 0})).await;
    assert_eq!(env["code"], 0, "回放必须成功: {env}");
    let data = env["data"].as_object().expect("data 必须是对象");
    for key in REPLAY_DATA_KEYS {
        assert!(data.contains_key(*key), "data 缺冻结键 {key}: {env}");
    }
    assert_eq!(
        data.len(),
        REPLAY_DATA_KEYS.len(),
        "data 键集合应与冻结键一致，不得多键: {env}"
    );

    g.shutdown().await.unwrap();
}

/// 未知会话回放：与 `session:get/send/stop` 同口径，**信封 `404`**（不是空页）。
///
/// 网关 `own()` 先做存在性 + 归属校验（`agent-gateway/src/rpc.rs:66`），
/// 未知 `id` 落 `SessionNotFound` → `core()` 映射为 `404 "not found"`，
/// **早于** `agent-core` 层 `replay()` 的「未知会话 404 Vec」分支。
/// 既有回归锚 `agent-gateway/tests/replay_rpc.rs::replay_of_unknown_session_is_not_found`
/// 断言同一行为；本条与之对齐，防止两侧口径漂移。
#[tokio::test]
async fn replay_of_unknown_session_is_not_found() {
    let (g, _dir) = start_gateway().await;
    // 合法 v4 形态、但绝未创建过的 id：不引入 `uuid` 依赖（`Cargo.toml`/`Cargo.lock`
    // 属共享区不可改），用固定的合法字面量；网关只要求 `Uuid::parse_str` 成功。
    let unknown = "00000000-0000-4000-8000-000000000001";

    let env = replay(&g, json!({"id": unknown, "afterSeq": 0})).await;
    assert_eq!(
        env["code"], 404,
        "未知会话应先被 `own()` 的存在性校验拦下，返 404 而非空页: {env}"
    );
    assert!(
        env["data"].is_null(),
        "错误信封的 `data` 应为 null（不回显内部状态）: {env}"
    );
    assert_eq!(
        env["msg"], "not found",
        "错误文案应与 `session:get/send/stop` 的 `own()` 口径一致: {env}"
    );

    g.shutdown().await.unwrap();
}

/// 常量自检：`REPLAY_MAX_PAGE` 必须与 `agent-core` 的 `DEFAULT_PAGE_LIMIT` 逐值相等。
///
/// core 侧调大/调小页上限时，本 crate 的冻结值必须同批跟改，否则这条先红。
#[test]
fn frozen_replay_max_page_matches_core_page_limit() {
    assert_eq!(REPLAY_MAX_PAGE, agent_core::event_log::DEFAULT_PAGE_LIMIT);
    assert_eq!(
        REPLAY_MAX_PAGE, 500,
        "分页上限冻结值被改动，须同步 core 与文档"
    );
}
