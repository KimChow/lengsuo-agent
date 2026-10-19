//! 准备策略授权与分量级 deny 匹配：路径安全闸的判定数据层。
//! 各检查点（`FsToolbox::resolve`）自行完成对根与 carveout 的文件系统解析。

use std::ffi::OsStr;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::pathguard::normalize;

/// 不可覆盖的受保护路径分量：凭据目录自身与其全部深层子项。
pub(crate) const PROTECTED_COMPONENTS: [&str; 2] = [".ssh", ".gnupg"];

/// 已准备的策略：解析后的工作区根 + 分量级 deny 匹配器。
#[derive(Debug, Clone)]
pub(crate) struct PreparedPolicy {
    /// 工作区根：存在的路径已解析符号链接，缺失时保留词法规范化结果。
    pub root: PathBuf,
    /// 分量级 deny；`None` 表示策略未配置任何 carveout（当前恒为 `Some`）。
    pub component_denials: Option<ProtectedComponentMatcher>,
}

/// 路径分量级 deny：按分量**全等**比对，不做子串或前缀匹配
/// （`.ssh-backup` 是不同分量，不会命中）。
///
/// 跨平台词法规则：POSIX 区分大小写；Windows 文件系统大小写不敏感，
/// 按 ASCII 折叠后比对，防止 `.SSH` 写法绕过 deny。
#[derive(Debug, Clone)]
pub(crate) struct ProtectedComponentMatcher {
    names: Vec<OsString>,
}

impl ProtectedComponentMatcher {
    /// 由受保护分量名构造；空名单视为未配置（返回 `None`）。
    pub fn new(names: &[&str]) -> Option<Self> {
        if names.is_empty() {
            return None;
        }
        Some(Self {
            names: names.iter().map(|n| OsStr::new(*n).to_owned()).collect(),
        })
    }

    /// 单个分量是否受保护。
    pub fn is_protected(&self, component: &OsStr) -> bool {
        self.names
            .iter()
            .any(|name| component_eq(name.as_os_str(), component))
    }

    /// 绝对路径中第一个受保护分量所在前缀（含该分量）：定位负责该发现的根。
    /// 只认 `Normal` 分量，盘符/根/`..` 等结构分量不参与匹配。
    pub fn protected_prefix(&self, absolute: &Path) -> Option<PathBuf> {
        let mut prefix = PathBuf::new();
        for component in absolute.components() {
            prefix.push(component.as_os_str());
            if let Component::Normal(seg) = component {
                if self.is_protected(seg) {
                    return Some(prefix);
                }
            }
        }
        None
    }
}

/// 分量名全等比较：Windows 按 ASCII 折叠大小写，其余平台区分大小写。
#[cfg(windows)]
fn component_eq(a: &OsStr, b: &OsStr) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// 分量名全等比较（区分大小写平台）。
#[cfg(not(windows))]
fn component_eq(a: &OsStr, b: &OsStr) -> bool {
    a == b
}

/// 准备策略：解析存在的根（缺失则保留词法规范化结果），装配分量 deny。
pub(crate) fn prepare_policy(workspace: &Path) -> io::Result<PreparedPolicy> {
    let root = match workspace.canonicalize() {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == io::ErrorKind::NotFound => normalize(workspace),
        Err(error) => return Err(error),
    };
    Ok(PreparedPolicy {
        root,
        component_denials: ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS),
    })
}

/// 词法回退：根解析失败（权限、链接环等）时保留词法规范化结果。
/// 单条路径的 `resolve` 仍会在同样的错误上 fail-closed，故回退不放大逃逸面。
pub(crate) fn lexical_fallback(workspace: &Path) -> PreparedPolicy {
    PreparedPolicy {
        root: normalize(workspace),
        component_denials: ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matcher_全等匹配且不误伤同名前缀() {
        let m = ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS).unwrap();
        assert!(m.is_protected(OsStr::new(".ssh")));
        assert!(m.is_protected(OsStr::new(".gnupg")));
        assert!(!m.is_protected(OsStr::new(".ssh-backup")));
        assert!(!m.is_protected(OsStr::new("ssh")));
        assert!(!m.is_protected(OsStr::new(".sshx")));
    }

    #[cfg(not(windows))]
    #[test]
    fn matcher_区分大小写平台下大小写不同的分量不命中() {
        let m = ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS).unwrap();
        assert!(!m.is_protected(OsStr::new(".SSH")));
    }

    #[cfg(windows)]
    #[test]
    fn matcher_大小写不敏感平台下折叠ASCII比较() {
        let m = ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS).unwrap();
        assert!(m.is_protected(OsStr::new(".SSH")));
        assert!(m.is_protected(OsStr::new(".GnuPG")));
    }

    #[test]
    fn matcher_空名单视为未配置() {
        assert!(ProtectedComponentMatcher::new(&[]).is_none());
    }

    #[test]
    fn protected_prefix_定位第一个受保护分量前缀() {
        let m = ProtectedComponentMatcher::new(&PROTECTED_COMPONENTS).unwrap();
        assert_eq!(
            m.protected_prefix(Path::new("/ws/.ssh/keys/id")),
            Some(PathBuf::from("/ws/.ssh"))
        );
        assert_eq!(
            m.protected_prefix(Path::new("/ws/a/.gnupg")),
            Some(PathBuf::from("/ws/a/.gnupg"))
        );
        assert_eq!(m.protected_prefix(Path::new("/ws/.ssh-backup/x")), None);
    }
}
