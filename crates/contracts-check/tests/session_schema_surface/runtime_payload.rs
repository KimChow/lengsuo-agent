//! 事件 payload 键名的 ⊆ 断言与壳侧读取守卫。

use super::registered;
use super::scanner::{
    runtime_source, scan_index_assignment_keys, scan_json_object_key_groups, scan_json_object_keys,
};
use contracts_check::{RUNTIME_EVENT_PAYLOAD_KEYS, TOOL_EVENT_PAYLOAD_KEYS};
use lengsuo_shell::gateway::{EventKind, GatewayEvent};
use lengsuo_shell::state::{SessionMode, SessionStreamState};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// 被扫描的 `agent-core` runtime 源文件（相对 `agent-core/src/runtime/`）。
///
/// 覆盖面说明：这四份是**事件 payload 的全部构造点**（工具事件的四条路径）：
/// - `turns.rs`——回合主循环，发 `user` / `thinking` / `text` / `done` / `tool_use`；
/// - `turns_settle.rs`——结算路径，发 `tool_use` / `tool_result` / `text` / `done`；
/// - `tool_result.rs`——`tool_result` payload 的构造（`push_tool_result`）；
/// - `execute.rs`——工具执行结算，发 `tool_result`（被拒 / 失败 / MCP 三条）。
///
/// ⚠️ **未覆盖**（本卡范围外，登记面不覆盖）：`sessions.rs` / `context.rs` 发的
/// `session_update` / `user` / `warning` 载荷——它们的键面另属「会话元信息面」。
const SCANNED_RUNTIME_FILES: &[&str] = &[
    "turns.rs",
    "turns_settle.rs",
    "tool_result.rs",
    "execute.rs",
];

/// 标识「工具事件载荷」的**签名键**——只含**工具语义独有**的键。
///
/// 载荷里出现其中任一个 ⇒ 该 `json!` 块是 `tool_use` / `tool_result` 载荷：
/// - `argsRef`——`tool_use` 携带的参数 blob 引用（`turns.rs` / `turns_settle.rs`）；
/// - `resultRef` / `chars` / `preview`——`tool_result` 的结果引用 / 长度 / 摘要；
/// - `isError`——被拒 / 执行失败两种结算标记。
///
/// 识别口径不靠「调用点长什么样」（`emit(…, "tool_use", json!({…}))` 的排版会随
/// 实现改写），而靠**载荷的键名形态**——那才是线上契约的稳定面。
const TOOL_PAYLOAD_SIGNATURE_KEYS: &[&str] =
    &["argsRef", "resultRef", "chars", "preview", "isError"];

/// **宽面断言**：实现源码里扫出的事件 payload 键名 **⊆ 契约登记的键名面**。
///
/// 红了说明：core 在事件 payload 里发了新键，而契约侧没登记——先补登记
/// （并同步 `docs/contracts/shell-agent.md`），再合并。
///
/// ⚠️ 刻意**只守 ⊆ 关系、不锁数量/取值**：实现正当新增 payload 键时，把新键
/// 登记进常量后本断言自然变绿。
#[test]
fn runtime_payload_keys_are_subset_of_the_registered_contract() {
    let contracted = registered(RUNTIME_EVENT_PAYLOAD_KEYS);
    for file in SCANNED_RUNTIME_FILES {
        let scanned = scan_json_object_keys(&runtime_source(file));
        // 防空扫描：扫不出任何键 ⇒ 扫描器本身坏了（路径写错 / 口径改坏），
        // 断言会「因为什么都没扫到」而假绿。
        assert!(
            !scanned.is_empty(),
            "{file} 一个 payload 键名都没扫到——扫描口径或文件路径坏了，断言会假绿"
        );
        let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
        assert!(
            unregistered.is_empty(),
            "{file} 发出但 RUNTIME_EVENT_PAYLOAD_KEYS 未登记的 payload 键（差集）\
             {unregistered:?}——契约落后于实现，先补登记再合并"
        );
    }
}

/// **窄面断言（本卡的核心那条）**：`tool_use` / `tool_result` 事件 payload 的
/// 键名 **⊆ 登记的窄面**。
///
/// 识别口径：按**签名键**认载荷（见 [`TOOL_PAYLOAD_SIGNATURE_KEYS`]）——
/// `tool_use` 载荷带 `argsRef`；`tool_result` 载荷带 `chars` / `preview` /
/// `resultRef` / `isError`。另把下标写入形态（`payload["result"] = …` /
/// `payload["isError"] = …`）计入——它们补进的就是 `tool_result` 载荷。
///
/// 只守 ⊆：实现正当新增工具事件 payload 键时，先登记进
/// [`TOOL_EVENT_PAYLOAD_KEYS`] 后本断言自然变绿。
#[test]
fn tool_event_payload_keys_are_subset_of_the_registered_contract() {
    let contracted = registered(TOOL_EVENT_PAYLOAD_KEYS);
    let mut scanned = BTreeSet::new();
    for file in SCANNED_RUNTIME_FILES {
        let source = runtime_source(file);
        for group in scan_json_object_key_groups(&source) {
            let is_tool_payload = TOOL_PAYLOAD_SIGNATURE_KEYS
                .iter()
                .any(|sig| group.contains(*sig));
            if is_tool_payload {
                scanned.extend(group);
            }
        }
        scanned.extend(scan_index_assignment_keys(&source));
    }
    assert!(
        !scanned.is_empty(),
        "一个工具事件 payload 键名都没扫到——扫描口径坏了，断言会假绿"
    );
    let unregistered: Vec<String> = scanned.difference(&contracted).cloned().collect();
    assert!(
        unregistered.is_empty(),
        "工具事件 payload 键超出 TOOL_EVENT_PAYLOAD_KEYS 登记面（差集）{unregistered:?}——\
         契约落后于实现，先补登记再合并"
    );
}

/// 喂一条**照抄 core 真实形态**的 `tool_use` 事件，返回壳侧解出的参数引用。
///
/// 「真实形态」指键名是 `argsRef`（`agent-core/src/runtime/turns.rs:347` 的
/// `json!({"name", "args", "argsRef", "tsMs"})`）——**camelCase**，不是
/// `args_ref`。
fn shell_reads_args_ref(payload: Value) -> Option<String> {
    let mut state = SessionStreamState::new("s-1", SessionMode::Craft);
    let event = GatewayEvent {
        seq: 1,
        session_id: "s-1".to_string(),
        kind: EventKind::ToolUse,
        payload,
        ts_ms: 1_700_000_000_000u64,
    };
    state
        .apply_event(event)
        .expect("工具调用事件应被接受（craft 模式允许工具消息）");
    state
        .messages()
        .iter()
        .find_map(|m| m.tool.as_ref())
        .expect("应落一条带工具信息的消息")
        .args_ref
        .clone()
}

/// **漂移守卫（本卡的价值所在）**：壳侧必须能读到**线上真实形态**的 `argsRef`。
///
/// 壳侧原先只读 `args_ref` ⇒ 恒为 `None`（字段在、值恒空）——这正是
/// 「core 发 `argsRef`、壳侧读 `args_ref`」的键名漂移。
///
/// 这条断言**在修之前必然红**（当前恒为 `None`）⇒ 它就是本卡的先红后绿证据。
#[test]
fn shell_reads_the_camel_case_args_ref_from_a_real_tool_use_payload() {
    let args_ref = "blob-args-1";
    let got = shell_reads_args_ref(json!({
        "name": "fs.read",
        "args": {"path": "a.txt"},
        "argsRef": args_ref,
        "tsMs": 1_700_000_000_000u64
    }));
    assert_eq!(
        got.as_deref(),
        Some(args_ref),
        "壳侧读不到线上 camelCase 的 `argsRef`——键名漂移（core 发 argsRef，壳侧读 args_ref）"
    );
}

/// **兼容不破坏**：旧形态（`args_ref`，snake_case）**仍应可读**。
#[test]
fn shell_still_reads_the_legacy_snake_case_args_ref() {
    let args_ref = "blob-args-legacy";
    let got = shell_reads_args_ref(json!({
        "name": "fs.read",
        "args": {"path": "a.txt"},
        "args_ref": args_ref,
        "tsMs": 1_700_000_000_000u64
    }));
    assert_eq!(
        got.as_deref(),
        Some(args_ref),
        "壳侧读不到旧形态的 `args_ref`——兼容读取被破坏"
    );
}
