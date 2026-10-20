//! 正例：两份真实样本通过校验 + 描述长度按字符计（非字节）。
//!
//! 样本来源见 `support.rs` 的成员表（本地对照资料，不入库）。
use super::support::*;

// ---------------------------------------------------------------------------
// 正例：两份真实样本
// ---------------------------------------------------------------------------

#[test]
fn real_sample_single_agent_passes_validation() {
    let temp = TempDir::new("senior");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_eq!(
        report.checked, 26,
        "应跑满 26 项（§17 的 1–25，其中 #3 与 #10 各拆两条子项）"
    );
    assert!(
        report.blocking().is_empty(),
        "单专家样本应无阻断发现，实际：{:#?}",
        report.blocking()
    );
    assert!(report.is_ok());
}

#[test]
fn real_sample_team_passes_validation() {
    let temp = TempDir::new("pandaai");
    let manifest = build_team(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    assert_eq!(report.checked, 26);
    assert!(
        report.blocking().is_empty(),
        "团队型样本应无阻断发现，实际：{:#?}",
        report.blocking()
    );
    assert!(report.is_ok());
}

#[test]
fn real_samples_pass_through_fail_closed_entry_point() {
    let temp = TempDir::new("failclosed");
    let manifest = build_team(temp.path());
    let report = Validator::new()
        .validate_dir(temp.path(), &manifest)
        .expect("真实样本应通过 fail-closed 入口");
    assert!(report.is_ok());
    assert_eq!(report.checked, 26);
}

/// 两份样本的**偏离项**必须是 Warning（不是 Error）——否则真实样本被判死。
#[test]
fn sample_violations_are_warnings_not_errors() {
    let temp = TempDir::new("warn");
    let manifest = build_team(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    let warnings: BTreeMap<u8, String> = report
        .warnings()
        .iter()
        .map(|finding| (finding.item(), finding.code().to_string()))
        .collect();
    // #3b 目录名 ≠ name（样本目录 `pandaai-team` vs name `pandaai-ai-quant-research-team`）
    assert_eq!(
        warnings.get(&item::NAME_MATCHES_DIR).map(String::as_str),
        Some("NAME_DIR_MISMATCH")
    );
    // #12 描述 51 字（超出 [40,50]）
    assert_eq!(
        warnings.get(&item::DESCRIPTION_LENGTH).map(String::as_str),
        Some("DESCRIPTION_LENGTH")
    );
    // #17 团队型 profession ≠ displayName
    assert_eq!(
        warnings
            .get(&item::TEAM_PROFESSION_MATCHES_DISPLAY)
            .map(String::as_str),
        Some("TEAM_PROFESSION_DISPLAY_MISMATCH")
    );
}

/// #12 的计数口径：`chars().count()`（规格 §6 铁律 5 明写）。
#[test]
fn description_length_counts_chars_not_bytes() {
    let temp = TempDir::new("desclen");
    let manifest = build_single(temp.path());
    let report = Validator::new().report_dir(temp.path(), &manifest);
    let finding = finding_for(&report, item::DESCRIPTION_LENGTH).expect("单专家样本 39 字应报");
    // 39 个字符（113 字节）⇒ 报 39，不是字节数。
    let Some(agent_expert::ExpertError::DescriptionLength { count }) = finding.expert_error()
    else {
        panic!("应是 DESCRIPTION_LENGTH 变体");
    };
    assert_eq!(*count, 39);
}
