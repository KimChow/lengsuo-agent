//! 四组枚举的线上取值面断言。

use super::registered;
use agent_core::schema::{PermissionMode, Role, SessionStatus};
use contracts_check::{MESSAGE_ROLES, PERMISSION_MODES, SESSION_MODES, SESSION_STATUSES};
use std::collections::BTreeSet;

/// 取一组枚举的**线上字符串**集合。
///
/// 取值口径按「实现侧真实可用的方式」分两种：
/// - `Mode` / `PermissionMode` 有 `as_str()` ⇒ 直接取；
/// - `SessionStatus` / `Role` **没有** `as_str()` ⇒ 用 **serde 序列化**
///   （`#[serde(rename_all = "snake_case")]` ⇒ 序列化结果就是线上那个串）。
///
/// 两种口径的**共同点**是「取出来的串就是线上那个串」——否则登记面与线上面对不上。
macro_rules! wire_values {
    ($($value:expr),+ $(,)?) => {{
        let mut set: BTreeSet<String> = BTreeSet::new();
        $(
            let text = serde_json::to_value($value)
                .expect("枚举可序列化")
                .as_str()
                .expect("枚举序列化为字符串")
                .to_string();
            set.insert(text);
        )+
        set
    }};
}

/// §3 的 `Mode` 五值必须与实现侧**集合相等**。
///
/// 实现侧取 `Mode::ALL`（`agent-core/src/schema.rs:27`）的 `as_str()`；
/// 登记侧是 [`SESSION_MODES`]。集合相等 ⇒ 两边**同时**增删项即红。
#[test]
fn session_mode_values_match_the_registered_contract() {
    let implemented: BTreeSet<String> = agent_core::schema::Mode::ALL
        .iter()
        .map(|m| (*m).as_str().to_string())
        .collect();
    let contracted = registered(SESSION_MODES);
    assert_eq!(
        implemented, contracted,
        "Mode 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `PermissionMode` 两值必须与实现侧**集合相等**。
#[test]
fn permission_mode_values_match_the_registered_contract() {
    let implemented = wire_values![PermissionMode::ReadOnly, PermissionMode::Full];
    let contracted = registered(PERMISSION_MODES);
    assert_eq!(
        implemented, contracted,
        "PermissionMode 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `SessionStatus` 四值必须与实现侧**集合相等**。
///
/// `SessionStatus` 没有 `as_str()` ⇒ 取 **serde 序列化**结果（`snake_case`）。
#[test]
fn session_status_values_match_the_registered_contract() {
    let implemented = wire_values![
        SessionStatus::Running,
        SessionStatus::Idle,
        SessionStatus::Done,
        SessionStatus::Failed
    ];
    let contracted = registered(SESSION_STATUSES);
    assert_eq!(
        implemented, contracted,
        "SessionStatus 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}

/// §3 的 `Role` 四值必须与实现侧**集合相等**。
///
/// `Role` 没有 `as_str()` ⇒ 取 **serde 序列化**结果（`snake_case`）。
#[test]
fn message_role_values_match_the_registered_contract() {
    let implemented = wire_values![
        Role::User,
        Role::Assistant,
        Role::ToolCall,
        Role::ToolResult
    ];
    let contracted = registered(MESSAGE_ROLES);
    assert_eq!(
        implemented, contracted,
        "Role 取值面与契约登记不一致：实现 {implemented:?} vs 登记 {contracted:?}"
    );
}
