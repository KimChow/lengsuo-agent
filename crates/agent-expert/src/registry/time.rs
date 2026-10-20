//! 注册表时间戳（§4.2 `registeredAt`）。
//!
//! 与 `registry.rs` 分文件：本文件只做 UTC 时间格式化（不引 `chrono` 依赖）。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 UTC 时间（RFC3339，毫秒精度）——`registeredAt` 用。
pub(super) fn now_rfc3339_millis() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        now.subsec_millis()
    )
}

/// 天数 → 公历年月日（Howard Hinnant `civil_from_days` 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}
