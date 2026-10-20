//! 团队运行的**文件契约**（规格 §8.4）：`task_packet.json`（主理人 → 团员）
//! 与 `member_handoff.json`（团员 → 主理人 / 下一团员）。
//!
//! ⛔ **只 schema 不运行**：本文件只冻结**数据结构 + 序列化契约**（字段、线
//! 格式、关联关系）；子代理 spawn、上下文隔离、看板、阶段 seal/verify 属于
//! 团队运行卡，**本仓多代理运行时尚未就绪 ⇒ 明确不做**。
//!
//! 与 §8.1 结构模型的关系：§8.1 描述「一个专家团包长什么样」，本文件描述
//! 「一次派发与回传长什么样」；两者共用成员 id 命名空间（`members[].id`）。
//!
//! 线格式口径与 `manifest.rs` 一致：Rust 侧 snake_case，线格式 camelCase
//! （`#[serde(rename_all = "camelCase")]`）。

use serde::{Deserialize, Serialize};

/// 团队工作流契约描述（挂在 `teamInfo.workflow` 上，默认 `None`）。
///
/// 描述「这个团用哪一版契约、契约文件在哪、跑哪几个阶段」——**不含任何
/// 运行期状态**（不记进度、不记看板）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamWorkflow {
    /// 契约版本（当前为 1）。
    pub schema_version: u32,
    /// 任务包契约文件的相对路径（包内）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_packet_contract: Option<String>,
    /// 交接包契约文件的相对路径（包内）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_contract: Option<String>,
    /// 阶段名列表（按执行顺序；空 = 未声明阶段）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
}

impl Default for TeamWorkflow {
    /// 默认契约：版本 1，无契约文件、无阶段。
    fn default() -> Self {
        Self {
            schema_version: 1,
            task_packet_contract: None,
            handoff_contract: None,
            stages: Vec::new(),
        }
    }
}

/// 任务派发包：主理人 → 团员（§8.4「为每个阶段建 task_packet.json」）。
///
/// **只放引用，不放正文**：`context_refs` / `acceptance` 里是路径与判定条目，
/// 大段上下文由团员按路径自行读取（上下文隔离的前提：主理人不把会话正文
/// 塞进任务包）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPacket {
    /// 任务包 id（团内唯一；交接包靠它回指）。
    pub packet_id: String,
    /// 团 id（= 专家包 `name`）。
    pub team_id: String,
    /// 派发方 id（主理人 id = `members[]` 中 `role=lead` 的那项 id）。
    pub from_member: String,
    /// 接收方 id（团员 id）。
    pub to_member: String,
    /// 单句目标（**非空**：空目标的任务包没有判定依据）。
    pub objective: String,
    /// 上下文引用（路径 / id；**不内联大段上下文**）。
    #[serde(default)]
    pub context_refs: Vec<String>,
    /// 验收条目（可判定的检查项；**非空**）。
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// 创建时间（RFC3339 文本）。
    pub created_at: String,
}

/// 交接回传包：团员 → 主理人（或下一团员）（§8.4「成员写业务产物 +
/// member_handoff.json」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberHandoff {
    /// 回指的任务包 id（**必须**等于某个 `TaskPacket.packet_id`）。
    pub packet_id: String,
    /// 回传方 id（团员 id）。
    pub from_member: String,
    /// 交接状态。
    pub status: HandoffStatus,
    /// 结论摘要（一句话结论 + 关键结论）。
    pub summary: String,
    /// 产物路径列表（相对团包根）。
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// 未决问题（需要主理人或人工拍板的事项）。
    #[serde(default)]
    pub open_questions: Vec<String>,
    /// 创建时间（RFC3339 文本）。
    pub created_at: String,
}

/// `MemberHandoff` 的短名（规格把交接包简称为「Handoff」）。
pub type Handoff = MemberHandoff;

/// 交接状态（线格式：`done` / `blocked` / `needsHuman` / `unknown`）。
///
/// - `done`：完成，可进入封存判定；
/// - `blocked`：被阻断（缺前置、缺授权、外部依赖不可用）；
/// - `needsHuman`：需人工拍板；
/// - `unknown`：线格式出现未知取值时的兜底（向前兼容；**不视为完成**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffStatus {
    /// 完成。
    Done,
    /// 被阻断。
    Blocked,
    /// 需人工介入。
    NeedsHuman,
    /// 未知取值兜底。
    Unknown,
}

impl HandoffStatus {
    /// 是否完成态（只有 `done` 算完成；封存判定用它）。
    pub fn is_done(&self) -> bool {
        matches!(self, HandoffStatus::Done)
    }
}

impl MemberHandoff {
    /// 是否回指某个任务包（**只判关联**，不触盘、不跑运行时）。
    ///
    /// 判据：`packet_id` 逐字相等。`packet_id` 为空 ⇒ 恒 `false`。
    pub fn links_to(&self, packet: &TaskPacket) -> bool {
        !self.packet_id.is_empty() && self.packet_id == packet.packet_id
    }
}
