//! 步 4：声明的工具与指令是否超出授权面。
//!
//! 参考件：09 册 §2.1（`allowed-tools` 限定该技能可用工具）与本仓
//! `SKILLS-SPEC` 的 fail-closed 语义——技能自报内容不能扩大宿主授权。
//!
//! 判据（相对宿主已授权工具集 `host_tools`）：
//! - 技能声明的工具不在宿主授权内 ⇒ **P1**（越权声明，需用户确认）。
//! - 授权声明不可靠解析（`policy.rs` 记入 `fail_closed_fields`）⇒ **P1**。
//! - 正文里出现越权指令（要求使用未授权工具）⇒ **P1**。
//!
//! 没有宿主授权面可比对（`host_tools` 为 `None`）⇒ 跳过本步，不臆造结论。

use std::collections::BTreeSet;

use super::{contains_needle, read_text, AuditStep, Finding, Package, RiskLevel};

/// 越权指令关键词：正文里出现即提示（配合宿主授权面判定为越权）。
/// 越权指令关键词：正文里出现即提示（配合宿主授权面判定为越权）。
///
/// 只收「绕过权限/审批」类措辞；**提示注入**类措辞归步 5（P2），不在此重复
/// 登记——否则同一段正文会因是否传入 `host_tools` 而落在不同档位。
const OVERREACH_KEYWORDS: [&str; 4] = [
    "bypass permissions",
    "skip approval",
    "跳过审批",
    "无需用户确认",
];

/// 步 4：授权面检查。
pub(crate) fn audit(package: &Package, host_tools: Option<&BTreeSet<String>>) -> Vec<Finding> {
    let Some(host) = host_tools else {
        return Vec::new();
    };
    let mut findings = Vec::new();

    // 用既有解析器取限制元数据（不改 `parse.rs`/`policy.rs` 语义）。
    let (skill, policy) = match crate::parse::parse_skill_with_policy(&package.dir) {
        Ok(parsed) => parsed,
        Err(_) => {
            // 解析失败本身由 `catalog` 拒绝加载，审计不重复记结论。
            return findings;
        }
    };

    // 声明的工具超出宿主授权面 ⇒ P1。
    for tool in &policy.declared_tools {
        if !host.contains(tool.as_str()) {
            findings.push(Finding {
                level: RiskLevel::P1,
                step: AuditStep::Scope,
                location: crate::parse::SKILL_FILE.to_string(),
                message: format!("技能声明的工具 {tool} 超出宿主授权面"),
            });
        }
    }

    // 授权声明不可靠解析 ⇒ P1（fail-closed 降级为不授权任何工具）。
    for field in &policy.fail_closed_fields {
        findings.push(Finding {
            level: RiskLevel::P1,
            step: AuditStep::Scope,
            location: crate::parse::SKILL_FILE.to_string(),
            message: format!("授权声明不可靠解析，已按 fail-closed 降级: {field}"),
        });
    }

    // 正文里的越权指令 ⇒ P1。
    if let Some(text) = package.skill_md.as_ref().and_then(|path| read_text(path)) {
        for keyword in OVERREACH_KEYWORDS {
            if contains_needle(&text, keyword) {
                findings.push(Finding {
                    level: RiskLevel::P1,
                    step: AuditStep::Scope,
                    location: skill.path.display().to_string(),
                    message: format!("正文含越权指令: {keyword}"),
                });
            }
        }
    }

    findings
}
