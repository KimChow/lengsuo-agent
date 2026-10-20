//! 契约验证 crate：把 `docs/contracts/` 与 `AGENT-KICKOFF.md` 的冻结条款
//! 变成可执行断言。本 crate 不参与生产，只被 `tests/` 使用。
//!
//! 本模块只放「冻结常量的单一来源」：动作面、事件面、响应码段。测试从这些常量
//! 出发做穷举断言，任何一侧偷偷加/删一项都会在 `tests/` 里红掉，而不是等到
//! 两轨合流时才发现「文档写着、代码各写各的」。
//!
//! 真相来源：`AGENT-KICKOFF.md` §2（本地网关协议 v0 冻结版）。
#![forbid(unsafe_code)]

/// v0 全部动作名（`<域>:<动作>`），共 11 个。
///
/// 逐项对应 `AGENT-KICKOFF.md` 第 24 行「v0 动作面」的冻结清单：
/// `session:list/create/get/send/stop/replay`、`approval:respond`、
/// `config:get/set`、`tool:list`、`sys:ping`
/// （`session:replay` 为 2026-10-19「只增不改」追加，同见该行）。
pub const ACTIONS: &[&str] = &[
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
    // agent-acp method::INITIALIZE
    "initialize",
    // agent-acp method::AUTHENTICATE
    "authenticate",
    // agent-acp method::SESSION_NEW
    "session/new",
    // agent-acp method::SESSION_LOAD
    "session/load",
    // agent-acp method::SESSION_RESUME
    "session/resume",
    // agent-acp method::SESSION_PROMPT
    "session/prompt",
    // agent-acp method::SESSION_CANCEL（通知）
    "session/cancel",
    // agent-acp method::SESSION_UPDATE（通知）
    "session/update",
    // agent-acp method::SESSION_SET_MODE
    "session/set_mode",
    // agent-acp method::SESSION_REQUEST_PERMISSION
    "session/request_permission",
    // agent-acp method::LOGOUT
    "logout",
    // agent-acp method::ELICITATION_CREATE
    "elicitation/create",
    // agent-acp method::ELICITATION_COMPLETE
    "elicitation/complete",
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
    // agent-core EventKind::Thinking
    "thinking",
    // agent-core EventKind::Text
    "text",
    // agent-core EventKind::ToolUse
    "tool_use",
    // agent-core EventKind::ToolResult
    "tool_result",
    // agent-core EventKind::ApprovalRequest
    "approval_request",
    // agent-core EventKind::ApprovalResolved（第 23 行缺席，见常量文档注释）
    "approval_resolved",
    // agent-core EventKind::Files
    "files",
    // agent-core EventKind::Done
    "done",
    // agent-core EventKind::Error（第 23 行缺席，见常量文档注释）
    "error",
    // agent-core EventKind::SessionUpdate
    "session_update",
    // 持久化事件 `user`（非 `agent-core::schema::EventKind` 变体，见常量文档注释）
    "user",
];

/// 已冻结的响应码（`AGENT-KICKOFF.md` 第 22 行「信封」+ 第 25 行「鉴权」）。
///
/// - `0`   成功（`code==0` 即成功，`msg` 为 `ok`）
/// - `403` 鉴权拒绝（token 缺失/错误；`agent-gateway` 中间件在路由之前返回）
/// - `404` 未注册动作（「其余动作 404，不预留半成品」）
/// - `408` 超时（默认 30s，`rpc_timeout_ms` 可调）
///
/// 第 22 行提到「其余按 server-contract 错误码段」，但该段**不在本仓**
/// （属参考客户端，见报告「偏离清单」），故本常量只固化本仓已冻结的四个码。
pub const ENVELOPE_CODES: &[u16] = &[
    0,   // 行 22：code==0 成功
    403, // 行 22/25：403 鉴权拒绝
    404, // 行 22：404 未注册动作
    408, // 行 22：408 超时
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
