//! 壳侧协议入口的**枚举取值面**绑定契约：会话 mode / 会话 status / 消息 role 三面。
//!
//! 同一个协议概念在仓里有**三份手写实现**——`agent-core` 侧一份、
//! `lengsuo-shell` 侧一份、契约登记一份——而契约断言只绑住了 `agent-core`
//! 那一份（见 `tests/session_schema_surface.rs`）。壳侧那一半**无人看守**：
//! 给 `agent-core` 加了新取值并同步更新登记，壳侧解析器不会跟着改，
//! 线上发出新取值时壳侧运行期才炸，而**当前所有断言全绿**。
//!
//! 后果不是假想：壳侧 `SessionMode::parse` 就是 `session_update` 的协议入口
//! （`lengsuo-shell/src/state/stream/events.rs`），未登记取值必须被
//! **显式拒绝**（`StateError::InvalidTransition`），不得静默忽略。
//!
//! # 断言口径：**只守关系，不锁具体取值/数量**
//!
//! - 取值面：壳侧实现侧集合 == 契约登记集合（集合相等，不锁顺序、不锁项数）；
//! - 解析面：登记里的每个名字壳侧都解析得出；壳侧每个变体都被登记命中；
//! - 未登记取值：壳侧必须 fail-closed（返回 `None` / 拒绝事件），不猜语义。
//!
//! ⛔ 刻意**不写** `assert_eq!(x.len(), N)` / `assert_eq!(x, &[...])` 这类锁规模
//! 或锁取值的断言——它们会在正当扩充时必红。
//!
//! # 本卡只加断言，不改任何生产代码
//!
//! 当前壳侧与登记取值面一致 ⇒ 本卡是**护栏**不是修 bug：把「以后漂移了没人
//! 知道」变成「漂移即红」。收敛成唯一定义处需要改 `Cargo.toml`（共享区），
//! 归基建卡，不在本卡范围。
//!
//! # 壳侧新增枚举变体时怎么变红
//!
//! 1. 本文件的**穷尽 match**（无 `_` 兜底臂）⇒ 壳侧新增变体时**编译不过**
//!    ⇒ 改壳侧的人必然被推到本文件；
//! 2. 登记侧若**同步**加了新取值 ⇒ 取值面的**集合相等**断言红（清单与登记不等）；
//! 3. 登记侧若**没**加 ⇒ 协议入口对新取值 fail-closed（`parse` 返回 `None`），
//!    线上发该取值会被壳侧**显式拒绝**（见 E 组探针），不会静默误解。
//!
//! ⚠️ **已知边界**：壳侧新增了一个变体、但**既没**同步登记、**也没**更新本文件
//! 的变体清单时，本卡拦不住（清单与登记仍相等）。该方向的危害是「壳侧接收端
//! 更宽松」，不会造成「线上发新取值被拒」；要彻底堵住需把枚举收敛到唯一定义处
//! （要改 `Cargo.toml`，属基建卡，不在本卡范围）。
use contracts_check::{MESSAGE_ROLES, SESSION_MODES, SESSION_STATUSES};
use lengsuo_shell::gateway::{EventKind, GatewayEvent};
use lengsuo_shell::state::{MessageRole, SessionMode, SessionStatus, SessionStreamState};
use lengsuo_shell::StateError;
use serde_json::json;
use std::any::Any;
use std::collections::BTreeSet;

/// 壳侧 `SessionMode` 的全部变体（壳侧**没有** `ALL` 常量，只能逐个列举）。
const SHELL_MODE_VARIANTS: &[SessionMode] = &[
    SessionMode::Quick,
    SessionMode::Ask,
    SessionMode::Plan,
    SessionMode::Craft,
    SessionMode::Expert,
];

/// 壳侧 `SessionStatus` 的全部变体。
const SHELL_STATUS_VARIANTS: &[SessionStatus] = &[
    SessionStatus::Running,
    SessionStatus::Idle,
    SessionStatus::Done,
    SessionStatus::Failed,
];

/// 壳侧 `MessageRole` 的全部变体。
const SHELL_ROLE_VARIANTS: &[MessageRole] = &[
    MessageRole::User,
    MessageRole::Assistant,
    MessageRole::ToolCall,
    MessageRole::ToolResult,
];

/// 把契约登记常量转成集合。
fn registered(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// **集合相等**的唯一出口：三个取值面都走它。
///
/// 口径：实现侧取值集合 == 契约登记取值集合（**集合相等**，不锁顺序、不锁
/// 项数）。集合相等 ⇒ 两边**同时**增删项即红；只增一边也红。
fn assert_same_value_face(
    label: &str,
    implemented: BTreeSet<String>,
    contracted: BTreeSet<String>,
) {
    assert_eq!(
        implemented, contracted,
        "{label} 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}——\
         两边必须同时增删（实现侧改了就要同步契约登记，登记改了就要同步实现侧）"
    );
}

/// 壳侧 `SessionMode` 的协议名。
///
/// **穷尽 match**（不加 `_` 兜底臂）：壳侧新增 mode ⇒ 本函数**编译不过**
/// ⇒ 漂移在编译期就红，不会等到运行期才发现。协议名一律取自壳侧自己的
/// `as_str()`，本文件不另抄一份字符串。
fn shell_mode_name(mode: SessionMode) -> &'static str {
    match mode {
        SessionMode::Quick => SessionMode::Quick.as_str(),
        SessionMode::Ask => SessionMode::Ask.as_str(),
        SessionMode::Plan => SessionMode::Plan.as_str(),
        SessionMode::Craft => SessionMode::Craft.as_str(),
        SessionMode::Expert => SessionMode::Expert.as_str(),
    }
}

/// 壳侧 `SessionMode` 的取值面（协议名集合）。
fn shell_mode_face() -> BTreeSet<String> {
    SHELL_MODE_VARIANTS
        .iter()
        .map(|mode| shell_mode_name(*mode).to_string())
        .collect()
}

/// 壳侧 `SessionStatus` 的协议名（本文件内的显式对照表）。
///
/// 壳侧 `SessionStatus` **没有 `as_str()`** ⇒ 取值面只能靠显式对照表达。
/// 同样用**穷尽 match**：壳侧新增 status ⇒ 编译不过。
///
/// 表里的每个名字都会在 `shell_session_status_values_match_registered_contract`
/// 里与壳侧真实的 `parse()` 回验 ⇒ 这张表不是「自说自话」。
fn shell_status_name(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "running",
        SessionStatus::Idle => "idle",
        SessionStatus::Done => "done",
        SessionStatus::Failed => "failed",
    }
}

/// 壳侧 `SessionStatus` 的取值面（协议名集合）。
fn shell_status_face() -> BTreeSet<String> {
    SHELL_STATUS_VARIANTS
        .iter()
        .map(|status| shell_status_name(*status).to_string())
        .collect()
}

/// 壳侧 `MessageRole` 的协议名（本文件内的显式对照表）。
///
/// 壳侧 `MessageRole` **既无 `as_str()` 也无 `parse()`** ⇒ 取值面只能靠显式
/// 对照表达。同样用**穷尽 match**：壳侧新增 role ⇒ 编译不过。
fn shell_role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::ToolCall => "tool_call",
        MessageRole::ToolResult => "tool_result",
    }
}

/// 壳侧 `MessageRole` 的取值面（协议名集合）。
fn shell_role_face() -> BTreeSet<String> {
    SHELL_ROLE_VARIANTS
        .iter()
        .map(|role| shell_role_name(*role).to_string())
        .collect()
}

/// **A 组 · 1**：壳侧 mode 取值面必须与契约登记**集合相等**。
///
/// 守住的方向：壳侧给 mode 改值 / 加值而登记没跟上（或反过来）。
/// 集合相等 ⇒ 两边同时增删项即红。
#[test]
fn shell_session_mode_values_match_registered_contract() {
    assert_same_value_face("SessionMode", shell_mode_face(), registered(SESSION_MODES));
}

/// **A 组 · 2**：登记里的**每一个** mode 名，壳侧 `parse` 都必须认。
///
/// 守住的方向：**登记加了新 mode、壳侧 `parse` 漏改**——这正是线上炸掉的
/// 路径（壳侧 `parse` 是 `session_update` 的协议入口）。
#[test]
fn shell_session_mode_parse_accepts_every_registered_mode() {
    let unrecognized: Vec<&str> = SESSION_MODES
        .iter()
        .copied()
        .filter(|name| SessionMode::parse(name).is_none())
        .collect();
    assert!(
        unrecognized.is_empty(),
        "契约登记的 mode 取值 {unrecognized:?} 壳侧 SessionMode::parse 认不出——\
         协议入口与登记漂移（线上发这些取值时壳侧会运行期拒绝）"
    );
}

/// **A 组 · 3**：`parse` 与 `as_str()` 必须往返一致。
///
/// 守住的方向：壳侧 `parse` 与 `as_str` 指向**不同的名字**（如 `parse("ask")`
/// 给出 `Plan`、`as_str()` 却说 `"plan"`）——单看集合相等查不出来。
#[test]
fn shell_session_mode_parse_round_trips() {
    for mode in SHELL_MODE_VARIANTS {
        let name = shell_mode_name(*mode);
        assert_eq!(
            SessionMode::parse(name),
            Some(*mode),
            "SessionMode 往返不一致：{name} 应解析回 {mode:?}"
        );
    }
}

/// **A 组 · 4**：未登记取值必须被**显式拒绝**（fail-closed，不猜语义）。
#[test]
fn shell_session_mode_parse_rejects_unknown_value() {
    assert!(
        SessionMode::parse("__not_a_mode__").is_none(),
        "壳侧 SessionMode::parse 必须对未知取值返回 None（不猜语义）"
    );
}

/// **B 组 · 1**：壳侧 status 取值面必须与契约登记**集合相等**。
///
/// 壳侧 `SessionStatus` **没有 `as_str()`** ⇒ 取值面用**双向表达**：
/// - ① 登记侧 → 实现侧：登记里的每个名字 `parse` 得 `Some(_)`；
/// - ② 实现侧 → 登记侧：壳侧的每个变体都被某个登记名 `parse` 命中。
///
/// 两侧合起来才是「集合相等」：只做 ① 会漏掉「壳侧多了一个登记里没有的状态」，
/// 只做 ② 会漏掉「登记里多了一个壳侧认不出的状态」。
#[test]
fn shell_session_status_values_match_registered_contract() {
    // 对照表 ↔ 壳侧真实解析器：每一行都要自洽（表里的名字 parse 出来就是该变体）。
    for status in SHELL_STATUS_VARIANTS {
        let name = shell_status_name(*status);
        assert_eq!(
            SessionStatus::parse(name),
            Some(*status),
            "壳侧 status 对照表与解析器不一致：{name} 应解析成 {status:?}"
        );
    }

    // ① 登记侧 → 实现侧。
    let unrecognized: Vec<&str> = SESSION_STATUSES
        .iter()
        .copied()
        .filter(|name| SessionStatus::parse(name).is_none())
        .collect();
    assert!(
        unrecognized.is_empty(),
        "契约登记的 status 取值 {unrecognized:?} 壳侧 SessionStatus::parse 认不出——\
         登记与实现漂移"
    );

    // ② 实现侧 → 登记侧。
    let uncovered: Vec<&str> = SHELL_STATUS_VARIANTS
        .iter()
        .filter(|status| {
            !SESSION_STATUSES
                .iter()
                .any(|name| SessionStatus::parse(name) == Some(**status))
        })
        .map(|status| shell_status_name(*status))
        .collect();
    assert!(
        uncovered.is_empty(),
        "壳侧 status 变体 {uncovered:?} 不在契约登记里——登记落后于实现，先补登记"
    );

    assert_same_value_face(
        "SessionStatus",
        shell_status_face(),
        registered(SESSION_STATUSES),
    );
}

/// **B 组 · 2**：未登记 status 必须被**显式拒绝**（fail-closed）。
#[test]
fn shell_session_status_parse_rejects_unknown_value() {
    assert!(
        SessionStatus::parse("__not_a_status__").is_none(),
        "壳侧 SessionStatus::parse 必须对未知取值返回 None（不猜语义）"
    );
}

/// **C 组 · 1**：壳侧 role 取值面必须与契约登记**集合相等**。
///
/// 壳侧 `MessageRole` **既无 `as_str()` 也无 `parse()`** ⇒ 取值面靠本文件内
/// 的显式对照表（`shell_role_name`）表达，⛔ 不给壳侧加方法（那是改生产代码）。
/// 守的是「壳侧改了 role 名 / 加了 role，而登记没跟上」。
#[test]
fn shell_message_role_values_match_registered_contract() {
    assert_same_value_face("MessageRole", shell_role_face(), registered(MESSAGE_ROLES));
}

/// 取出 panic payload 里的文本（`catch_unwind` 之后自证用）。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// **D 组 · 护栏自证**：集合相等的护栏必须在输入不同时**真的 panic**。
///
/// 没有这条，本卡的断言可能恒绿（helper 写错 / 集合口径写错都测不出来）
/// ⇒ 本卡白做。用 `catch_unwind` 捕获 panic：**捕获不到 = 护栏恒绿 = 本卡无效**。
///
/// `catch_unwind` 会让 panic 消息打到 stderr（噪音）⇒ 临时 `set_hook` 静音，
/// 捕获后**恢复原 hook**（`take_hook` + `set_hook`）。
#[test]
fn guard_detects_enumeration_drift() {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| {
        assert_same_value_face(
            "drift-probe",
            BTreeSet::from(["implemented".to_string()]),
            BTreeSet::from(["registered".to_string()]),
        );
    });
    std::panic::set_hook(previous_hook);

    let payload = outcome.expect_err("护栏必须能红：两套不同的取值集合竟未触发 panic");
    let message = panic_message(payload.as_ref());
    assert!(
        message.contains("drift-probe"),
        "panic 文案应带上取值面标签，实际 {message}"
    );
}

/// 构造一条归属 `s1` 的网关事件。
fn event(kind: EventKind, seq: u64, payload: serde_json::Value) -> GatewayEvent {
    GatewayEvent {
        seq,
        session_id: "s1".to_string(),
        kind,
        payload,
        ts_ms: 1_700_000_000_000 + seq,
    }
}

/// 起一个会话流并落一条合法文本事件（seq=1）。
fn seeded(mode: SessionMode) -> SessionStreamState {
    let mut state = SessionStreamState::new("s1", mode);
    state
        .apply_event(event(EventKind::Text, 1, json!({ "text": "开始处理" })))
        .expect("seq=1 的合法文本事件应被接受");
    state
}

/// **E 组 · 行为级探针**：未登记 mode 走**真实协议入口**必须被显式拒绝。
///
/// 入口是壳侧 `SessionStreamState::apply_event`——线上 `session_update` 的
/// `mode` 字段正走这条路（`lengsuo-shell/src/state/stream/events.rs`）。
///
/// - 送 `{"mode": "__unregistered__"}` ⇒ `Err(StateError::InvalidTransition)`
///   （**显式拒绝**，不静默忽略、不猜语义）；
/// - 再送**同一 seq** 的 `{"mode": "craft"}` ⇒ `Ok` ⇒ 说明被拒事件没有消费
///   序号，且模式确实被改成了 `Craft`。
#[test]
fn shell_session_update_rejects_unregistered_mode() {
    let mut state = seeded(SessionMode::Quick);

    let err = state
        .apply_event(event(
            EventKind::SessionUpdate,
            2,
            json!({ "mode": "__unregistered__" }),
        ))
        .expect_err("未登记的 mode 必须被显式拒绝，不静默忽略");
    assert!(
        matches!(err, StateError::InvalidTransition { .. }),
        "拒绝原因应为 InvalidTransition，实际 {err:?}"
    );

    state
        .apply_event(event(
            EventKind::SessionUpdate,
            2,
            json!({ "mode": "craft" }),
        ))
        .expect("同 seq 的合法 session_update 应可应用");
    assert_eq!(state.mode(), SessionMode::Craft, "模式应被更新为 Craft");
    assert_eq!(state.next_seq(), 3, "被拒事件不得消费序号");
}
