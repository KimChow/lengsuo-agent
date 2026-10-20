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
    // 08b9f40 新增（非行 24 冻结项；台账 docs/contracts/acp-method-ledger.md §1）：
    // session:cancel（取消当前回合，fire-and-forget，立即回快照）
    "session:cancel",
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
