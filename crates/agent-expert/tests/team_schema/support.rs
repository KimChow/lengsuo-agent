//! 团队 schema 测试的共享工具：夹具载入 + 错误码判定。
//!
//! 夹具是**裁过的真实样本结构**（只留结构字段，见各文件）：两份正例
//! （`team` 型 / `agent` 型）与三份反例（无 lead / 两个 lead / `memberAgents`
//! 多一个）。反例由正例机械改一处得到，便于断言「只命中对应错误码」。

pub use agent_expert::{
    validate::item, ExpertManifest, Handoff, HandoffStatus, MemberHandoff, Severity, TaskPacket,
    TeamView, TeamWorkflow,
};

pub const TEAM: &str = include_str!("../fixtures/team/team.example.plugin.json");
pub const AGENT: &str = include_str!("../fixtures/team/agent.example.plugin.json");
pub const NO_LEAD: &str = include_str!("../fixtures/team/no-lead.plugin.json");
pub const TWO_LEADS: &str = include_str!("../fixtures/team/two-leads.plugin.json");
pub const EXTRA_MEMBER_AGENT: &str =
    include_str!("../fixtures/team/extra-member-agent.plugin.json");

/// 解析一份夹具（夹具本身是合法 JSON，解析失败即测试失败）。
pub fn parse(text: &str) -> ExpertManifest {
    ExpertManifest::from_json_str(text).expect("夹具应可解析")
}

/// 视图在 `check_consistency` 里报出的错误码（顺序同报告顺序）。
pub fn codes(view: &TeamView<'_>) -> Vec<&'static str> {
    view.check_consistency()
        .iter()
        .map(|err| err.code())
        .collect()
}

/// 视图是否报了某个错误码。
pub fn reports(view: &TeamView<'_>, code: &str) -> bool {
    codes(view).contains(&code)
}

/// 一条固定时刻的 RFC3339 文本（测试不读系统时钟）。
pub const T0: &str = "2026-10-21T00:00:00Z";
