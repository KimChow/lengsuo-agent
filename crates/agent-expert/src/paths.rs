//! 专家包路径常量与越界防护（卡 3：发现/注册表落地；§4.1）。
//!
//! 调研 §4.1 给出的本仓落地方案（对齐 `agent-memory/src/paths.rs` 的
//! home / workspace 二分）：
//!
//! ```text
//! ~/.lengsuo/experts/                    # 用户级专家根（跨项目）
//! ├── registry.json                      # 注册表（专家注册/发现索引）
//! └── <expert-id>/                       # 一个专家包 = 一个目录
//!     ├── .lengsuo-plugin/plugin.json    # 原名 .codebuddy-plugin/（§3.3 唯一改名）
//!     ├── agents/…
//!     ├── skills/…
//!     └── avatars/…
//!
//! <workspace>/.lengsuo/experts/          # 项目级专家根（可选，覆盖用户级）
//! └── <expert-id>/…
//! ```
//!
//! 越界防护**在本 crate 自建**（`agent-memory::MemoryPaths::ensure_within` 是
//! 私有约定，不改 `agent-memory`）：与它的判定口径逐条对齐——先词法归一化
//! 比对前缀，再取最深存在实体做 `canonicalize` 复核，拦 symlink 逃逸。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// 插件目录名（原 `.codebuddy-plugin/`，本仓改名为 `.lengsuo-plugin/`，§3.3）。
pub const PLUGIN_DIR: &str = ".lengsuo-plugin";

/// manifest 文件名（插件目录内的 `plugin.json`）。
pub const MANIFEST_FILE: &str = "plugin.json";

/// 人格目录名（`agents/`）。
pub const AGENTS_DIR: &str = "agents";

/// 技能目录名（`skills/`）。
pub const SKILLS_DIR: &str = "skills";

/// 头像目录名（`avatars/`）。
pub const AVATARS_DIR: &str = "avatars";

/// 可选通用 CLI 目录（`bin/`）。
pub const BIN_DIR: &str = "bin";

/// 用户级专家根相对 home 的路径（`~/.lengsuo/experts`，§4.1）。
pub const USER_EXPERTS_DIR: &str = ".lengsuo/experts";

/// 项目级专家根相对工作区的路径（`<ws>/.lengsuo/experts`，§4.1）。
pub const PROJECT_EXPERTS_DIR: &str = ".lengsuo/experts";

/// 注册表文件名（专家根下 `registry.json`，§4.2）。
pub const REGISTRY_FILE: &str = "registry.json";

/// 下载时间标记文件名（**规范外**，§1.3：保留作注册表展示/排序用）。
pub const DOWNLOADED_AT_FILE: &str = ".downloaded_at";

/// 包目录内的 manifest 相对路径（`.lengsuo-plugin/plugin.json`）。
pub fn manifest_rel_path() -> PathBuf {
    Path::new(PLUGIN_DIR).join(MANIFEST_FILE)
}

/// 用户级专家根：`<home>/.lengsuo/experts`。
pub fn user_experts_root(home: &Path) -> PathBuf {
    home.join(USER_EXPERTS_DIR)
}

/// 项目级专家根：`<workspace>/.lengsuo/experts`。
pub fn project_experts_root(workspace: &Path) -> PathBuf {
    workspace.join(PROJECT_EXPERTS_DIR)
}

/// 越界拒绝原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathEscape {
    /// 词法归一化后已不在授权根内（含显式 `..` 穿越）。
    Lexical,
    /// 词法合法，但最深的已存在实体经符号链接落到授权根之外（含悬空 symlink）。
    Symlink,
    /// 复核途中文件系统访问失败。
    Io(String),
}

/// 越界防护：`candidate` 必须落在授权根 `base` 内。
///
/// 口径与 `agent-memory::MemoryPaths::ensure_within` 一致：
/// 1. **词法**：解析 `.`/`..` 后比前缀，触盘前拦下显式穿越；
/// 2. **真实路径复核**：取 `candidate` 的祖先链中**最深存在实体**（按
///    `symlink_metadata`，含悬空 symlink），其 `canonicalize` 结果必须仍在
///    `canonicalize(base)` 内。
///
/// 局限（显式记录）：两步之间非原子（TOCTOU）；`base` 自身是 symlink 时其真实
/// 路径即授权根。`base` 尚不存在 ⇒ 词法校验已足够（子树没有可逃逸的落盘实体）。
pub fn ensure_within(base: &Path, candidate: &Path) -> Result<(), PathEscape> {
    let base_norm = normalize(base);
    let cand_norm = normalize(candidate);
    if !cand_norm.starts_with(&base_norm) {
        return Err(PathEscape::Lexical);
    }
    check_real_prefix(&base_norm, &cand_norm)
}

/// 词法归一化：解析 `.`/`..` 段，不触盘。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 真实路径复核：见 [`ensure_within`] 第 2 步。
fn check_real_prefix(base: &Path, target: &Path) -> Result<(), PathEscape> {
    let root = match fs::canonicalize(base) {
        Ok(root) => root,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(PathEscape::Io(e.to_string())),
    };
    for ancestor in target.ancestors() {
        // 越出授权根词法边界即停：不用上层目录误判成逃逸。
        if !ancestor.starts_with(base) {
            break;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(_) => {}
            // 尚未创建 ⇒ 继续向上层找。
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(PathEscape::Io(e.to_string())),
        }
        return match fs::canonicalize(ancestor) {
            // 悬空 symlink 无法确认落点 ⇒ fail-closed。
            Err(e) if e.kind() == ErrorKind::NotFound => Err(PathEscape::Symlink),
            Err(e) => Err(PathEscape::Io(e.to_string())),
            Ok(real) if real.starts_with(&root) => Ok(()),
            Ok(_) => Err(PathEscape::Symlink),
        };
    }
    Ok(())
}
