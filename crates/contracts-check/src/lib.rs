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

/// v0 全部事件 kind，共 9 个。
///
/// 对应 `AGENT-KICKOFF.md` 第 23 行「事件名」：
/// `thinking / text / tool_use / tool_result / approval_request / files / done`
/// （参考客户端 基线 §5 的七类）+ 「`session_update`」。
///
/// 另注：`user` 是 v0 实现里**持久化在事件日志中的第 9 类**——`agent-core`
/// `schema::EventKind::User` 与 `runtime/turns.rs` 落盘 `"user"` 事件，
/// `lengsuo-shell` 客户端侧 `state/stream/events.rs` 也实现了该分支。
/// 它未写进 `AGENT-KICKOFF.md` 第 23 行的枚举句（该行只按「参考客户端 基线 §5」
/// 抄了七类再补 `session_update`），但实现稳定存在且被测试覆盖，故按
/// 「文档缺口（实现为准）」登记在本常量里——不要在这里删掉它。
pub const EVENT_KINDS: &[&str] = &[
    // 行 23：thinking
    "thinking",
    // 行 23：text
    "text",
    // 行 23：tool_use
    "tool_use",
    // 行 23：tool_result
    "tool_result",
    // 行 23：approval_request
    "approval_request",
    // 行 23：files
    "files",
    // 行 23：done
    "done",
    // 行 23：session_update
    "session_update",
    // 行 23 缺席，但实现与客户端解码都在（见本常量文档注释）：user
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
