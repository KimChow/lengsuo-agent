//! v0 动作面（`域:动作`）清单的**权威来源**（W2 · lane proto）。
//!
//! # 为什么清单落在 `agent-gateway`
//!
//! 这份清单描述的是**网关 `/rpc` 路由面**——`POST /rpc` 的 body 字段 `action`
//! 允许的**全部** `域:动作` 名字，也就是 `crates/agent-gateway/src/rpc.rs::run`
//! 里 `match action` 的**全部合法分支**。清单与实现同属一个 crate，才谈得上
//! 「清单即实现的镜像」。
//!
//! 放在 `contracts-check` 那侧时方向是 `contracts-check → agent-gateway`
//! （`contracts-check` 以 `agent-gateway` 为 dev-dependency），反向引用会成
//! **依赖环**，于是历史上只能另抄一份**手写副本**——而手写副本会漂移（本卡
//! 起因：`session:cancel` 补登记进契约时，副本没跟上）。把清单**下沉**到实现侧，
//! 副本随之消失，且**不需要改动任何 `Cargo.toml`**。
//!
//! # 两份常量的区别
//!
//! - [`BASELINE_ACTIONS`]：`AGENT-KICKOFF.md` §2 第 24 行**首版冻结**的 11 项
//!   （`session:replay` 为 2026-10-19 按「只增不改」追加，同见该行）。它是
//!   「首版冻结了什么」的锚点，用来守**动作面不缩水**。
//! - [`V0_ACTIONS`]：当前 v0 动作面的**全集** = 基线 + 后续按「只增不改」补
//!   登记的追加项（当前第 12 项 `session:cancel` 来自提交 `08b9f40`，并经
//!   `docs/contracts/acp-method-ledger.md` §1 补登记）。
//!
//! # 消费方
//!
//! - `crates/agent-gateway/tests/acp_forward_whitelist.rs`：白名单 ⊆ 动作面
//!   （防自创名）、遍历全体动作面。
//! - `crates/contracts-check/tests/action_surface.rs` / `method_ledger.rs`：
//!   动作面清单与 `contracts_check::ACTIONS`（镜像）的**逐项相等**断言。
//!
//! # 演进口径
//!
//! **不要**在清单里锁死数量（`len() == N` 之类的断言）：新增动作是「只增不改」
//! 下的正当演进，锁死数量会让每次正当扩充必红。要守的是**关系**——基线 ⊆ 全集、
//! 全集无重复、镜像与全集相等。

/// `AGENT-KICKOFF.md` §2 第 24 行**首版冻结**的 v0 动作面（11 项）。
///
/// 这份基线是「只增不改」的**下界**：后续动作面**只能扩、不能缩**。
///
/// - `session:replay` 为 2026-10-19 按「只增不改」追加（同见该行）。
/// - 本清单**不含**后续补登记的追加项（如 `session:cancel`）；那类项在
///   [`V0_ACTIONS`] 里。
pub const BASELINE_ACTIONS: &[&str] = &[
    // 行 24：session:list（会话列表）
    "session:list",
    // 行 24：session:create（创建会话）
    "session:create",
    // 行 24：session:get（读取单个会话）
    "session:get",
    // 行 24：session:send（发送用户输入）
    "session:send",
    // 行 24：session:stop（停止会话）
    "session:stop",
    // 行 24：session:replay（事件回放；payload {id, afterSeq, limit?}）
    "session:replay",
    // 行 24：approval:respond（审批应答）
    "approval:respond",
    // 行 24：config:get（读配置）
    "config:get",
    // 行 24：config:set（写配置）
    "config:set",
    // 行 24：tool:list（工具清单）
    "tool:list",
    // 行 24：sys:ping（健康检查）
    "sys:ping",
];

/// 当前 v0 动作面**全集**（基线 + 后续补登记的追加项）。
///
/// 本清单是**权威来源**：`crates/contracts-check/src/lib.rs::ACTIONS` 只是它的
/// **镜像**（`agent-gateway` 在 `contracts-check` 侧是 dev-dependency，lib 目标
/// 用不上，故镜像无法改成 `pub use`），两者由
/// `crates/contracts-check/tests/method_ledger.rs` 的**双向差集相等断言**守住。
///
/// 台账：`docs/contracts/acp-method-ledger.md` §1（逐项语义与映射判定）。
pub const V0_ACTIONS: &[&str] = &[
    // 行 24：session:list（会话列表）
    "session:list",
    // 行 24：session:create（创建会话）
    "session:create",
    // 行 24：session:get（读取单个会话）
    "session:get",
    // 行 24：session:send（发送用户输入）
    "session:send",
    // 行 24：session:stop（停止会话）
    "session:stop",
    // 行 24：session:replay（事件回放；payload {id, afterSeq, limit?}）
    "session:replay",
    // 08b9f40 新增（台账 docs/contracts/acp-method-ledger.md §1 补登记）：
    // session:cancel（取消当前回合，fire-and-forget，立即回快照）
    "session:cancel",
    // 行 24：approval:respond（审批应答）
    "approval:respond",
    // 行 24：config:get（读配置）
    "config:get",
    // 行 24：config:set（写配置）
    "config:set",
    // 行 24：tool:list（工具清单）
    "tool:list",
    // 行 24：sys:ping（健康检查）
    "sys:ping",
];
