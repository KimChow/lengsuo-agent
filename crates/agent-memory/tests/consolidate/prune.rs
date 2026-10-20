//! 30 天窗口删除：已入册的旧日文件删除，未入册的旧日文件保留。

use agent_memory::consolidate;
use chrono::Duration;

use crate::support::{now, section_headers, TempDir, NOW_DATE};

fn days_ago(n: i64) -> chrono::NaiveDate {
    NOW_DATE - Duration::days(n)
}

fn enrolled_section(date: chrono::NaiveDate) -> String {
    format!("# 项目记忆蒸馏\n\n## {date}\n- 09:00 既有摘要\n")
}

#[test]
fn old_enrolled_days_are_pruned_but_old_unenrolled_day_is_kept() {
    let tmp = TempDir::new("prune-window");
    let enrolled = [days_ago(40), days_ago(41), days_ago(42)];
    let unenrolled = days_ago(43);

    for date in enrolled.iter().copied().chain([unenrolled]) {
        tmp.write_daily(date, "- 09:00 旧摘要\n正文\n");
    }
    let mut seed = String::new();
    for date in enrolled {
        seed.push_str(&enrolled_section(date));
    }
    tmp.write_consolidated(&seed);

    let report = consolidate(&tmp.workspace(), now()).expect("窗口删除应成功");

    assert_eq!(report.days_pruned, 3, "3 个已入册旧日文件应删除");
    assert_eq!(report.days_skipped, 3);
    assert_eq!(report.days_appended, 1, "未入册旧日文件本次聚合入册");
    for date in enrolled {
        assert!(!tmp.exists(date), "{date} 已入册且超 30 天，应删除");
    }
    assert!(
        tmp.exists(unenrolled),
        "{unenrolled} 运行前未入册，绝不删除"
    );

    let text = tmp.consolidated();
    let headers = section_headers(&text);
    assert!(
        headers.contains(&unenrolled.to_string()),
        "未入册旧日文件应在本轮入册: {headers:?}"
    );
}

#[test]
fn recent_enrolled_day_is_not_pruned() {
    let tmp = TempDir::new("prune-recent");
    let recent = days_ago(10);
    tmp.write_daily(recent, "- 09:00 近期摘要\n");
    tmp.write_consolidated(&enrolled_section(recent));

    let report = consolidate(&tmp.workspace(), now()).expect("窗口删除应成功");

    assert_eq!(report.days_pruned, 0, "未超 30 天不得删除");
    assert!(tmp.exists(recent), "近期日文件必须保留");
}

#[test]
fn exactly_thirty_days_is_not_pruned() {
    let tmp = TempDir::new("prune-boundary");
    let boundary = days_ago(30);
    tmp.write_daily(boundary, "- 09:00 边界摘要\n");
    tmp.write_consolidated(&enrolled_section(boundary));

    let report = consolidate(&tmp.workspace(), now()).unwrap();

    assert_eq!(report.days_pruned, 0, "`> 30 天` 不含恰好 30 天");
    assert!(tmp.exists(boundary));
}
