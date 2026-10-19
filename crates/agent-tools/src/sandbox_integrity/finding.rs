//! 策略判定的共享发现结构：一次包含性判定及其证据。

use std::path::PathBuf;

/// 一次包含性完整性判定：目标路径、对其生效的授权根、以及证据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IntegrityFinding {
    /// 触发判定的请求路径（未解析形态）。
    pub target: PathBuf,
    /// 对目标生效、导致该发现的授权根（按命中顺序排列）。
    pub write_roots: Vec<PathBuf>,
    pub details: IntegrityFindingDetails,
}

/// 判定证据：按检测到的包含性弱点分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IntegrityFindingDetails {
    /// 目标的内容可被写入/读取（解析后的目标）。
    Contents { resolved_target: PathBuf },
}
