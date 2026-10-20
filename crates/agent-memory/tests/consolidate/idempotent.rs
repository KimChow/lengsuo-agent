//! 幂等水位：同输入恒同输出；只增量追加新日期节。

use agent_memory::consolidate;

use crate::support::{now, section_body, section_headers, ymd, TempDir};

fn seed_three(tmp: &TempDir) -> [chrono::NaiveDate; 3] {
    let dates = [ymd(2026, 10, 1), ymd(2026, 10, 2), ymd(2026, 10, 3)];
    tmp.write_daily(dates[0], "- 09:00 第一天\n正文\n");
    tmp.write_daily(dates[1], "- 09:00 第二天\n正文\n");
    tmp.write_daily(dates[2], "- 09:00 第三天\n正文\n");
    dates
}

#[test]
fn running_twice_is_byte_identical() {
    let tmp = TempDir::new("twice");
    seed_three(&tmp);

    let first = consolidate(&tmp.workspace(), now()).expect("首次聚合");
    let after_first = tmp.consolidated();
    assert_eq!(first.days_appended, 3);

    let second = consolidate(&tmp.workspace(), now()).expect("二次聚合");
    let after_second = tmp.consolidated();

    assert_eq!(second.days_scanned, 3);
    assert_eq!(second.days_appended, 0, "已有水位 ⇒ 不得重复追加");
    assert_eq!(second.days_skipped, 3);
    assert_eq!(second.days_pruned, 0);
    assert_eq!(
        after_first.as_bytes(),
        after_second.as_bytes(),
        "同输入必须逐字节相同"
    );
}

#[test]
fn fourth_day_appends_only_one_section_and_keeps_previous_bytes() {
    let tmp = TempDir::new("incremental");
    seed_three(&tmp);
    consolidate(&tmp.workspace(), now()).unwrap();
    let before = tmp.consolidated();

    let d4 = ymd(2026, 10, 4);
    tmp.write_daily(d4, "- 12:00 第四天\n正文\n");
    let report = consolidate(&tmp.workspace(), now()).expect("增量聚合");
    let after = tmp.consolidated();

    assert_eq!(report.days_appended, 1, "只新增第 4 天");
    assert_eq!(report.days_skipped, 3, "前 3 天命中水位");
    assert!(
        after.starts_with(&before),
        "前 3 节必须逐字节不变\nbefore={before:?}\nafter={after:?}"
    );
    assert_eq!(
        section_headers(&after),
        ["2026-10-01", "2026-10-02", "2026-10-03", "2026-10-04"]
    );
    assert_eq!(section_body(&after, "2026-10-04"), ["- 12:00 第四天"]);
}
