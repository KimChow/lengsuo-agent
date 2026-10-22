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

/// 网关 **u16 信封层**（`{code,msg,data}`）的线上键名面——与上面 error 子对象**同名不同形**。
pub const GATEWAY_ENVELOPE_KEYS: &[&str] = &["code", "msg", "data"];
