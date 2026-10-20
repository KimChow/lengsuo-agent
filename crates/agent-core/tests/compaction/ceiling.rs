//! 压缩上限公式（`calculate_ceiling`）的验收。
//!
//! 逐条对照 opencode `compaction.ts:911-918`：buffer 分支、10%/RESERVE_MIN
//! 取大者、小窗口只减 10%、窗口未知为无穷。

use agent_core::runtime::compaction::calculate_ceiling;

use super::support::{RESERVE_MIN, UNKNOWN_WINDOW};

/// 窗口 200_000、buffer 16_000 ⇒ 184_000（参考 `window - buffer` 分支）。
#[test]
fn ceiling_with_buffer_subtracts_buffer() {
    assert_eq!(
        calculate_ceiling(Some(200_000), Some(RESERVE_MIN)),
        184_000.0,
        "有 buffer ⇒ 窗口减 buffer（184_000）"
    );
}

/// 窗口 200_000、无 buffer ⇒ 200_000 - max(10% = 20_000, RESERVE_MIN = 16_000) = 180_000。
#[test]
fn ceiling_without_buffer_keeps_the_larger_of_ten_percent_and_reserve() {
    assert_eq!(
        calculate_ceiling(Some(200_000), None),
        180_000.0,
        "大窗口 ⇒ 留 10%（20_000 > RESERVE_MIN 16_000）"
    );
    // 窗口 40_000：10% = 4_000 < RESERVE_MIN ⇒ 留 16_000 ⇒ 24_000。
    assert_eq!(
        calculate_ceiling(Some(40_000), None),
        24_000.0,
        "窗口 >= 2*RESERVE_MIN ⇒ 留 RESERVE_MIN"
    );
}

/// 窗口 20_000 < 2 * RESERVE_MIN ⇒ 只减 10% = 18_000。
#[test]
fn ceiling_small_window_only_reserves_ten_percent() {
    assert_eq!(
        calculate_ceiling(Some(20_000), None),
        18_000.0,
        "小窗口给不出 RESERVE_MIN ⇒ 只留 10%"
    );
}

/// 窗口未知（`None` 或 `<= 0`）⇒ 无穷（自动压缩永不触发）。
#[test]
fn ceiling_unknown_window_is_infinite() {
    assert!(
        !calculate_ceiling(None, None).is_finite(),
        "未知窗口 ⇒ 无穷"
    );
    assert!(
        !calculate_ceiling(Some(0), None).is_finite(),
        "窗口 0 ⇒ 无穷"
    );
    assert!(
        !calculate_ceiling(Some(-1), None).is_finite(),
        "负窗口 ⇒ 无穷"
    );
    // 窗口未知时，压缩请求改用 UNKNOWN_WINDOW 假定的窗口（参考 `compaction.ts:222-225`）。
    let unknown = calculate_ceiling(Some(UNKNOWN_WINDOW), None);
    assert_eq!(unknown, 180_000.0, "UNKNOWN_WINDOW 走与普通窗口同一条公式");
}
