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

/// `agent-core` runtime 源文件里**全部**事件 payload 键名（宽面）。
///
/// 这是 [`TOOL_EVENT_PAYLOAD_KEYS`]（工具事件窄面）的**超集**，还含被扫源文件
/// 里其它事件（`user` / `thinking` / `text` / `done` / `session_update` /
/// `turn.queue.*` / 技能注入 / 模型重试 …）的 payload 键。
///
/// 覆盖面（**三处扫描共用本常量，各扫各的文件、同一个 ⊆ 口径**）：
/// - **工具事件面**（`tests/session_schema_surface/runtime_payload.rs`）：
///   `turns.rs` / `turns_settle.rs` / `tool_result.rs` / `execute.rs`；
/// - **会话元信息面**（`tests/session_meta_payload_surface.rs`）：
///   `sessions.rs` / `context.rs` 发的 `session_update` / `memory_update` /
///   `warning` / `user` 载荷；
/// - **投递队列 / 技能注入 / 模型重试面**
///   （`tests/runtime_event_file_coverage.rs`）：`turn_queue.rs`（`turn.{steer,queued}
///   .enqueued` / `turn.queue.expired` / `turn.{steer,queued}.consumed`）/
///   `skills.rs`（`SKILL_INJECTED_EVENT`）/ `retry_policy.rs`（`MODEL_RETRY_EVENT`）。
///   这三个文件原先**零断言**守着（实现加键或改名时契约侧静默落后、不变红），
///   该测试把这条缺口补上；同一测试还带**覆盖面完备性护栏**：runtime 目录下每个
///   含 `.emit(` 的文件必须在「已覆盖清单 ∪ 豁免表」内，否则变红（防这类手工清单
///   缺口再复发）。
///
/// 用途：三处测试对各自那批源文件做**全量**扫描，断言「扫出的键 ⊆ 本常量」——
/// 实现侧在**任何**事件 payload 里新增键而契约没登记即红（窄面只管工具事件，
/// 宽面管全部）。`type` / `message` / `reason` 是**嵌套**错误对象的键（整体挂在
/// `payload["result"]` 之下），不是 payload 顶层键；它们出现在本常量里是因为
/// 扫描按 `json!` 块的顶层键抽取，而该 `json!` 块本身就是那个嵌套对象。同理
/// `ok`（`sessions.rs` 的 `execute_memory_tool` **返回值**，非事件载荷）与
/// `code` / `expertId` / `skill` / `kind` / `mode` / `title`（会话元信息事件的真
/// 正载荷键）一并登记——口径是「该源文件里 `json!` 块发出的顶层键」，只多不漏。
/// `tools` / `content`（`SKILL_INJECTED_EVENT` 的工具名列表与段落正文）同理。
pub const RUNTIME_EVENT_PAYLOAD_KEYS: &[&str] = &[
    "approvalId",
    "args",
    "argsRef",
    "attempt",
    "chars",
    "code",
    "content",
    "delayMs",
    "enqueuedAtMs",
    "errorKind",
    "expertId",
    "isError",
    "kind",
    "maxSteps",
    "message",
    "mode",
    "name",
    "note",
    "ok",
    "ownerRequestId",
    "path",
    "preview",
    "reason",
    "reminder",
    "result",
    "resultRef",
    "skill",
    "source",
    "sourceRef",
    "status",
    "steps",
    "text",
    "title",
    "tool",
    "toolCallId",
    "tools",
    "tsMs",
    "type",
    "version",
    "willRetry",
];
