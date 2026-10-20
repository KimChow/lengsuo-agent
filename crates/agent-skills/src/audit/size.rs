//! 步 3：总体字节上限（单技能包总量、文件数上限）。
//!
//! 参考件：09 册 §5.1 安装生命周期（上传 ≤100MB 走腾讯 TI，见 16 册）与
//! 18 册 §9.1「审计 `SKILL.md` 与全部捆绑文件」——总量必须设上限防 DoS。
//!
//! 级别：**P1**（体积超限不是恶意证据，但需要用户确认后放行）。

use super::{AuditStep, Finding, Package, RiskLevel};

/// 步 3：包总量 / 文件数 / 单文件体积上限检查（只读元数据）。
pub(crate) fn audit(package: &Package, limits: &super::AuditLimits) -> Vec<Finding> {
    let mut findings = Vec::new();

    // `SKILL.md` 自身体积：与 `parse` 的硬上限一致。
    let skill_md_bytes = package
        .skill_md
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .unwrap_or(0);
    if skill_md_bytes > limits.max_skill_md_bytes {
        findings.push(Finding {
            level: RiskLevel::P1,
            step: AuditStep::Size,
            location: crate::parse::SKILL_FILE.to_string(),
            message: format!(
                "SKILL.md 超过体积上限（{} > {} 字节）",
                skill_md_bytes, limits.max_skill_md_bytes
            ),
        });
    }

    // 单文件体积上限。
    for file in &package.files {
        if file.bytes > limits.max_file_bytes {
            findings.push(Finding {
                level: RiskLevel::P1,
                step: AuditStep::Size,
                location: file.relative.clone(),
                message: format!(
                    "捆绑文件超过单文件上限（{} > {} 字节）",
                    file.bytes, limits.max_file_bytes
                ),
            });
        }
    }

    // 文件数上限（含 SKILL.md 本身）。
    let file_count = package.files.len() + 1;
    if file_count > limits.max_package_files {
        findings.push(Finding {
            level: RiskLevel::P1,
            step: AuditStep::Size,
            location: "技能包".to_string(),
            message: format!(
                "文件数超过上限（{} > {} 个）",
                file_count, limits.max_package_files
            ),
        });
    }

    // 总体积上限。
    let total_bytes = package.files.iter().map(|f| f.bytes).sum::<u64>() + skill_md_bytes;
    if total_bytes > limits.max_package_bytes {
        findings.push(Finding {
            level: RiskLevel::P1,
            step: AuditStep::Size,
            location: "技能包".to_string(),
            message: format!(
                "技能包总体积超过上限（{} > {} 字节）",
                total_bytes, limits.max_package_bytes
            ),
        });
    }

    findings
}
