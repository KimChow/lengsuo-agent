//! 契约验证 crate：把 `docs/contracts/` 与 `AGENT-KICKOFF.md` 的冻结条款
//! 变成可执行断言。本 crate 不参与生产，只被 `tests/` 使用。
//!
//! 本模块只放「冻结常量的单一来源」：动作面、事件面、响应码段。测试从这些常量
//! 出发做穷举断言，任何一侧偷偷加/删一项都会在 `tests/` 里红掉，而不是等到
//! 两轨合流时才发现「文档写着、代码各写各的」。
//!
//! 真相来源：`AGENT-KICKOFF.md` §2（本地网关协议 v0 冻结版）。
#![forbid(unsafe_code)]

/// v0 全部动作名（`<域>:<动作>`），共 12 个（11 个冻结 + 1 个追加）。
///
/// 前 11 项逐项对应 `AGENT-KICKOFF.md` 第 24 行「v0 动作面」的冻结清单：
/// `session:list/create/get/send/stop/replay`、`approval:respond`、
/// `config:get/set`、`tool:list`、`sys:ping`
/// （`session:replay` 为 2026-10-19「只增不改」追加，同见该行）。
/// 第 12 项 `session:cancel` **不在**第 24 行内，是 `08b9f40` 新增、经
/// `docs/contracts/acp-method-ledger.md` §1 补登记的追加项。
///
/// **本常量是「镜像」，权威来源在 `agent-gateway`**：
/// `crates/agent-gateway/src/actions.rs::V0_ACTIONS`（该 crate 正是实现
/// `域:动作` 路由的地方，清单与实现同侧才谈得上「清单即实现的镜像」）。
/// 之所以不能直接 `pub use agent_gateway::V0_ACTIONS as ACTIONS`：
/// `agent-gateway` 在本 crate 是 **dev-dependency**，而本文件是 **lib 目标**
/// ——dev-dependency 对 lib 目标不可用，而 `Cargo.toml` 属共享区（本卡不改）。
/// 故此处保留字面量镜像，由
/// `crates/contracts-check/tests/method_ledger.rs::contract_actions_mirror_the_authoritative_list`
/// 的**双向差集相等断言**守住：镜像与权威清单必须逐项相等，否则红。
pub const ACTIONS: &[&str] = &[
    "session:list",   // 行 24：session:list（会话列表）
    "session:create", // 行 24：session:create（创建会话）
    "session:get",    // 行 24：session:get（读取单个会话）
    "session:send",   // 行 24：session:send（发送用户输入）
    "session:stop",   // 行 24：session:stop（停止会话）
    // 08b9f40 新增（非行 24 冻结项；台账 docs/contracts/acp-method-ledger.md §1）：
    // session:cancel（取消当前回合，fire-and-forget，立即回快照）
    "session:cancel",
    "session:replay", // 行 24：session:replay（事件回放；payload {id, afterSeq, limit?}）
    "approval:respond", // 行 24：approval:respond（审批应答）
    "config:get",     // 行 24：config:get（读配置）
    "config:set",     // 行 24：config:set（写配置）
    "tool:list",      // 行 24：tool:list（工具清单）
    "sys:ping",       // 行 24：sys:ping（健康检查）
];

/// ACP 标准方法名（斜杠式），共 13 项——与 `agent-acp` 的 `method` 模块
/// **逐项对齐**。
///
/// 真相来源：`crates/agent-acp/src/methods.rs` 的 `pub mod method`（13 个
/// `pub const`，见 `docs/contracts/acp-method-ledger.md` §2）。命名按 ACP 0.25.0
/// 斜杠写法；与 v0 动作面（冒号式 [`ACTIONS`]）**不是**同一套命名——
/// 本常量只登记方法名集合，**不**声明任何映射关系。
///
/// **为什么需要它**：网关侧 `crates/agent-gateway/src/acp.rs::forward_action`
/// 把动作名**原样**当 ACP method 投；`docs/contracts/acp-host.md` §7 决策项 2
/// 要求「其余动作的 `域:动作` → ACP method 对应关系」先有台账。本常量是该台账
/// （`docs/contracts/acp-method-ledger.md`）的对端清单锚点：
/// `tests/method_ledger.rs` 用**源码文本扫描**从 `methods.rs` 抽字面量，断言其
/// 集合与本常量**相等**——一侧加方法、另一侧漏登记即红。
///
/// 不要在这里删项：本常量是 ACP 方法面的单一冻结来源，删项会让上述相等断言
/// 静默少跑一圈。
pub const ACP_METHODS: &[&str] = &[
    "initialize",                 // agent-acp method::INITIALIZE
    "authenticate",               // agent-acp method::AUTHENTICATE
    "session/new",                // agent-acp method::SESSION_NEW
    "session/load",               // agent-acp method::SESSION_LOAD
    "session/resume",             // agent-acp method::SESSION_RESUME
    "session/prompt",             // agent-acp method::SESSION_PROMPT
    "session/cancel",             // agent-acp method::SESSION_CANCEL（通知）
    "session/update",             // agent-acp method::SESSION_UPDATE（通知）
    "session/set_mode",           // agent-acp method::SESSION_SET_MODE
    "session/request_permission", // agent-acp method::SESSION_REQUEST_PERMISSION
    "logout",                     // agent-acp method::LOGOUT
    "elicitation/create",         // agent-acp method::ELICITATION_CREATE
    "elicitation/complete",       // agent-acp method::ELICITATION_COMPLETE
];

/// ACP **通知方法面**（单向、无响应）——[`ACP_METHODS`] 的**子集**。
///
/// ACP 方法分两种**帧形状**（JSON-RPC 2.0）：**请求-响应**
/// （`Message::Request`，发出去必须配一个 `Response` 回来）与**通知**
/// （`Message::Notification`，发出去**永不配响应**，对端不应答）。本常量
/// 登记的是**第二种**。真相来源是 `agent-acp` 的 `method` 模块里**常量行
/// 上一行的 `///` 文档注释**带「（通知）」的那几项：
///
/// | 方法 | 常量 | 常量行 | 「（通知）」散文锚点 |
/// |---|---|---|---|
/// | `session/cancel` | `method::SESSION_CANCEL` | `methods.rs:368` | `methods.rs:367` |
/// | `session/update` | `method::SESSION_UPDATE` | `methods.rs:370` | `methods.rs:369` |
///
/// # 为什么必须结构化登记（此前只有散文）
///
/// 「通知」这个语义属性此前**只由三个汉字承载**（`（通知）`）：实现侧
/// `grep -rn "NOTIFICATION\|NOTIFY" crates/agent-acp/src/methods.rs` 零命中
/// （没有通知族清单常量），契约侧 [`ACP_METHODS`] 里那两行也只是**行尾散文
/// 注释**。散文被改写/挪位/重排就**静默丢失**，没有任何机器可判定的断言。
///
/// # 为什么它是**承重**语义
///
/// 网关的 ACP 转发机制是**请求-响应**（`crates/agent-gateway/src/acp.rs` 的
/// `forward_action` 发 Rpc 请求帧、按 id 等响应帧配对）。把**通知**方法投进
/// 那条路径 ⇒ 对端永不应答 ⇒ 本端等到超时 ⇒ 用户拿 `502 "acp forwarding
/// failed"`，而操作**实际已在对端发生** ⇒ **必然的错误行为**。
/// `session:cancel` 的终局裁定（台账 `docs/contracts/acp-method-ledger.md` §5）
/// 正是基于这条推理。
///
/// # 断言口径（`tests/acp_notification_surface.rs`，**只守关系、不锁数量**）
///
/// 源码文本扫描 `methods.rs`（常量行**上一行**的 `///` 注释含「通知」⇒ 视为
/// 通知方法）断言：**实现面 ⊆ 登记面**、**登记面 ⊆ 实现面**、**通知族 ⊆
/// [`ACP_METHODS`]**、**通知族 ∩ [`ACP_FORWARDED_ACTIONS`] == ∅**、**通知族 ∩
/// [`ACP_FORWARD_WHITELIST_SNAPSHOT`] == ∅**（通知方法**不得**进转发面）、
/// **逐项锚点仍在**。
///
/// ⚠️ **不锁数量/取值**：下一张卡正当新增一个通知方法时，把新方法登记进本
/// 常量即可；任何 `assert_eq!(ACP_NOTIFICATION_METHODS.len(), 2)` 之类的断言
/// 都会在正当扩充时必红（教训：`tests/method_ledger.rs` 的
/// `assert_eq!(ACP_METHODS.len(), 13)`）。
pub const ACP_NOTIFICATION_METHODS: &[&str] = &[
    // agent-acp method::SESSION_CANCEL（methods.rs:368；「（通知）」锚点 methods.rs:367）
    "session/cancel",
    // agent-acp method::SESSION_UPDATE（methods.rs:370；「（通知）」锚点 methods.rs:369）
    "session/update",
];

/// 网关 ACP 转发白名单的**当前快照**（本卡：**快照**，**非不变量**）。
///
/// 真相来源：`crates/agent-gateway/src/rpc.rs::ACP_FORWARD_ACTIONS`，本卡取值
/// `["sys:ping"]`（唯一成员，v0 动作**原名**，非自创名）。
///
/// ⚠️ **这是快照，不是不变量**：下一张卡正当扩充白名单时，本常量**要同步更新**；
/// `tests/method_ledger.rs` **不得**据此锁死「白名单恰好几个成员」——那样会在
/// 合法扩充时必红。本常量只用于守**关系不变量**：
/// `ACP_FORWARD_WHITELIST_SNAPSHOT ⊆ ACTIONS`（防自创名）。
///
/// 本卡**不新增**白名单成员（见 `docs/contracts/acp-method-ledger.md` §5）。
pub const ACP_FORWARD_WHITELIST_SNAPSHOT: &[&str] = &["sys:ping"];

// ---- 动作分派面：`/rpc` 动作 → 应答路径（台账 `acp-method-ledger.md` §7） ----
//
// 分派顺序：`rpc.rs::run`（130）第一句 `acp_forward(...)?`（113）命中即返回，否则落到
// `match action`（131）本地实现。下列常量皆**快照**，只守 `tests/action_dispatch_surface.rs`
// 的关系，不得锁数量或取值。

/// 走 ACP 旁路转发的动作——`rpc.rs:43` `ACP_FORWARD_ACTIONS` 的镜像快照。
/// 同 [`ACP_FORWARD_WHITELIST_SNAPSHOT`]（后者只守 ⊆ `ACTIONS`）；本常量另守
/// 「源码白名单 == 镜像」「与 `ACP_METHODS` 不相交」。
pub const ACP_FORWARDED_ACTIONS: &[&str] = &["sys:ping"]; // rpc.rs:43 `ACP_FORWARD_ACTIONS`

/// 同时在「本地实现」与「ACP 旁路」两侧都有实现的动作（**双路径动作**）。
/// 挂 `acp` 时由 host 应答、未挂时由 `run()` 本地区应答；两侧语义**不同**（`sys:ping`
/// 分别回 host pid / 网关 pid），故成员须在 [`LOCAL_DISPATCH_ACTIONS`] 与
/// [`ACP_FORWARDED_ACTIONS`] 里**都在**——本地那份是未挂 `acp` 时的退路，
/// 删掉即让该部署形态静默回落 `-32601`。
pub const DUAL_PATH_ACTIONS: &[&str] = &["sys:ping"];

/// **本地应答、禁入 ACP 转发白名单**的动作清单（台账 `acp-method-ledger.md` §5 裁定）。
///
/// 转发机制是「**请求-响应**」（`forward_action` 发 Rpc 请求帧、等响应帧配对），而
/// 清单里这些动作的对端 ACP 方法是**通知、无响应**（`session/cancel`，见
/// `agent-acp/src/methods.rs::method::SESSION_CANCEL`）——原样转发 ⇒ 对端永不应答
/// ⇒ 本端超时 ⇒ 用户拿 `502 "acp forwarding failed"`，而取消实际已在对端发生。
/// 次要理由：两侧会话 id 命名空间独立（本端 `Uuid` ↔ 对端 ACP 会话 id），无映射表。
///
/// ⇒ 裁定：`session:cancel` **保持本地实现**（fire-and-forget，立即回快照），**不得**
/// 进 [`ACP_FORWARDED_ACTIONS`] / [`ACP_FORWARD_WHITELIST_SNAPSHOT`] / [`DUAL_PATH_ACTIONS`]。
/// 断言：`tests/cancel_answer_surface.rs`（不相交 + 本地分支存在 + 对端「通知」锚点）。
pub const LOCAL_ONLY_ANSWER_ACTIONS: &[&str] = &["session:cancel"];

/// `rpc.rs::run()` 里 `match action`（131）本地实现覆盖的动作，源码扫描的比对基准；
/// 与 [`ACTIONS`] **集合相等**（`tests/action_dispatch_surface.rs`）。
pub const LOCAL_DISPATCH_ACTIONS: &[&str] = &[
    "sys:ping",         // rpc.rs:132
    "config:get",       // rpc.rs:133
    "config:set",       // rpc.rs:141
    "session:list",     // rpc.rs:149
    "session:create",   // rpc.rs:150
    "session:get",      // rpc.rs:186
    "session:send",     // rpc.rs:187
    "session:stop",     // rpc.rs:199
    "session:cancel",   // rpc.rs:205
    "session:replay",   // rpc.rs:207
    "tool:list",        // rpc.rs:222
    "approval:respond", // rpc.rs:250
];

/// ACP 转发**传播超时**的默认秒数（转发档，**必须小于**网关 RPC 超时档）。
///
/// # 真相来源
///
/// `crates/agent-gateway/src/acp.rs` 的
/// `const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);`
/// （**私有 const、非 `pub`** ⇒ 契约层拿不到，只能扫源码文本读取）。
///
/// # 两档超时的关系（**承重**）
///
/// | 档 | 值 | 来源 | 超时后的信封码 |
/// |---|---|---|---|
/// | 转发传播超时 | **5s** | `agent-gateway/src/acp.rs` `FORWARD_TIMEOUT` | `502 "acp forwarding failed"` |
/// | 网关 RPC 超时 | **30s**（默认） | `agent-core/src/config.rs` `DEFAULT_RPC_TIMEOUT_MS` | `408 "request timed out"` |
///
/// 网关 `/rpc` 的**外层** `timeout` 用的是 RPC 超时档（`rpc.rs` 的 `handle`），
/// 而 ACP 转发在**内层**先撞自己的传播超时：转发失败被映射成 `502`
/// （`rpc.rs` 的 `acp_forward`）。⇒ **正常情况下对端不应答 ⇒ 先撞 5s ⇒ 出 502，
/// 外层 30s 永不命中转发路径**。
///
/// ⇒ **这条「5s < 30s」的大小关系是承重的**：一旦 `FORWARD_TIMEOUT` ≥ RPC 超时，
/// 外层 30s 会**先**到 ⇒ 转发路径被外层 `timeout` 砍掉 ⇒ 用户拿到
/// **`408 "request timed out"`** 而不是 `502` ⇒ **错误码退化**（408 的语义是
/// 「整个 RPC 没做完」，不是「ACP 对端不应答」）⇒ **误导性错误码**。
///
/// # 断言口径
///
/// `tests/acp_timeout_tier.rs`：**实现值 == 登记值**（扫 `acp.rs` 文本解析
/// `FORWARD_TIMEOUT` 的秒数）、**转发档 < RPC 默认档**（**两边都扫出来**：左边是
/// `acp.rs` 的 `FORWARD_TIMEOUT`、右边是 `agent-core/src/config.rs` 的
/// `DEFAULT_RPC_TIMEOUT_MS`，都不是硬编码 ⇒ 改任一边的实现都会红）、防空扫描。
///
/// # ⚠️ 已知限制（**可配行为，不是可断言的不变量**）
///
/// `rpc_timeout_ms` **可被用户配置改小**（`config.rs` 的 `"rpcTimeoutMs"` ⇒
/// `positive_ms`）⇒ 若被配到 **≤ 转发超时**（例如 `3000`），转发路径就会被外层
/// `timeout` 砍掉 ⇒ **错误码同样退化为 408（非 502）**。
/// ⇒ **调用方须把 `rpcTimeoutMs` 配得大于转发超时**；本常量守的是**默认值下的**
/// 关系，用户改小后**不再成立**（那是部署配置的责任，不写成断言）。
pub const ACP_FORWARD_TIMEOUT_SECS: u64 = 5;

/// v0 全部事件 kind，共 11 项——与 `agent-core::schema::EventKind` **逐项对齐**。
///
/// 真相来源：`agent-core` `schema.rs` 的 `enum EventKind`（10 个变体：
/// `thinking / text / tool_use / tool_result / approval_request / approval_resolved /
/// files / done / error / session_update`），加上 v0 实现里**持久化在事件日志中**的
/// `user` 一类，共 11 项。
///
/// `AGENT-KICKOFF.md` 第 23 行只按「参考客户端 基线 §5」抄了七类再加 `session_update`，
/// 实现里稳定存在的三类**未写进该行枚举句**，按「文档缺口（实现为准）」登记：
/// - `user`：`runtime/turns.rs` 落盘 `"user"`，`lengsuo-shell`
///   `state/stream/events.rs` 实现了该分支；
/// - `approval_resolved` / `error`：`agent-core` `EventKind` 有对应变体、`as_str()`/`parse()`
///   都给名字，网关侧 `agent-gateway` `server.rs` 把 `event.kind` 当**任意字符串**原样透传。
///
/// 不要在这里删掉任何一项：本常量是事件面的单一冻结来源，删项会让
/// `tests/event_surface.rs` 的穷举断言静默少跑一圈。
pub const EVENT_KINDS: &[&str] = &[
    "thinking",          // agent-core EventKind::Thinking
    "text",              // agent-core EventKind::Text
    "tool_use",          // agent-core EventKind::ToolUse
    "tool_result",       // agent-core EventKind::ToolResult
    "approval_request",  // agent-core EventKind::ApprovalRequest
    "approval_resolved", // agent-core EventKind::ApprovalResolved（第 23 行缺席，见常量文档注释）
    "files",             // agent-core EventKind::Files
    "done",              // agent-core EventKind::Done
    "error",             // agent-core EventKind::Error（第 23 行缺席，见常量文档注释）
    "session_update",    // agent-core EventKind::SessionUpdate
    "user", // 持久化事件 `user`（非 `agent-core::schema::EventKind` 变体，见常量文档注释）
];

/// `AGENT-KICKOFF.md` 第 22 行「信封」**逐字冻结**的响应码（4 项）。
///
/// 原文（第 22 行）：
/// > 信封 `{code, msg, data}`（`code==0` 成功；404 未注册动作、403 鉴权拒绝、
/// > 408 超时；其余按 server-contract 错误码段）
///
/// - `0`   成功（`code==0` 即成功，`msg` 为 `ok`）
/// - `403` 鉴权拒绝（token 缺失/错误；`agent-gateway` 中间件在路由之前返回）
/// - `404` 未注册动作（「其余动作 404，不预留半成品」）
/// - `408` 超时（默认 30s，`rpc_timeout_ms` 可调）
///
/// **这 4 项是「冻结子集」**：它们必须**逐项在** [`ENVELOPE_CODES`] 里
/// （不得缩水）。[`ENVELOPE_CODES`] 另有实现侧额外发出的码，故两者**不是**相等关系。
pub const FROZEN_ENVELOPE_CODES: &[u16] = &[0, 403, 404, 408];

/// 本仓实现**实际会发出**的全部响应码（8 项）。
///
/// = 第 22 行**冻结**的 4 码（见 [`FROZEN_ENVELOPE_CODES`]）
/// + 实现**额外发出**的 4 码（400 / 409 / 500 / 502）。
///
/// # 为什么不是「只列冻结 4 码」
///
/// 第 22 行末尾写着「**其余按 server-contract 错误码段**」，而那一段**不在本仓**。
/// 本常量登记的**不是** server-contract 段的全文（拿不到、也不该臆造），
/// 而是**本仓 `agent-gateway` 实现侧真实会发出的码**——契约侧的登记必须
/// **覆盖实现面**，否则实现发了未登记的码，客户端无从判断「合法码 or 协议漂移」。
///
/// 每项的来源（`agent-gateway` 源码行号，随实现演进会变，语义以注解为准）：
///
/// | 码 | 语义 | 来源（`agent-gateway/src/`） | 归属 |
/// |---|---|---|---|
/// | `0`   | 成功（`msg` = `ok`） | `rpc.rs:71` `envelope(0, "ok", data)` | §2 第 22 行冻结 |
/// | `400` | 请求体非 JSON / payload 校验失败 / 配置非法 | `rpc.rs:56` `invalid JSON`、`rpc.rs:77` `invalid payload`、`rpc.rs:82` `invalid configuration` | 实现额外发出 |
/// | `403` | 鉴权拒绝 / 归属越权 / 服务端绑定身份 | `rpc.rs:99` `forbidden`、`rpc.rs:156` `server-bound identity`、`rpc.rs:259` `forbidden`、`server.rs:196` 鉴权中间件 | §2 第 22 行冻结 |
/// | `404` | 未注册动作 / 会话或审批不存在 / 未知路由 | `rpc.rs:81` `not found`、`rpc.rs:276` `unknown action`、`server.rs:88` `unknown route` | §2 第 22 行冻结 |
/// | `408` | RPC 超时（默认 30s） | `rpc.rs:63` `request timed out` | §2 第 22 行冻结 |
/// | `409` | 会话已结束、拒绝新输入 | `rpc.rs:83` `session closed`（`CoreError::SessionClosed`） | 实现额外发出 |
/// | `500` | 内部错误兜底 | `rpc.rs:84` `operation failed`（`CoreError` 兜底分支）、`rpc.rs:62` `request failed`（阻塞任务 join 失败） | 实现额外发出 |
/// | `502` | ACP 转发传输失败 | `rpc.rs:123` `acp forwarding failed` | 实现额外发出 |
///
/// # 断言口径（关系，不锁数量/取值）
///
/// - `FROZEN_ENVELOPE_CODES ⊆ ENVELOPE_CODES`（冻结面不得缩水）；
/// - `ENVELOPE_CODES` 内部**无重复**；
/// - 实现源码里扫出的码字面量集合 `⊆ ENVELOPE_CODES`（见
///   `tests/envelope_code_surface.rs`）。
///
/// ⚠️ **不要在这里锁死「恰好几个码」**：实现正当新增错误码时，先把新码登记进
/// 本常量（并同步 `docs/contracts/shell-agent.md` §2），本常量随之变宽即可；
/// 任何「数量/取值相等」的断言都会在正当扩充时必红。
pub const ENVELOPE_CODES: &[u16] = &[
    0,   // 冻结：成功（rpc.rs:71）
    400, // 额外：请求体/payload 非法（rpc.rs:56,77,82）
    403, // 冻结：鉴权拒绝（rpc.rs:99,156,259 / server.rs:196）
    404, // 冻结：未注册动作（rpc.rs:81,276 / server.rs:88）
    408, // 冻结：超时（rpc.rs:63）
    409, // 额外：会话已结束（rpc.rs:83）
    500, // 额外：内部错误兜底（rpc.rs:62,84）
    502, // 额外：ACP 转发传输失败（rpc.rs:123）
];

// ---- ACP wire 错误码面（`agent-acp` 的 JSON-RPC **负数**族） ----
//
// ⛔ 这是**另一套命名空间**，与上面的 [`ENVELOPE_CODES`]（网关信封层 `u16`）**不是**
// 同一族、⛔ **不可混表**：
// - 信封层码：**非负 u16**，出现在信封**顶层 `code`**（`{code, msg, data}`）；
// - ACP wire 码：**负 i64**，只出现在 **`data.error.code`**。
//
// 两套码**会同时出现在同一个响应体里**：`crates/agent-gateway/src/rpc.rs` 的
// `acp_forward()` 在「转发成功但对端回的是 ACP 错误」时，把对端的
// `{"error":{…}}` 原样塞进信封的 `data` 字段，而信封**顶层 `code` 仍是 `0`**
// （转发本身送达并拿到应答）。既有回归锚：
// `crates/agent-gateway/tests/acp_forward_whitelist.rs`（断言 `resp["code"] == 0`
// 且 `resp["data"]["error"]["code"] == -32601`）。
//
// ⚠️ **标准族与私有族是两层**（`crates/agent-acp/src/message.rs:61` 的注释：
// 「错误码：标准族见 `JSONRPC_ERROR_*` 常量，**私有码原样保留**」）⇒ 登记成
// **两个常量**，不合并成一族。
//
// 真相来源：`crates/agent-acp/src/{message,router,host_router}.rs` 的
// `pub const …: i64` 字面量。断言：`tests/acp_error_surface.rs`（源码文本扫描，
// 双向差集 + 两族不相交 + 族内去重 + 与信封层不串味，**只守关系、不锁数量**）。

/// ACP **标准族** wire 错误码（JSON-RPC 2.0 标准区，当前 4 项）。
///
/// 真相来源：`crates/agent-acp/src/message.rs` 的四个 `JSONRPC_ERROR_*` 常量。
///
/// | 码 | 常量 | 来源 | 语义 |
/// |---|---|---|---|
/// | `-32700` | `JSONRPC_ERROR_PARSE` | `message.rs:71` | 解析失败 |
/// | `-32601` | `JSONRPC_ERROR_METHOD_NOT_FOUND` | `message.rs:73` | 方法不存在（host 侧自动回，防对端悬挂） |
/// | `-32600` | `JSONRPC_ERROR_INVALID_REQUEST` | `message.rs:75` | 无效请求 |
/// | `-32602` | `JSONRPC_ERROR_INVALID_PARAMS` | `message.rs:77` | 参数无效（如 `initialize` 参数解析失败） |
///
/// ⚠️ **不锁数量**：实现正当新增一个标准码时，把新码登记进本常量即可；「恰好
/// 4 项」之类的断言会在正当扩充时必红（教训：`tests/method_ledger.rs` 的
/// `assert_eq!(ACP_METHODS.len(), 13)`）。
pub const ACP_STANDARD_ERROR_CODES: &[i64] = &[-32700, -32601, -32600, -32602];

/// ACP **私有族** wire 错误码（非标准区，当前 2 个**码值**）。
///
/// 真相来源：`crates/agent-acp/src/router.rs` 的两个私有码常量。
///
/// | 码 | 常量 | 来源 | 语义 |
/// |---|---|---|---|
/// | `-32000` | `ACP_ERROR_CONNECTION_CLOSED` | `router.rs:40` | 连接终止（撤下 pending） |
/// | `-32001` | `ACP_ERROR_INTERNAL` | `router.rs:42` | 内部协议违例（如对端响应既无 result 也无 error） |
///
/// ⚠️ **按「码值」去重登记**：`router.rs:40` 的 `ACP_ERROR_CONNECTION_CLOSED` 与
/// `host_router.rs:278` 的别名 `ACP_ERROR_HOST_CONNECTION_CLOSED` **同值**
/// （`-32000`；别名写成 `= ACP_ERROR_CONNECTION_CLOSED;` 而非字面量）。本常量
/// 登记的是**码值**、**不登记常量名** ⇒ `-32000` 只出现一次。
///
/// `router.rs:38` 的注释明写这是「非标准族，`-3260x` 之外的自定义区」。
pub const ACP_PRIVATE_ERROR_CODES: &[i64] = &[-32000, -32001];

/// ACP **control 面**（`V.*` subtype）的 **wire 拼写面**——**可注册**的 11 个
/// subtype 名。
///
/// # 真相来源
///
/// `crates/agent-acp/src/control.rs` 的 `enum ControlSubtype`（12 个变体：11 个
/// 具名 + `Unknown`）与 `impl ControlSubtype::as_str()`（`control.rs:48-66`）里
/// **手写**的 `match` 字面量。serde 侧另有 `#[serde(rename_all = "snake_case")]`
/// （`control.rs:19`）——**变体名 ⇒ wire 名**的**自动**派生规则；本常量登记的是
/// **手写拼写那一份**，两者由 `tests/acp_control_surface.rs` 的「serde 派生名
/// == 手写拼写」断言钉在一起。
///
/// # 为什么它是**承重**的：它就是 **handler 路由键**
///
/// - 注册侧：`host_send.rs` 的 `on_control`（`:96-111`）用 `subtype.as_str()`
///   作 **map 的键**插入 handler 表；
/// - 分派侧：`host_router.rs` 的入站分派（`:171-185`）用**同一个** `as_str()`
///   **查表**。
///
/// ⇒ **拼写写错 ⇒ 查表不命中 ⇒ 请求掉进事件队列**（`host_router.rs:183`）⇒ 不是
/// 报错，而是**静默降级**：请求**永久悬挂**，除非上层自己 `respond_control` 兜底
/// （`host_send.rs:113-122`「对未认领的 control 请求延后应答」）。
///
/// # 为什么**排除** `Unknown`（本常量里**没有** `"unknown"`）
///
/// `Unknown`（`control.rs:43-45`）是 `#[serde(other)]` 的兜底变体：解码遇到
/// **未知/新增** subtype 时**保留原文**（它承载「原文」，不是某个固定拼写）。
/// `as_str()` 对它的 `"unknown"` 是**占位串**（`control.rs:49`：「`Unknown` 无法
/// 还原原文，返回占位串仅供日志」），**不是**任何真实 subtype 的 wire 名。
///
/// 且注册侧**明确拒绝**注册它：`host_send.rs:102-104`
/// `if subtype == ControlSubtype::Unknown { return; }`（「注册它等于死代码」）⇒
/// **`Unknown` 不属于「可注册 handler 的 wire 名面」**；混进清单会误导使用者去
/// 注册一个**永不命中**的键。
///
/// # 命名空间（**与 [`ACP_METHODS`] 不是一套**）
///
/// | 面 | 拼写风格 | 例 |
/// |---|---|---|
/// | ACP method 面 | 斜杠 `域/动作` | `session/update` |
/// | control subtype 面 | 下划线 `snake_case` | `session_update` |
///
/// 归一化后**同名**但**同层不同义** ⇒ **不可互换**。
///
/// # 断言口径（`tests/acp_control_surface.rs`，**只守关系、不锁数量/取值**）
///
/// 源码文本扫描 `control.rs`（`include_str!` **编译期**绑定；`agent-acp` 不是本
/// crate 的依赖，`Cargo.toml` 属共享区、本卡不得改 ⇒ 只能扫文本、不能
/// `use agent_acp::…`）断言：**实现面 ⊆ 登记面**、**登记面 ⊆ 实现面**、
/// **serde 派生名 == 手写拼写**、**逐项锚点仍在**、**`Unknown` 被排除**、
/// **与 method 面拼写风格不相交**。
///
/// ⚠️ **不锁数量/取值**：实现正当新增一个可注册 subtype 时，把新名字登记进本
/// 常量即可；任何 `assert_eq!(ACP_CONTROL_SUBTYPES.len(), 11)` 之类的断言都会
/// 在正当扩充时必红（教训：`tests/method_ledger.rs` 的
/// `assert_eq!(ACP_METHODS.len(), 13)`）。
pub const ACP_CONTROL_SUBTYPES: &[&str] = &[
    "elicitation_create",         // control.rs:53 Self::ElicitationCreate
    "elicitation_complete",       // control.rs:54 Self::ElicitationComplete
    "session_request_permission", // control.rs:55 Self::SessionRequestPermission
    "session_update",             // control.rs:56 Self::SessionUpdate
    "fs_read_text_file",          // control.rs:57 Self::FsReadTextFile
    "fs_write_text_file",         // control.rs:58 Self::FsWriteTextFile
    "terminal_create",            // control.rs:59 Self::TerminalCreate
    "terminal_kill",              // control.rs:60 Self::TerminalKill
    "terminal_output",            // control.rs:61 Self::TerminalOutput
    "terminal_release",           // control.rs:62 Self::TerminalRelease
    "terminal_wait_for_exit",     // control.rs:63 Self::TerminalWaitForExit
                                  // ⛔ 不登记 "unknown"：Unknown 不可注册（见本常量文档注释）。
];

/// ACP control 面的**信封级判别键面**（顶层 `type` 字段的取值）。
///
/// control 面在 JSON-RPC 三分类**之外**还有一层信封，顶层靠 `type` 字段判别：
/// `control_request`（`ControlRequest` / `ControlRequestTag`）、
/// `control_notification`（`ControlNotification` / `ControlNotificationTag`）、
/// `control_response`（`ControlResponse` / `ControlResponseTag`）。
///
/// # 为什么是承重字面量
///
/// `crates/agent-acp/src/control.rs` 的 `TopLevel::classify` 用这些字面量做
/// **顶层帧归类**（它是 `agent-acp` 编解码的**唯一入口**，`codec.rs` 的
/// `decode_frame` 调它）。任一拼写漂移 ⇒ 该信封**不被识别** ⇒ 落 `other`
/// 分支 ⇒ 抛 `MalformedMessage` ⇒ **整帧被拒**（**硬失败**，不是静默降级）。
/// 且 serde 侧也用同一拼写去反序列化 ⇒ **编解码两端会一起错**（对端可能认得、
/// 我方不认）。
///
/// # 双来源（本常量存在的原因）
///
/// ① **唯一权威来源** = 三个 `serde(rename = "…")`（`control.rs` 的
///    `ControlRequestTag` / `ControlNotificationTag` / `ControlResponseTag`）；
/// ② **消费方** = `TopLevel::classify` 的 `match` 分支字面量。
/// 两者是**两处独立手写** ⇒ 必须有机器断言把它们锁在一起。
///
/// # 与 [`ACP_METHODS`] **正交**
///
/// 本面**不含** [`ACP_METHODS`] 里的方法名——那是**载荷内**判别键（`method`
/// 字段），与本面（顶层 `type` 字段）分属两层，**互不相干**。
/// （[`ACP_CONTROL_SUBTYPES`] 同理是**载荷内**的 `subtype` 判别键。）
///
/// # 断言口径（`tests/acp_envelope_tag_surface.rs`，**只守关系、不锁数量**）
///
/// 源码文本扫描 `control.rs`（`include_str!` **编译期**绑定；`agent-acp` 不是本
/// crate 的依赖，`Cargo.toml` 属共享区、本卡不得改 ⇒ 只能扫文本、不能
/// `use agent_acp::…`）断言：**实现面 == 登记面**（双向）、**双来源各自独立**
/// 且相等、**未登记的 type 必被拒**（兜底臂返回 `MalformedMessage`）、**防空扫描**。
///
/// ⚠️ **不锁数量**：实现正当新增一个信封类型时，把新标签登记进本常量即可；任何
/// `assert_eq!(ACP_ENVELOPE_TAGS.len(), 3)` 之类的断言都会在正当扩充时必红
/// （同款教训：`tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。
pub const ACP_ENVELOPE_TAGS: &[&str] = &[
    "control_request",      // control.rs:95 ControlRequestTag（serde rename）
    "control_notification", // control.rs:114 ControlNotificationTag（serde rename）
    "control_response",     // control.rs:138 ControlResponseTag（serde rename）
];

/// `session:replay` 单页上限——`agent-core` 的 `DEFAULT_PAGE_LIMIT` 实际值。
///
/// 真相来源：`crates/agent-core/src/event_log/mod.rs` 的
/// `pub const DEFAULT_PAGE_LIMIT: usize = 500`；`jsonl.rs` 的 `replay()` 用
/// `limit.clamp(1, DEFAULT_PAGE_LIMIT)` 把调用方给的 `limit` 夹进 `[1, 500]`。
/// 网关只透传一个 `limit`（其缺省 `REPLAY_DEFAULT_LIMIT = 200` 亦被该上限钳制），
/// 故对外的可观测上限就是这里。
///
/// `tests/replay_surface.rs` 断言它与 `agent_core::event_log::DEFAULT_PAGE_LIMIT`
/// **逐值相等**：core 侧调大/调小这个上限，本常量必须同批跟改，否则契约红。
pub const REPLAY_MAX_PAGE: usize = 500;

/// `session:replay` 成功响应 `data` 必含的键名（顺序无关）。
///
/// 真相来源：`crates/agent-gateway/src/rpc.rs` 的 `"session:replay"` 分支
/// `json!({"events": events, "lastSeq": last_seq})`。`events` 为 `seq > afterSeq`
/// 的升序一页；`lastSeq` 为这一页末条事件的 `seq`，**空页时回落到传入的 `afterSeq`**。
pub const REPLAY_DATA_KEYS: &[&str] = &["events", "lastSeq"];

// ---- §3 会话/消息 schema：**枚举取值面**与**线上键名面** ----

/// §3 冻结的**会话模式**（5 值）。出处：`AGENT-KICKOFF.md` 第 30 行
/// 「`mode: quick|ask|plan|craft|expert`」；实现侧 `agent-core/src/schema.rs:18`
/// 的 `enum Mode`（snake_case ⇒ 线上同串）。
pub const SESSION_MODES: &[&str] = &["quick", "ask", "plan", "craft", "expert"];

/// §3 冻结的**权限模式**（2 值）。出处：`AGENT-KICKOFF.md` 第 31 行
/// 「`permission_mode: readonly|full`」；实现侧 `agent-core/src/schema.rs:48`
/// 的 `enum PermissionMode`。
pub const PERMISSION_MODES: &[&str] = &["readonly", "full"];

/// §3 冻结的**会话状态**（4 值）。出处：`AGENT-KICKOFF.md` 第 33 行
/// 「`status: running|idle|done|failed`」；实现侧 `agent-core/src/schema.rs:70`
/// 的 `enum SessionStatus`。
pub const SESSION_STATUSES: &[&str] = &["running", "idle", "done", "failed"];

/// §3 冻结的**消息角色**（4 值）。出处：`AGENT-KICKOFF.md` 第 34 行
/// 「`role: user|assistant|tool_call|tool_result`」；实现侧
/// `agent-core/src/schema.rs:80` 的 `enum Role`。
pub const MESSAGE_ROLES: &[&str] = &["user", "assistant", "tool_call", "tool_result"];

/// §3 文档写的 **Rust 侧 snake_case 源字段名**——**不是**线上键名。
///
/// §3 冻结的是**源码结构体的字段名**，而 `agent-core` 的 schema 全带
/// `#[serde(rename_all = "camelCase")]` ⇒ **线上**是 camelCase 那一份。
///
/// 本常量**只**用于一条反向断言：登记的线上键名面**不得**与它相交（见
/// `tests/session_schema_surface.rs`）——防止有人把 §3 原文的 snake_case 名
/// 误当成线上键名登记进去。只收**与线上写法不同**的那些名字（`role` /
/// `content` / `name` / `ms` 等两侧同名，登记无意义）。
pub const KICKOFF_SCHEMA_FIELD_NAMES: &[&str] = &[
    "workspace_root",
    "permission_mode",
    "env_binding",
    "created_at_ms",
    "updated_at_ms",
    "ts_ms",
    "args_ref",
    "result_ref",
    "user_id",
    "tenant_id",
    "env_id",
    "session_id",
];

/// `Session` 线上 JSON 的**顶层键名**（camelCase）。
///
/// ⚠️ **§3 文档写的是 Rust 侧 snake_case 源字段名，线上不是那一份**：§3 第 30–33
/// 行写 `workspace_root` / `permission_mode` / `created_at_ms` 等，而
/// `agent-core/src/schema.rs:142` 的 `Session` 带
/// `#[serde(rename_all = "camelCase")]` ⇒ **线上**是 `workspaceRoot` /
/// `permissionMode` / `createdAtMs` 等。本常量登记的是**线上那一份**。
///
/// 键集合随「`Option` 字段是否为空」变化（`env_binding` / `memory_digest` /
/// `run_status` 带 `skip_serializing_if = "Option::is_none"`）⇒ 断言侧必须构造
/// **把这些字段都填上值**的会话再取键集合。`isolation` / `messages` 是嵌套对象，
/// 本常量只登记它们的**顶层键**；`isolation` 的嵌套键面见 [`ISOLATION_JSON_KEYS`]。
pub const SESSION_JSON_KEYS: &[&str] = &[
    "createdAtMs",
    "envBinding",
    "id",
    "isolation",
    "lastSeq",
    "memoryDigest",
    "messages",
    "mode",
    "permissionMode",
    "runStatus",
    "status",
    "title",
    "updatedAtMs",
    "workspaceRoot",
];

/// `Message` 线上 JSON 的**键名**（camelCase）。出处：
/// `agent-core/src/schema.rs:130` 的 `struct Message`。§3 第 34 行写的源字段名
/// `ts_ms` ⇒ 线上是 `tsMs`；`role` / `content` / `tool` 两侧同名。`tool` 带
/// `skip_serializing_if = "Option::is_none"` ⇒ 断言侧构造**带 tool** 的消息。
pub const MESSAGE_JSON_KEYS: &[&str] = &["content", "role", "tool", "tsMs"];

/// `ToolInfo` 线上 JSON 的**键名**（camelCase）。出处：
/// `agent-core/src/schema.rs:116` 的 `struct ToolInfo`。§3 第 34 行写的源字段名
/// `args_ref` / `result_ref` ⇒ 线上是 `argsRef` / `resultRef`；`name` / `ms`
/// 两侧同名。三个 `Option` 字段都带 `skip_serializing_if = "Option::is_none"`
/// ⇒ 断言侧构造**全部填上值**的 `ToolInfo` 再取键集合。
pub const TOOL_INFO_JSON_KEYS: &[&str] = &["argsRef", "ms", "name", "resultRef"];

/// `Isolation` 线上 JSON 的**键名**（camelCase）。出处：
/// `agent-core/src/schema.rs:90` 的 `struct Isolation`。§3 第 32 行写的源字段名
/// `user_id` / `tenant_id` / `env_id` ⇒ 线上是 `userId` / `tenantId` / `envId`。
/// `env_id` 带 `skip_serializing_if = "Option::is_none"` ⇒ 断言侧构造**填上
/// `envId`** 的隔离键再取键集合。
pub const ISOLATION_JSON_KEYS: &[&str] = &["envId", "tenantId", "userId"];

/// `tool_use` / `tool_result` 事件 **payload 的线上键名**（camelCase）。
///
/// ⚠️ 这是**事件载荷**的键名面，与 [`TOOL_INFO_JSON_KEYS`]（会话消息里 `tool`
/// 子对象的键名面）**不是同一份**——两者字段相近但来源不同，别混。
///
/// 实测来源（`agent-core` 侧，payload 由 `json!` 直接构造，网关原样透传）：
/// - `tool_use`：`runtime/turns.rs:347` 与 `runtime/turns_settle.rs:248` 的
///   `json!({"name", "args", "argsRef", "tsMs"})`；
/// - `tool_result`：`runtime/tool_result.rs:62` 的
///   `json!({"name", "chars", "resultRef", "preview", "tsMs"})`，另由
///   `runtime/execute.rs` 与 `runtime/turns_settle.rs` 的下标写入补上
///   `result` / `isError`（被拒 / 执行失败两种结算）。
///
/// **没有 `ms`**：`ToolInfo.ms` 在实现侧恒为 `None`，事件 payload 里**从无**
/// `ms` 键 ⇒ 壳侧读到的耗时恒为 0 是**正确**的（执行耗时确实没测），不得拿
/// `tsMs`（消息时间戳）顶替（语义不同，那是造数据）。
///
/// `tests/session_schema_surface.rs` 用**源码扫描**断言「实现侧发出的键名
/// ⊆ 本常量」——实现新增键而契约没登记即红。
pub const TOOL_EVENT_PAYLOAD_KEYS: &[&str] = &[
    "approvalId",
    "args",
    "argsRef",
    "chars",
    "executed",
    "isError",
    "name",
    "preview",
    "result",
    "resultRef",
    "toolCallId",
    "tsMs",
];

/// `agent-core` runtime 四份源文件里**全部**事件 payload 键名（宽面）。
///
/// 这是 [`TOOL_EVENT_PAYLOAD_KEYS`]（工具事件窄面）的**超集**，还含同一批源文件
/// 里其它事件（`user` / `thinking` / `text` / `done`）的 payload 键。
///
/// 用途：`tests/session_schema_surface.rs` 对同一批源文件做**全量**扫描，断言
/// 「扫出的键 ⊆ 本常量」——实现侧在**任何**事件 payload 里新增键而契约没登记
/// 即红（窄面只管工具事件，宽面管全部）。`type` / `message` / `reason` 是
/// **嵌套**错误对象的键（整体挂在 `payload["result"]` 之下），不是 payload 顶层
/// 键；它们出现在本常量里是因为扫描按 `json!` 块的顶层键抽取，而该 `json!` 块
/// 本身就是那个嵌套对象。
pub const RUNTIME_EVENT_PAYLOAD_KEYS: &[&str] = &[
    "approvalId",
    "args",
    "argsRef",
    "chars",
    "isError",
    "maxSteps",
    "message",
    "name",
    "note",
    "preview",
    "reason",
    "reminder",
    "result",
    "resultRef",
    "status",
    "steps",
    "text",
    "tool",
    "toolCallId",
    "tsMs",
    "type",
];

// ---- JSON-RPC 2.0 **基础信封键面**（真相来源：`agent-acp/src/message.rs`） ----
/// 帧分类**判别键面**：`Message::from_value` 判别三分类时实际查的键名。
pub const ACP_JSONRPC_DISCRIMINATION_KEYS: &[&str] = &["id", "method", "result", "error"];
/// `JsonRpcRequest` 的线上键名面（wire 键名 = 字段名，该文件无 `serde(rename)`）。
pub const ACP_JSONRPC_REQUEST_KEYS: &[&str] = &["jsonrpc", "id", "method", "params"];
/// `JsonRpcNotification` 的线上键名面。
pub const ACP_JSONRPC_NOTIFICATION_KEYS: &[&str] = &["jsonrpc", "method", "params"];
/// `JsonRpcResponse` 的线上键名面。
pub const ACP_JSONRPC_RESPONSE_KEYS: &[&str] = &["jsonrpc", "id", "result", "error"];
/// `JsonRpcErrorObject`（JSON-RPC error 子对象）的线上键名面。
pub const ACP_JSONRPC_ERROR_OBJECT_KEYS: &[&str] = &["code", "message", "data"];
/// 网关 **u16 信封层**（`{code,msg,data}`）的线上键名面——与上面 error 子对象**同名不同形**。
pub const GATEWAY_ENVELOPE_KEYS: &[&str] = &["code", "msg", "data"];
/// `jsonrpc` 字段的线上取值（由 `JsonRpcVersion` 的自定义 `Deserialize` 硬保证）。
pub const ACP_JSONRPC_VERSION: &str = "2.0";
