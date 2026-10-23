//! 事件种类面**第三条腿**：壳侧 `EventKind` 变体集合 ⇄ 契约登记 `EVENT_KINDS`
//! 的双向绑定守卫。
//!
//! 「事件种类（event kind）」这个协议概念在本仓有**三份手写实现**：
//!
//! | 源 | 位置 |
//! |---|---|
//! | agent-core（真相来源） | `crates/agent-core/src/schema.rs` 的 `enum EventKind` |
//! | 壳侧解码器 | `crates/lengsuo-shell/src/gateway/events.rs` 的 `pub enum EventKind` |
//! | 契约登记 | `crates/contracts-check/src/events.rs` 的 `EVENT_KINDS` |
//!
//! 三条腿里前两条已被守住：`tests/acp_namespace_disjoint.rs` 守
//! 「agent-core 实现面 ⊆ 登记」，`tests/event_surface.rs` 的缺口探针守
//! 「登记 ⊆ 壳侧可解码」。**壳侧变体集合这一条此前无人看守**：壳侧 `EventKind`
//! 是客户端解码事件流的协议入口，一旦与登记面不同步，线上发出的事件会被壳侧
//! **显式拒绝**（`EventError::UnknownKind`）。壳侧源码注释本身就记录过一次真实漂移
//! ——`ApprovalResolved` 曾因缺此变体导致该事件被拒。
//!
//! # 断言口径：**只守关系，不锁具体取值/数量**
//!
//! - 取值面：壳侧变体集合 == 契约登记集合（集合相等，不锁顺序、不锁项数）；
//! - 拼写面：登记里每个词壳侧 serde 解得开且往返一致，与本文件手写拼写表一致；
//! - 反向：壳侧每个变体的线上词都必须 ∈ 登记；
//! - 未登记词：壳侧**真实解码路径**必须 fail-closed（`EventError::UnknownKind`）。
//!
//! ⛔ 刻意**不写** `assert_eq!(EVENT_KINDS.len(), N)` / `assert_eq!(shell_variants, &[...])`
//! 这类锁规模或锁取值的断言——它们会在正当扩充时必红（故集合断言一律走
//! `BTreeSet` 相等）。
//!
//! # 壳侧增删变体时怎么变红（**编译期锚**）
//!
//! [`shell_event_kind_wire_name`] 是**穷尽 match（无 `_` 兜底臂）**：壳侧
//! `EventKind` 一旦**新增或删除变体**，本文件**编译不过**——这是**有意的**。
//! 壳侧那个 enum 没有可枚举的 `ALL` 常量，「变体清单与 enum 同步」只能由
//! 编译器的穷尽性检查强制；改壳侧的人会被编译错误推到这里，再顺带看到集合相等
//! 断言是否也红了（登记侧同步了吗？）。
//!
//! ⚠️ **已知边界**：若壳侧新增变体、本文件变体清单与登记**同时**跟着改，则集合相等
//! 仍绿。该路径的危害是「壳侧接收端更宽松」，不会造成「线上事件被拒」；彻底堵死
//! 需把枚举收敛到唯一定义处（要改 `Cargo.toml` 共享区，不属本卡范围）。
//!
//! 本卡只新增断言，不改任何生产代码。
use contracts_check::EVENT_KINDS;
use lengsuo_shell::gateway::{EventError, EventKind, GatewayClient, GatewayHandshake};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

mod support;

/// 壳侧 `EventKind` 的全部变体（壳侧**没有** `ALL` 常量，只能逐个列举）。
///
/// 与 [`shell_event_kind_wire_name`] 的穷尽 match 互为保险：清单漏了 enum 里的
/// 变体不会在这里报错，但会让 match **非穷尽** ⇒ 编译不过 ⇒ 必须回来补。
const SHELL_EVENT_KIND_VARIANTS: &[EventKind] = &[
    EventKind::User,
    EventKind::Thinking,
    EventKind::Text,
    EventKind::ToolUse,
    EventKind::ToolResult,
    EventKind::ApprovalRequest,
    EventKind::ApprovalResolved,
    EventKind::Files,
    EventKind::Done,
    EventKind::Error,
    EventKind::SessionUpdate,
];

/// 壳侧 `EventKind` 的线上拼写（本文件内的显式对照表）。
///
/// **穷尽 match（无 `_` 兜底臂）**：壳侧新增/删除变体 ⇒ 本函数**编译不过**
/// ⇒ 漂移在编译期就红（见文件头「编译期锚」）。表里的每个名字都会在
/// [`every_registered_kind_round_trips_through_shell_serde`] 里与壳侧真实的
/// serde 派生规则互验 ⇒ 这张表不是「自说自话」。
fn shell_event_kind_wire_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::User => "user",
        EventKind::Thinking => "thinking",
        EventKind::Text => "text",
        EventKind::ToolUse => "tool_use",
        EventKind::ToolResult => "tool_result",
        EventKind::ApprovalRequest => "approval_request",
        EventKind::ApprovalResolved => "approval_resolved",
        EventKind::Files => "files",
        EventKind::Done => "done",
        EventKind::Error => "error",
        EventKind::SessionUpdate => "session_update",
    }
}

/// 壳侧 `EventKind` 的取值面（线上拼写集合），由上面的穷尽 match 映射而得。
fn shell_event_kind_face() -> BTreeSet<String> {
    SHELL_EVENT_KIND_VARIANTS
        .iter()
        .map(|kind| shell_event_kind_wire_name(*kind).to_string())
        .collect()
}

/// 把契约登记常量转成集合。
fn registered(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// **集合相等**的唯一出口：与 `tests/shell_protocol_enum_surface.rs` 同款口径
/// （实现侧取值集合 == 契约登记取值集合，不锁顺序、不锁项数）。
fn assert_same_value_face(
    label: &str,
    implemented: BTreeSet<String>,
    contracted: BTreeSet<String>,
) {
    assert_eq!(
        implemented, contracted,
        "{label} 取值面与契约登记不一致：壳侧 {implemented:?} vs 登记 {contracted:?}——\
         两边必须同时增删（壳侧改了就要同步契约登记，登记改了就要同步壳侧解码器）"
    );
}

/// **① 集合相等（双向）**：壳侧变体集合必须与 `EVENT_KINDS` **集合相等**。
///
/// 守住的方向：壳侧解码器与契约登记任一侧单方面增删事件种类。
/// 集合相等 ⇒ 只改一边即红，同时改两边才绿。
#[test]
fn shell_event_kind_values_match_registered_contract() {
    assert_same_value_face(
        "EventKind",
        shell_event_kind_face(),
        registered(EVENT_KINDS),
    );
}

/// **② 线上拼写钉死（逐项往返）**：登记的每个词壳侧都解得开、编码回去不变，
/// 且与手写拼写表（①的 [`shell_event_kind_wire_name`]）一致。
///
/// 守住的方向：登记加了新 kind、而壳侧解码器没跟上——这正是「线上发出的事件
/// 被壳侧显式拒绝」的爆炸路径。
#[test]
fn every_registered_kind_round_trips_through_shell_serde() {
    for name in EVENT_KINDS {
        let decoded: EventKind = serde_json::from_value(json!(name)).unwrap_or_else(|e| {
            panic!("契约登记的 kind `{name}` 壳侧 EventKind 解不开（壳侧落后于登记）：{e}")
        });
        let reencoded = serde_json::to_value(decoded).expect("壳侧 EventKind 应可序列化");
        assert_eq!(
            reencoded,
            json!(name),
            "登记词 `{name}` 壳侧解码后再编码回不去（往返不一致）——\
             壳侧 serde 派生规则与登记面漂移"
        );
        assert_eq!(
            shell_event_kind_wire_name(decoded),
            *name,
            "登记词 `{name}` 解出的壳侧变体 {decoded:?} 在手写拼写表里写作 \
             `{}`——拼写表与壳侧 serde 派生规则不一致",
            shell_event_kind_wire_name(decoded)
        );
    }
}

/// **③ 反向包含**：壳侧每个变体的线上词都必须 ∈ `EVENT_KINDS`。
///
/// 这是「壳侧变体 ⊆ 登记」的方向（腿 C 的核心；登记 ⊄ 壳侧那半边由
/// `tests/event_surface.rs` 的缺口探针守）。守住的方向：壳侧偷偷加了一个
/// 登记面不认识的事件种类（登记落后于实现）。
#[test]
fn every_shell_variant_wire_word_is_registered() {
    for kind in SHELL_EVENT_KIND_VARIANTS {
        let wire = serde_json::to_value(kind).expect("壳侧 EventKind 应可序列化");
        let word = wire
            .as_str()
            .unwrap_or_else(|| panic!("壳侧 EventKind 的线上词应为 JSON 字符串，实得 {wire}"));
        assert!(
            EVENT_KINDS.contains(&word),
            "壳侧变体 {kind:?} 的线上词 `{word}` 不在契约登记 EVENT_KINDS 里——\
             登记落后于壳侧实现，先补登记再放行"
        );
    }
}

/// **④ 未登记词 fail-closed**：走壳侧**真实解码路径**的未知 kind 必须被
/// `EventError::UnknownKind` 显式拒绝，**不得**静默接受。
///
/// 探针词先行断言「确实不在登记里」，避免登记扩充后本用例变成空对空。
#[tokio::test]
async fn unregistered_kind_is_rejected_with_unknown_kind() {
    const PROBE: &str = "__probe_unknown_kind__";
    assert!(
        !EVENT_KINDS.contains(&PROBE),
        "探针词 `{PROBE}` 竟已被登记——本用例的前提失效，换一个确定未登记的探针词"
    );

    let mock = support::ws_mock(vec![support::event_frame(
        1,
        "11111111-1111-1111-1111-111111111111",
        PROBE,
        &json!({ "text": "hi" }).to_string(),
        1_700_000_000_000,
    )])
    .await;
    let client = GatewayClient::from_handshake(GatewayHandshake {
        port: mock.addr().port(),
        pid: std::process::id(),
        token: "loopback-mock".to_owned(),
    })
    .with_timeout(Duration::from_secs(5));
    let mut sub = client.subscribe_events().await.expect("WS 握手成功");

    match sub.next().await {
        Err(EventError::UnknownKind { kind }) => {
            assert_eq!(kind, PROBE, "UnknownKind 应回报原始 kind 名，不得改写/丢弃")
        }
        other => panic!("未登记 kind `{PROBE}` 必须被 UnknownKind 显式拒绝，实得 {other:?}"),
    }
    mock.shutdown().await;
}

/// **⑤ 防空自检**：两侧面都非空，且穷尽 match 覆盖到的变体数（= 变体清单大小）
/// 等于壳侧取值面大小。
///
/// 没有这条，「扫了个空集、后面全绿」和「match 把两个变体写成同一个词」都测不出来。
#[test]
fn parity_guard_inputs_are_non_empty() {
    assert!(
        !EVENT_KINDS.is_empty(),
        "契约登记 EVENT_KINDS 是空的——上面所有集合断言都会空对空假绿"
    );
    assert!(
        !SHELL_EVENT_KIND_VARIANTS.is_empty(),
        "壳侧变体清单是空的——穷尽 match 无从覆盖"
    );
    assert_eq!(
        shell_event_kind_face().len(),
        SHELL_EVENT_KIND_VARIANTS.len(),
        "穷尽 match 覆盖到的变体数 {} ≠ 壳侧变体清单大小 {}——\
         有两条 match 臂写成了同一个线上词（反解歧义），或清单里有重复项",
        shell_event_kind_face().len(),
        SHELL_EVENT_KIND_VARIANTS.len()
    );
}

/// **⑥ 护栏自证**：集合相等的护栏必须在输入不同时**真的 panic**。
///
/// 没有这条，本卡的断言可能恒绿（helper 写错 / 集合口径写错都测不出来）
/// ⇒ 本卡白做。用 `catch_unwind` 捕获：**捕获不到 = 护栏恒绿 = 本卡无效**。
///
/// `catch_unwind` 会让 panic 消息打到 stderr（噪音）⇒ 临时静音 hook，
/// 捕获后**恢复原 hook**。
#[test]
fn guard_detects_event_kind_drift() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        assert_same_value_face(
            "EventKind-drift-probe",
            BTreeSet::from(["implemented".to_string()]),
            BTreeSet::from(["registered".to_string()]),
        );
    });
    std::panic::set_hook(previous_hook);

    let payload = outcome.expect_err("护栏必须能红：两套不同的事件种类集合竟未触发 panic");
    let message = if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    };
    assert!(
        message.contains("EventKind-drift-probe"),
        "panic 文案应带上取值面标签，实际 {message}"
    );
}
