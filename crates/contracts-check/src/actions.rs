#[allow(unused_imports)] // 仅供本模块文档注释里的跨模块链接解析；本模块代码不引用它。
use crate::acp::ACP_FORWARD_WHITELIST_SNAPSHOT;

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
