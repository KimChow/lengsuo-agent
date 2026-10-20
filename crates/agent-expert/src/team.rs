//! 专家团（`expertType == "team"`）结构视图与团队运行契约（卡 9）。
//!
//! 规格：`harness-expert.md` §8（team 型结构：§8.1 结构 / §8.2 主理人 vs
//! 团员 / §8.3 成员命名 / §8.4 团队运行）+ §17 #15–#18 + §6 铁律 7/8/9。
//!
//! 两层职责，**互不复制**：
//!
//! | 层 | 位置 | 做什么 |
//! |---|---|---|
//! | 单项判定 | `crate::validate::team`（卡 2 已实现，本卡**不改**） | §17 #15–#18，每条一项、每项一个错误码 |
//! | 聚合视图 | 本模块 [`TeamView`] | 把四个集合按「主理人 / 团员」两分法露出，并把三处数量关系统一聚成 `Vec<ItemError>` |
//!
//! [`TeamView::check_consistency`] 的规则来源是**校验器本身**：它用
//! （空目录 + 默认规则）的 [`ValidateContext`] 跑一遍 [`Validator::report_dir`]，
//! 再按 §17 编号（#15 / #16）取出团队项发现——规则只有一份，不重写。
//!
//! **只 schema 不运行**：团队运行所需的两份文件契约（`task_packet.json` /
//! `member_handoff.json`，§8.4）冻结在 [`packet`]；子代理 spawn、上下文
//! 隔离、看板、阶段 seal/verify **不在本模块**。

pub mod consistency;
pub mod packet;

use crate::manifest::{ExpertManifest, ExpertMember, ExpertType, MemberRole};
use crate::validate::item;
use crate::validate::{RuleSet, ValidateContext, Validator};
use crate::validate_errors::ItemError;

/// 专家团 manifest 的**只读视图**（借用 [`ExpertManifest`]，不复制、不改写）。
///
/// 样本实测的四个集合（§8.1）：
///
/// ```text
/// agentName  主理人 MD 文件名（含团名前缀）
/// teamInfo   { leadAgent: <agentName>, memberAgents: [团员 id…] }   # 不含主理人
/// members[]  { id, name, profession, avatar, role }                 # 含主理人（role=lead）
/// agents[]   主理人 MD + 各团员 MD                                   # 共 N+1 项
/// ```
///
/// **两分法**：主理人**只**从 [`TeamView::lead`] 取，团员**只**从
/// [`TeamView::members`] 取；`members()` 不包含主理人，避免调用方把
/// 主理人当团员再分一次。
#[derive(Debug, Clone, Copy)]
pub struct TeamView<'a> {
    manifest: &'a ExpertManifest,
}

impl<'a> TeamView<'a> {
    /// 以 manifest 构造只读视图（不做任何校验；非法结构也能构造，留给判定报告）。
    pub fn new(manifest: &'a ExpertManifest) -> Self {
        Self { manifest }
    }

    /// 底层 manifest。
    pub fn manifest(&self) -> &'a ExpertManifest {
        self.manifest
    }

    /// 主理人成员：`members` 中 `role == lead` 的**第一项**。
    ///
    /// 归属判定：`role` 字段是主理人的唯一结构标记（§8.1「members 含
    /// 主理人（role=lead）」）；`id == agentName` 以及「是否恰好一个」属
    /// §17 #15，由校验器负责，本视图只按 `role` 取人。
    pub fn lead(&self) -> Option<&'a ExpertMember> {
        self.manifest
            .members
            .iter()
            .find(|member| member.role == MemberRole::Lead)
    }

    /// 团员（`members` 中**不含**主理人的部分，保持 manifest 原顺序）。
    pub fn members(&self) -> impl Iterator<Item = &'a ExpertMember> + '_ {
        self.manifest
            .members
            .iter()
            .filter(|member| member.role != MemberRole::Lead)
    }

    /// 团员 id（同 [`TeamView::members`] 的顺序）。
    pub fn member_ids(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.members().map(|member| member.id.as_str())
    }

    /// 全部人格文件路径（`agents[]` 原顺序；§8.2 主理人与团员共用这一数组）。
    pub fn agent_paths(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.manifest.agents.iter().map(String::as_str)
    }

    /// 把三处数量关系聚成一份发现列表（**空 = 三个数量关系都成立**）。
    ///
    /// | 数量关系 | 来源 | 失败码 | §17 |
    /// |---|---|---|---|
    /// | `members` 恰一 lead（且 id == `agentName`） | 校验器 #15 | `TEAM_LEAD_MISSING` | #15 |
    /// | `teamInfo.memberAgents` == 非 lead 成员 id 集合 | 校验器 #16 | `TEAM_MEMBERS_MISMATCH` | #16 |
    /// | `agents.len() == members.len()`（主理人 MD + 每个成员 MD） | 本函数（见下） | `TEAM_MEMBERS_MISMATCH` | §8.1 实测 |
    ///
    /// 口径：
    /// - 非 team 型（单专家）没有团队语义 ⇒ 恒空；
    /// - §17 编号只取 [#15](item::TEAM_LEAD_UNIQUE) / [#16](item::TEAM_MEMBERS_MATCH)，
    ///   不掺入 #17（Warning）/ #18（命名）——它们是「团队名」检查，不是数量关系；
    /// - `agents.len() != members.len()` 是 §8.1 实测的第三处数量关系，§17 表
    ///   没有单独编号 ⇒ 复用 #16 的错误码 `TEAM_MEMBERS_MISMATCH`（同一族
    ///   「成员集合与声明不一致」）；
    /// - 缺 `teamInfo` 时 #16 校验函数**直接放行**（§17 #5 负责报
    ///   `TYPE_STRUCTURE_MISMATCH`）；视图层面 team 型缺 `teamInfo` 就不成立，
    ///   故本函数在此补报一条 `TEAM_MEMBERS_MISMATCH`。
    pub fn check_consistency(&self) -> Vec<ItemError> {
        if self.manifest.expert_type != ExpertType::Team {
            return Vec::new();
        }
        let mut errors: Vec<ItemError> = self
            .team_findings()
            .into_iter()
            .filter_map(|finding| finding.expert_error().cloned().map(ItemError::Expert))
            .collect();
        if self.manifest.team_info.is_none() {
            errors.push(ItemError::Expert(
                crate::error::ExpertError::TeamMembersMismatch {
                    expected: self.member_ids().collect::<Vec<_>>().join(", "),
                    actual: "teamInfo 缺失".to_string(),
                },
            ));
        }
        if self.manifest.agents.len() != self.manifest.members.len() {
            errors.push(ItemError::Expert(
                crate::error::ExpertError::TeamMembersMismatch {
                    expected: format!("agents 与 members 各 {} 项", self.manifest.members.len()),
                    actual: format!("agents {} 项", self.manifest.agents.len()),
                },
            ));
        }
        errors
    }

    /// 只跑团队两项（#15 / #16）的校验报告。
    ///
    /// 校验上下文：**空目录**（#15 / #16 不触盘）+ 默认规则 ⇒ `Validator`
    /// 跑完整 26 项后只留团队两项的发现。目录相关的项在空目录上会报，
    /// 但被编号过滤掉，不影响结果。
    fn team_findings(&self) -> Vec<crate::validate_errors::ValidateError> {
        let rules = RuleSet::new();
        let ctx = ValidateContext {
            dir: std::path::Path::new(""),
            manifest: self.manifest,
            rules: &rules,
        };
        Validator::new()
            .report_dir(ctx.dir, ctx.manifest)
            .findings
            .into_iter()
            .filter(|finding| {
                matches!(
                    finding.item(),
                    item::TEAM_LEAD_UNIQUE | item::TEAM_MEMBERS_MATCH
                )
            })
            .collect()
    }
}
