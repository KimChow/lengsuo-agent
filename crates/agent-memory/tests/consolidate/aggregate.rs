//! 机械聚合：空目录、日期升序、只取摘要行、节内去重。

use agent_memory::{consolidate, CAP_CONSOLIDATED};

use crate::support::{now, section_body, section_headers, ymd, TempDir};

#[test]
fn empty_workspace_writes_header_without_date_sections() {
    let tmp = TempDir::new("empty");
    let report = consolidate(&tmp.workspace(), now()).expect("空目录应成功");

    assert_eq!(report.days_scanned, 0);
    assert_eq!(report.days_appended, 0);
    assert_eq!(report.days_skipped, 0);
    assert_eq!(report.days_pruned, 0);
    assert_eq!(report.consolidated_path, tmp.consolidated_file());

    let text = tmp.consolidated();
    assert!(text.contains("项目记忆蒸馏"), "应有稳定标题: {text:?}");
    assert!(
        section_headers(&text).is_empty(),
        "空目录不得产出日期节: {text:?}"
    );
}

#[test]
fn three_days_are_sorted_and_only_summary_lines_kept() {
    let tmp = TempDir::new("sort");
    let d1 = ymd(2026, 10, 1);
    let d2 = ymd(2026, 10, 2);
    let d3 = ymd(2026, 10, 3);
    // 乱序写入，输出仍须按日期升序。
    tmp.write_daily(d3, "- 11:00 第三天\n正文不应入选\n");
    tmp.write_daily(d1, "- 09:00 第一天\n正文不应入选\n");
    tmp.write_daily(d2, "- 10:00 第二天\n正文不应入选\n");

    let report = consolidate(&tmp.workspace(), now()).expect("聚合应成功");
    assert_eq!(report.days_scanned, 3);
    assert_eq!(report.days_appended, 3);
    assert_eq!(report.days_pruned, 0);

    let text = tmp.consolidated();
    assert_eq!(
        section_headers(&text),
        ["2026-10-01", "2026-10-02", "2026-10-03"],
        "日期节必须升序"
    );
    assert_eq!(section_body(&text, "2026-10-01"), ["- 09:00 第一天"]);
    assert_eq!(section_body(&text, "2026-10-03"), ["- 11:00 第三天"]);
}

#[test]
fn no_convention_line_falls_back_to_first_non_empty_line() {
    let tmp = TempDir::new("fallback");
    let date = ymd(2026, 10, 5);
    tmp.write_daily(date, "\n首行摘要（无时间前缀）\n第二行\n");

    consolidate(&tmp.workspace(), now()).unwrap();
    let text = tmp.consolidated();
    assert_eq!(
        section_body(&text, "2026-10-05"),
        ["- 首行摘要（无时间前缀）"],
        "无约定格式时回退首个非空行，并统一加 `- `"
    );
}

#[test]
fn oversized_output_evicts_oldest_sections_whole_but_keeps_newest() {
    let tmp = TempDir::new("cap-evict");
    let oldest = ymd(2026, 10, 10);
    let newest = ymd(2026, 10, 11);
    let long_summary = "甲".repeat(20_000); // 两节约 120KB，必超 64KiB 上限
    tmp.write_daily(oldest, &format!("- 09:00 {long_summary}\n"));
    tmp.write_daily(newest, &format!("- 09:00 {long_summary}\n"));

    consolidate(&tmp.workspace(), now()).expect("超限应淘汰而非失败");
    let text = tmp.consolidated();

    assert!(
        text.len() <= CAP_CONSOLIDATED,
        "淘汰后必须回到上限内: {} > {CAP_CONSOLIDATED}",
        text.len()
    );
    assert_eq!(
        section_headers(&text),
        ["2026-10-11"],
        "最旧日期节应被整节淘汰"
    );
    let body = section_body(&text, "2026-10-11");
    assert_eq!(body.len(), 1, "最新节不得被截成半节");
    assert_eq!(
        body[0].chars().count(),
        "- 09:00 ".chars().count() + 20_000,
        "最新节内容必须完整保留"
    );
}

#[test]
fn identical_lines_within_a_day_are_deduplicated() {
    let tmp = TempDir::new("dedup");
    let date = ymd(2026, 10, 6);
    tmp.write_daily(date, "- 09:00 重复摘要\n- 09:00 重复摘要\n- 10:00 另一条\n");

    consolidate(&tmp.workspace(), now()).unwrap();
    let text = tmp.consolidated();
    let body = section_body(&text, "2026-10-06");
    assert_eq!(
        body,
        ["- 09:00 重复摘要", "- 10:00 另一条"],
        "节内完全相同的行只留一次"
    );
}
