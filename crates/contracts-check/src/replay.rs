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
