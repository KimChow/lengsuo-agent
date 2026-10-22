//! 统一工具错误：安全闸、存在性、大小与组回滚分别可辨。
//!
//! ⛔ **展示纪律**：`Display` 只作为内部诊断（日志/测试），路径以 `Debug` 形式
//! 打出**真实绝对路径**。凡是要回给模型/用户的文本，一律走 [`ToolError::display_in`]
//! 按工作区相对化——模型只应看到 workspace 内的相对路径，不该看到宿主机的
//! 绝对目录结构（含用户名、挂载点等环境信息）。
//!
//! 该口径与参考实现一致：能力层把根路径 `strip_prefix` 掉后只暴露相对路径。

use std::path::{Path, PathBuf};

/// `file:*` 能力层错误。调用方（协议层）按变体映射到工具结果。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("路径不存在: {0:?}")]
    NotFound(PathBuf),

    #[error("越权访问或防呆拒绝: {0:?}")]
    Denied(PathBuf),

    #[error("路径已存在: {0:?}")]
    AlreadyExists(PathBuf),

    #[error("文件过大: {path:?} 超过上限 {limit} 字节")]
    TooLarge { path: PathBuf, limit: u64 },

    #[error("不是目录: {0:?}")]
    NotADirectory(PathBuf),

    #[error("重命名目标已存在: {0:?}")]
    TargetExists(PathBuf),

    #[error("批量写入在第 {failed_index} 个操作失败，已整体回滚: {reason}")]
    BatchRolledBack { failed_index: usize, reason: String },

    #[error("IO 失败 {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// 测试断言用的变体级相等：同变体且关键负载相同（`Io` 只比路径与 kind）。
impl PartialEq for ToolError {
    fn eq(&self, other: &Self) -> bool {
        use ToolError as E;
        match (self, other) {
            (E::NotFound(a), E::NotFound(b))
            | (E::Denied(a), E::Denied(b))
            | (E::AlreadyExists(a), E::AlreadyExists(b))
            | (E::NotADirectory(a), E::NotADirectory(b))
            | (E::TargetExists(a), E::TargetExists(b)) => a == b,
            (
                E::TooLarge {
                    path: p1,
                    limit: l1,
                },
                E::TooLarge {
                    path: p2,
                    limit: l2,
                },
            ) => p1 == p2 && l1 == l2,
            (
                E::BatchRolledBack {
                    failed_index: i, ..
                },
                E::BatchRolledBack {
                    failed_index: j, ..
                },
            ) => i == j,
            (
                E::Io {
                    path: p1,
                    source: s1,
                },
                E::Io {
                    path: p2,
                    source: s2,
                },
            ) => p1 == p2 && s1.kind() == s2.kind(),
            _ => false,
        }
    }
}

impl ToolError {
    /// 渲染成**可给模型/用户看**的文本：路径一律相对 `workspace`，`/` 分隔。
    ///
    /// 与 [`Display`](std::fmt::Display) 的区别只有一处——路径呈现方式：
    /// `Display` 用 `{:?}` 打真实绝对路径（内部诊断用），本方法把绝对路径
    /// `strip_prefix(workspace)` 掉。workspace 之外的路径（本就不该展示）保留原样，
    /// 避免相对化失败时变成空串而丢失线索。
    ///
    /// 参考实现同样只暴露相对根路径（能力层 `strip_prefix` 后渲染）。
    #[must_use]
    pub fn display_in(&self, workspace: &Path) -> String {
        /// 绝对路径 → workspace 相对路径（`/` 分隔）；越界时原样返回。
        fn rel(p: &Path, workspace: &Path) -> String {
            p.strip_prefix(workspace)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        }
        match self {
            Self::NotFound(p) => format!("路径不存在: {}", rel(p, workspace)),
            Self::Denied(p) => format!("越权访问或防呆拒绝: {}", rel(p, workspace)),
            Self::AlreadyExists(p) => format!("路径已存在: {}", rel(p, workspace)),
            Self::NotADirectory(p) => format!("不是目录: {}", rel(p, workspace)),
            Self::TargetExists(p) => format!("重命名目标已存在: {}", rel(p, workspace)),
            Self::TooLarge { path, limit } => {
                format!("文件过大: {} 超过上限 {limit} 字节", rel(path, workspace))
            }
            Self::BatchRolledBack {
                failed_index,
                reason,
            } => format!("批量写入在第 {failed_index} 个操作失败，已整体回滚: {reason}"),
            Self::Io { path, source } => format!("IO 失败 {}: {source}", rel(path, workspace)),
        }
    }

    /// 把 `std::io::Error` 按语义归位：不存在 / 权限 / 不是目录分别落到专门变体。
    pub(crate) fn from_io(path: PathBuf, err: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match err.kind() {
            K::NotFound => Self::NotFound(path),
            K::PermissionDenied => Self::Denied(path),
            K::NotADirectory => Self::NotADirectory(path),
            _ => Self::Io { path, source: err },
        }
    }

    /// 把非 IO 的失败（trash 错误、回滚冲突等）包成 `Io`。
    pub(crate) fn other(path: PathBuf, msg: String) -> Self {
        Self::Io {
            path,
            source: std::io::Error::other(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_in_把工作区内路径相对化() {
        let ws = Path::new("/home/user/project");
        let p = PathBuf::from("/home/user/project/src/main.rs");
        let err = ToolError::NotFound(p.clone());
        let out = err.display_in(ws);
        assert_eq!(out, "路径不存在: src/main.rs");
        assert!(!out.contains("/home/user/project"));
    }

    #[test]
    fn display_in_工作区外路径保留原样() {
        let ws = Path::new("/home/user/project");
        let err = ToolError::Denied(PathBuf::from("/etc/passwd"));
        let out = err.display_in(ws);
        assert_eq!(out, "越权访问或防呆拒绝: /etc/passwd");
    }

    #[test]
    fn display_in_输出统一为正斜杠() {
        // 用平台原生分隔符拼路径（Windows 上是 `\`），断言渲染结果恒为 `/`。
        let ws = Path::new("/home/user/project");
        let p = ws.join("sub").join("file.txt");
        let out = ToolError::NotFound(p).display_in(ws);
        assert_eq!(out, "路径不存在: sub/file.txt");
    }
}
