//! 三层记忆系统（规格：MEMORY-SPEC.md v0）。
//!
//! 层级：身份（SOUL/USER/AI 名）→ 用户记忆（跨会话，默认关）→ 项目记忆 + 当日日志。
//! 隐私：跨会话层默认关闭；写路径由调用方负责脱敏；凭据绝不入内。

mod caps;
mod daily;
mod engine;
mod error;
mod identity;
pub mod list;
mod paths;
pub mod read;
pub mod search;

pub use caps::{CAP_DAILY_LINE, CAP_PROJECT_MEMORY, CAP_SOUL, CAP_USER_MEMORY};
pub use daily::{append_daily, clear_daily, read_daily_tail};
pub use engine::{MemoryBundle, MemoryEngine};
pub use error::MemoryError;
pub use identity::{Identity, DEFAULT_AI_NAME, DEFAULT_USER_NAME};
pub use list::{
    ListMemoriesRequest, ListMemoriesResponse, MemoryEntry, MemoryEntryType,
    DEFAULT_LIST_MAX_RESULTS, MAX_LIST_RESULTS, PROJECT_PREFIX, USER_PREFIX,
};
pub use paths::MemoryPaths;
pub use read::{ReadMemoryRequest, ReadMemoryResponse, DEFAULT_READ_MAX_TOKENS};
pub use search::{
    MemorySearchMatch, SearchMatchMode, SearchMemoriesRequest, SearchMemoriesResponse,
    DEFAULT_SEARCH_MAX_RESULTS, MAX_SEARCH_RESULTS,
};

pub use chrono::NaiveDate;
