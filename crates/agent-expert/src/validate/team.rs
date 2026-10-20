//! 团队型校验（§17 #15–#18）：主理人、成员集合、profession、通用名。
//!
//! 规格 §8.1「结构（照抄样本）」：
//!
//! ```text
//! expertType: "team"
//! agentName:  "pd-ai-quant-research-team"        # 主理人 MD 文件名（含团名前缀）
//! teamInfo:   { leadAgent: <agentName>, memberAgents: [团员id…] }   # 不含主理人
//! members[]:  [ {id, name, profession, avatar, role} ]  # 含主理人(role=lead)
//! agents[]:   主理人 MD + 各团员 MD（共 6 项）
//! ```
//!
//! 调研 §8.1 实测：「`agents` 6 项、`teamInfo.memberAgents` 5 项、`members`
//! 6 项（含 lead）——**三处数量关系是校验核心**」。
//!
//! | # | 检查什么 | 失败码 | 级别 | 本文件函数 |
//! |---|---|---|---|---|
//! | 15 | team：`members` 恰一个 `role=="lead"` 且 id == `agentName` | `TEAM_LEAD_MISSING` | Error | [`team_lead_is_unique`] |
//! | 16 | team：`teamInfo.memberAgents` == 非 lead 成员 id 集合（且不含 lead） | `TEAM_MEMBERS_MISMATCH` | Error | [`team_member_agents_match_members`] |
//! | 17 | team：`profession` == `displayName`（en/zh） | `TEAM_PROFESSION_DISPLAY_MISMATCH` | ⚠️ **Warning**（偏离 D-C） | [`team_profession_matches_display_name`] |
//! | 18 | team：`agentName` 不落入通用名黑名单 | `GENERIC_AGENT_NAME` | Error | [`team_agent_name_not_generic`] |
//!
//! **四项都只在 `expertType == "team"` 时生效**：`agent` 型包的
//! `teamInfo` / `members` 本就为空（§17 #5 已判结构一致），再判团队项没有
//! 意义（且会让单专家样本误报）。

use std::collections::BTreeSet;

use crate::error::ExpertError;
use crate::manifest::{ExpertType, MemberRole};
use crate::validate::ValidateContext;
use crate::validate_errors::ItemError;

/// §6 铁律 7 的通用名黑名单（主理人文件名禁用的词）。
///
/// 规格 §6 铁律 7：「type=team 时 `agentName` 须命中通用名黑名单
/// （`team-lead`/`lead`/`main`/`agent`）」。原文是「不落入黑名单」——即这四个
/// 词**不能**用作主理人名。
pub(crate) const GENERIC_AGENT_NAMES: &[&str] = &["team-lead", "lead", "main", "agent"];

/// §17 #15：team 型 `members` 中恰有一个 `role=="lead"`，且其 id == `agentName`。
///
/// §6 铁律 8：「`members` 中恰有一个 `role=="lead"` 且 id == `agentName`」。
pub fn team_lead_is_unique(ctx: &ValidateContext) -> Result<(), ItemError> {
    if ctx.manifest.expert_type != ExpertType::Team {
        return Ok(());
    }
    let manifest = ctx.manifest;
    let leads: Vec<&str> = manifest
        .members
        .iter()
        .filter(|member| member.role == MemberRole::Lead)
        .map(|member| member.id.as_str())
        .collect();
    if leads.len() != 1 {
        return Err(ItemError::Expert(ExpertError::TeamLeadMissing));
    }
    if leads[0] != manifest.agent_name {
        return Err(ItemError::Expert(ExpertError::TeamLeadMissing));
    }
    Ok(())
}

/// §17 #16：team 型 `teamInfo.memberAgents` == 非 lead 成员 id 集合，且**不含
/// lead**（§6 铁律 8 的双向校验）。
pub fn team_member_agents_match_members(ctx: &ValidateContext) -> Result<(), ItemError> {
    if ctx.manifest.expert_type != ExpertType::Team {
        return Ok(());
    }
    let manifest = ctx.manifest;
    let Some(team) = manifest.team_info.as_ref() else {
        // 缺 teamInfo 由 §17 #5 报（TYPE_STRUCTURE_MISMATCH），本项不重复报。
        return Ok(());
    };
    let declared: BTreeSet<&str> = team.member_agents.iter().map(String::as_str).collect();
    let expected: BTreeSet<&str> = manifest
        .members
        .iter()
        .filter(|member| member.role != MemberRole::Lead)
        .map(|member| member.id.as_str())
        .collect();
    if declared == expected && !declared.contains(manifest.agent_name.as_str()) {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::TeamMembersMismatch {
            expected: expected.into_iter().collect::<Vec<_>>().join(", "),
            actual: declared.into_iter().collect::<Vec<_>>().join(", "),
        }))
    }
}

/// §17 #17：team 型 `profession` == `displayName`（en/zh 两侧都相等）。
///
/// §6 铁律 9。
///
/// ⚠️ **偏离 D-C**：真实样本 `pandaai-team` 的 `profession.zh`
/// （`PD-AI量化研究专家团`）≠ `displayName.zh`
/// （`研报复现因子挖掘回测审计`），**且人格 MD 的 frontmatter 与之一致**
/// （`displayName` / `profession` 两个块都写 `研报复现因子挖掘回测审计` 与
/// `PD-AI量化研究专家团`）⇒ 判定项与样本事实直接矛盾，Error 会把真实样本
/// 判死 ⇒ 本项降为 **Warning**（详见偏离清单）。
pub fn team_profession_matches_display_name(ctx: &ValidateContext) -> Result<(), ItemError> {
    if ctx.manifest.expert_type != ExpertType::Team {
        return Ok(());
    }
    let manifest = ctx.manifest;
    if manifest.profession == manifest.display_name {
        Ok(())
    } else {
        Err(ItemError::Expert(
            ExpertError::TeamProfessionDisplayMismatch,
        ))
    }
}

/// §17 #18：team 型 `agentName` 不落入通用名黑名单。
///
/// §6 铁律 7：「主理人文件名带专家团前缀，禁 `team-lead`」。
pub fn team_agent_name_not_generic(ctx: &ValidateContext) -> Result<(), ItemError> {
    if ctx.manifest.expert_type != ExpertType::Team {
        return Ok(());
    }
    let name = &ctx.manifest.agent_name;
    if GENERIC_AGENT_NAMES.contains(&name.as_str()) {
        Err(ItemError::Expert(ExpertError::GenericAgentName {
            name: name.clone(),
        }))
    } else {
        Ok(())
    }
}
