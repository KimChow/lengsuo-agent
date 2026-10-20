//! 三处数量关系的**表格化**（§8.1 实测：`agents` N+1 项、`memberAgents` N 项、
//! `members` N+1 项，N = 团员数）。
//!
//! 视图入口 [`TeamView::check_consistency`] 负责判定；本文件只把计数摊成
//! 一张确定性表，便于文档与测试用同一张表说话（规则本身仍在
//! [`crate::validate::team`]，不在这里重写）。

use crate::manifest::{ExpertManifest, MemberRole};

use super::TeamView;

/// 一处数量关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relation {
    /// 关系名（文档 / 测试断言用）。
    pub name: &'static str,
    /// 左侧实测值。
    pub left: usize,
    /// 右侧期望值。
    pub right: usize,
}

impl Relation {
    /// 是否成立。
    pub fn holds(&self) -> bool {
        self.left == self.right
    }
}

/// 一份 team 型 manifest 的三处数量关系实测表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// `teamInfo.memberAgents.len()`（不含主理人）。
    pub member_agents: usize,
    /// `members.len()`（含主理人）。
    pub members: usize,
    /// `agents.len()`（主理人 MD + 团员 MD）。
    pub agents: usize,
    /// `members` 中 `role == lead` 的项数。
    pub leads: usize,
}

impl Table {
    /// 从 manifest 实测。
    pub fn of(manifest: &ExpertManifest) -> Self {
        Self {
            member_agents: manifest
                .team_info
                .as_ref()
                .map_or(0, |team| team.member_agents.len()),
            members: manifest.members.len(),
            agents: manifest.agents.len(),
            leads: manifest
                .members
                .iter()
                .filter(|member| member.role == MemberRole::Lead)
                .count(),
        }
    }

    /// 三处数量关系（顺序与规格 §8.1 表述一致）。
    ///
    /// - `agents == members`：主理人 MD + 每个成员一份 MD；
    /// - `memberAgents == members - leads`：`memberAgents` **不含**主理人；
    /// - `leads == 1`：`members` 恰一主理人。
    pub fn relations(&self) -> [Relation; 3] {
        [
            Relation {
                name: "agents == members",
                left: self.agents,
                right: self.members,
            },
            Relation {
                name: "memberAgents == members - leads",
                left: self.member_agents,
                right: self.members.saturating_sub(self.leads),
            },
            Relation {
                name: "members 恰一 lead",
                left: self.leads,
                right: 1,
            },
        ]
    }

    /// 三处数量关系是否全部成立。
    pub fn holds(&self) -> bool {
        self.relations().iter().all(Relation::holds)
    }
}

impl TeamView<'_> {
    /// 三处数量关系的实测表。
    ///
    /// ⚠️ 只是计数；「恰一 lead 且 id == `agentName`」的完整判定仍以
    /// [`TeamView::check_consistency`] 为准（表里不含 id 交叉判定）。
    pub fn table(&self) -> Table {
        Table::of(self.manifest())
    }
}
