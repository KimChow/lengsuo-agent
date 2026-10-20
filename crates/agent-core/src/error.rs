//! 统一错误类型。

use std::path::PathBuf;

use uuid::Uuid;

/// agent-core 域错误；网关层负责映射到信封 code。
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("IO 失败 {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("JSON 处理失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("事件文件 schema 版本不支持: 文件为 {found}，本实现为 {supported}")]
    SchemaVersion { found: u32, supported: u32 },

    #[error("事件行过大: {actual} > {limit} 字节")]
    LineTooLarge { limit: usize, actual: usize },

    #[error("会话不存在: {0}")]
    SessionNotFound(Uuid),

    #[error("会话已结束，拒绝新输入: {0}")]
    SessionClosed(Uuid),

    #[error("配置非法: {0}")]
    InvalidConfig(String),

    #[error("工具未注册: {0}")]
    ToolNotFound(String),

    #[error("审批不存在或已超时: {0}")]
    ApprovalNotFound(Uuid),

    #[error("审批已应答，不可重复: {0}")]
    ApprovalAnswered(Uuid),

    #[error("凭据/大结果引用不存在或非法: {0}")]
    BlobNotFound(String),

    #[error("能力未实现（v0 占位）: {0}")]
    Unsupported(&'static str),

    /// 任务看板操作失败；字符串透传 `agent_tasks::TaskError` 原始文案（不吞原因）。
    #[error("{0}")]
    Task(String),

    /// 专家不可用（召唤 / 组装期取人格失败）。`code` 是可读错误码：`EXPERT_NOT_FOUND`
    /// 表示双根里没有该 id，其余为专家包载入/校验的稳定错误码（如 `TAG_COUNT`）；
    /// 显式带 `expert_id` 便于宿主定位。
    #[error("专家不可用（{code}）{expert_id}: {reason}")]
    ExpertUnavailable {
        expert_id: String,
        code: String,
        reason: String,
    },
}

impl CoreError {
    /// 错误码（宿主据此区分失败类别；非专家类错误返回 `None`）。
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::ExpertUnavailable { code, .. } => Some(code),
            _ => None,
        }
    }
}

impl From<agent_memory::MemoryError> for CoreError {
    fn from(e: agent_memory::MemoryError) -> Self {
        match e {
            agent_memory::MemoryError::Io { path, source } => Self::Io { path, source },
            other => Self::InvalidConfig(other.to_string()),
        }
    }
}
