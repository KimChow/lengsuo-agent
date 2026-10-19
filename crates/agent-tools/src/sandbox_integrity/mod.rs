//! 包含性完整性：策略准备（`policy`）与共享发现结构（`finding`）。
//! 检查点（`FsToolbox::resolve`）负责对这些根与 carveout 做文件系统解析。

mod finding;
mod policy;

pub(crate) use finding::IntegrityFinding;
pub(crate) use finding::IntegrityFindingDetails;
pub(crate) use policy::lexical_fallback;
pub(crate) use policy::prepare_policy;
pub(crate) use policy::PreparedPolicy;
