//! 安全闸：词法前缀校验（`..` 拒绝、绝对路径逃逸拒绝）+ 逐成分符号链接解析
//! + 受保护分量 deny。所有文件通道统一经过 [`FsToolbox::resolve`]。

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::error::ToolError;
use crate::sandbox_integrity::IntegrityFinding;
use crate::sandbox_integrity::IntegrityFindingDetails;

/// 断链符号链接展开的换向深度上限（对齐内核 ELOOP 量级）。
const MAX_LINK_DEPTH: usize = 40;

impl crate::FsToolbox {
    /// 完整路径闸：词法校验 → 逐成分解析符号链接 → 工作区包含判定 → 受保护分量 deny。
    ///
    /// 任何一步失败一律 `Denied`（fail-closed），错误负载是调用方原始路径。
    /// 成功时返回**解析后**的绝对路径；后续所有文件操作都用它。
    pub(crate) fn resolve(&self, p: &Path) -> Result<PathBuf, ToolError> {
        let lexical = self.guard_lexical(p)?;
        let resolved = resolve_links(&lexical)?;
        if !resolved.starts_with(&self.policy.root) {
            return Err(ToolError::Denied(p.to_path_buf()));
        }
        if let Some(finding) = self.protected_finding(p, &lexical, &resolved) {
            return Err(ToolError::Denied(finding.target));
        }
        Ok(resolved)
    }

    /// 词法闸：任何 `..` 成分直接拒绝；解析结果必须以工作区为前缀。
    fn guard_lexical(&self, p: &Path) -> Result<PathBuf, ToolError> {
        let candidate = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        };
        if candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(ToolError::Denied(p.to_path_buf()));
        }
        let normalized = normalize(&candidate);
        if normalized.starts_with(&self.workspace) {
            Ok(normalized)
        } else {
            Err(ToolError::Denied(p.to_path_buf()))
        }
    }

    /// 受保护分量 deny（不可覆盖）：词法路径与解析后目标各查一遍，
    /// 覆盖目录自身、深层子项与经符号链接换向后的落点。
    fn protected_finding(
        &self,
        target: &Path,
        lexical: &Path,
        resolved: &Path,
    ) -> Option<IntegrityFinding> {
        let denials = self.policy.component_denials.as_ref()?;
        let root = denials
            .protected_prefix(lexical)
            .or_else(|| denials.protected_prefix(resolved))?;
        Some(IntegrityFinding {
            target: target.to_path_buf(),
            write_roots: vec![root.clone()],
            details: IntegrityFindingDetails::Contents {
                resolved_target: root,
            },
        })
    }

    /// 单个路径分量名是否受保护（供列举/递归搜索剪枝）。
    pub(crate) fn is_protected_name(&self, name: &std::ffi::OsStr) -> bool {
        self.policy
            .component_denials
            .as_ref()
            .is_some_and(|denials| denials.is_protected(name))
    }

    /// guard 结果的相对工作区展示路径（`/` 分隔）。
    pub(crate) fn rel_display(&self, abs: &Path) -> String {
        abs.strip_prefix(&self.workspace)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

/// 逐成分解析符号链接后的词法规范化。
///
/// 对 `p` 的每个前缀尝试 `canonicalize`：已存在的中间符号链接（含断链的**父目录**）
/// 被换成真实目标；末段自身是断链时手工展开链接目标（限深 [`MAX_LINK_DEPTH`]），
/// 其余不存在的前缀原样保留，最后再做一次词法规范化。
/// 解析途中的非 NotFound 错误（权限、链接环等）一律 fail-closed 返回 `Denied`。
pub(crate) fn resolve_links(p: &Path) -> Result<PathBuf, ToolError> {
    resolve_links_bounded(p, 0)
}

fn resolve_links_bounded(p: &Path, depth: usize) -> Result<PathBuf, ToolError> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Prefix(_) | Component::RootDir => out.push(c.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(seg) => {
                out.push(seg);
                out = resolve_prefix(&out, depth)?;
            }
        }
    }
    Ok(normalize(&out))
}

/// 单个前缀的解析：存在的路径 canonicalize；断链符号链接展开其目标，
/// 避免后续写入/建目录跟随链接落到工作区外；其余缺失前缀原样保留。
fn resolve_prefix(prefix: &Path, depth: usize) -> Result<PathBuf, ToolError> {
    match fs::canonicalize(prefix) {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == ErrorKind::NotFound => match fs::symlink_metadata(prefix) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if depth >= MAX_LINK_DEPTH {
                    return Err(ToolError::Denied(prefix.to_path_buf()));
                }
                let target =
                    fs::read_link(prefix).map_err(|_| ToolError::Denied(prefix.to_path_buf()))?;
                let joined = if target.is_absolute() {
                    target
                } else {
                    prefix
                        .parent()
                        .unwrap_or_else(|| Path::new(""))
                        .join(target)
                };
                resolve_links_bounded(&joined, depth + 1)
            }
            _ => Ok(prefix.to_path_buf()),
        },
        Err(_) => Err(ToolError::Denied(prefix.to_path_buf())),
    }
}

/// 词法规范化：折叠 `.` 与重复分隔符，不触碰文件系统。
pub(crate) fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        if !matches!(c, Component::CurDir) {
            out.push(c.as_os_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::testutil::TempDir;

    use super::*;

    fn tb(workspace: &Path) -> crate::FsToolbox {
        crate::FsToolbox::new(workspace)
    }

    // 用真实临时目录做基准：`resolve` 会逐前缀 canonicalize，
    // 硬编码 `/tmp/ws` 这类不存在路径在链接根（如 macOS /tmp）上语义不同。
    #[test]
    fn guard_接受相对路径并折叠点段() {
        let t = TempDir::new();
        let t = tb(t.path());
        assert_eq!(
            t.guard_lexical(Path::new("a/b.txt")).unwrap(),
            t.workspace.join("a/b.txt")
        );
        assert_eq!(
            t.guard_lexical(Path::new("./a//b.txt")).unwrap(),
            t.workspace.join("a/b.txt")
        );
    }

    #[test]
    fn guard_拒绝穿越与工作区外绝对路径() {
        let t = tb(Path::new("/tmp/ws"));
        assert_eq!(
            t.guard_lexical(Path::new("../etc/passwd")),
            Err(ToolError::Denied(PathBuf::from("../etc/passwd")))
        );
        assert_eq!(
            t.guard_lexical(Path::new("a/../../b")),
            Err(ToolError::Denied(PathBuf::from("a/../../b")))
        );
        assert_eq!(
            t.guard_lexical(Path::new("/etc/passwd")),
            Err(ToolError::Denied(PathBuf::from("/etc/passwd")))
        );
    }

    #[test]
    fn guard_接受工作区内绝对路径_工作区自身合法() {
        let t = TempDir::new();
        let t = tb(t.path());
        assert_eq!(
            t.resolve(&t.workspace.join("x")).unwrap(),
            t.workspace.join("x")
        );
        assert_eq!(t.resolve(Path::new(".")).unwrap(), t.workspace);
        assert_eq!(t.resolve(Path::new("")).unwrap(), t.workspace);
    }

    #[test]
    fn guard_拒绝同前缀兄弟目录逃逸() {
        let t = tb(Path::new("/tmp/ws"));
        // /tmp/wsx 不是 /tmp/ws 的子路径，即便字符串前缀相同也必须拒绝。
        assert!(t.resolve(Path::new("/tmp/wsx/a")).is_err());
    }

    #[test]
    fn resolve_links_逐成分还原符号链接目标() {
        let t = TempDir::new();
        std::fs::create_dir(t.path().join("real")).unwrap();
        std::fs::write(t.path().join("real/x.txt"), "").unwrap();
        std::os::unix::fs::symlink("real", t.path().join("link")).unwrap();
        // 工作区内的符号链接：解析回真实目标，仍在工作区内。
        assert_eq!(
            resolve_links(&t.path().join("link/x.txt")).unwrap(),
            t.path().join("real/x.txt")
        );

        let outside = TempDir::new();
        std::fs::write(outside.path().join("secret.txt"), "").unwrap();
        std::os::unix::fs::symlink(outside.path(), t.path().join("out")).unwrap();
        // 词法上前缀仍在工作区内，逐成分解析后逃逸 —— 这正是词法闸的漏洞，
        // 由 resolve 的包含判定拦截（见 tests/path_safety.rs）。
        assert!(t.path().join("out/secret.txt").starts_with(t.path()));
        assert!(!resolve_links(&t.path().join("out/secret.txt"))
            .unwrap()
            .starts_with(t.path()));
    }

    #[test]
    fn resolve_links_断链指向区外时展开目标() {
        let t = TempDir::new();
        let outside = TempDir::new();
        let ghost = outside.path().join("ghost");
        std::os::unix::fs::symlink(&ghost, t.path().join("dangling")).unwrap();
        // 断链不是「不存在」：必须展开链接目标，否则后续写入会落到区外。
        assert_eq!(
            resolve_links(&t.path().join("dangling/new")).unwrap(),
            ghost.join("new")
        );
    }

    #[test]
    fn rel_display_返回工作区相对路径() {
        let t = TempDir::new();
        let t = tb(t.path());
        assert_eq!(t.rel_display(&t.workspace.join("a/b.txt")), "a/b.txt");
        assert_eq!(t.rel_display(&t.workspace), "");
    }
}
