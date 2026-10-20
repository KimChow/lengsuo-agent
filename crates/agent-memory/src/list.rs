//! `memory.list` 后端：列记忆根下**直接**文件与目录。
//!
//! 转写自 codex `codex-rs/ext/memories/src/local/list.rs`：同样的名称、同样的
//! cursor 语义（结果序号）、同样的 `max_results.min(MAX_LIST_RESULTS)` 夹取与
//! `{path, entries, next_cursor, truncated}` 响应形状。
//!
//! **唯一刻意偏离**：本仓有两个记忆根（用户层 / 项目层），`path` 支持
//! `user:` / `project:` 前缀消歧；省略前缀时默认项目根。所有候选路径都先过
//! [`MemoryPaths::ensure_within`]——越界防护只有这一份实现。
//!
//! 本模块同时承载三个检索模块共用的路径/目录辅助函数（对齐 codex
//! `local/path.rs`，因本卡只新增 list/search/read 三个文件，故不再单开模块）。

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::MemoryError;
use crate::paths::MemoryPaths;

/// `memory.list` 默认返回条数（照抄 codex `lib.rs:11`）。
pub const DEFAULT_LIST_MAX_RESULTS: usize = 2_000;
/// `memory.list` 上限（照抄 codex `lib.rs:12`）。
pub const MAX_LIST_RESULTS: usize = 2_000;

/// 用户记忆根前缀：`user:` = `<home>/memory`。
pub const USER_PREFIX: &str = "user:";
/// 项目记忆根前缀：`project:` = `<workspace>/.lengsuo/memory`。
pub const PROJECT_PREFIX: &str = "project:";

/// `memory.list` 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListMemoriesRequest {
    pub path: Option<String>,
    pub cursor: Option<String>,
    pub max_results: usize,
}

/// `memory.list` 响应（字段名照抄 codex）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListMemoriesResponse {
    pub path: Option<String>,
    pub entries: Vec<MemoryEntry>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

/// 单个目录项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryEntry {
    pub path: String,
    pub entry_type: MemoryEntryType,
}

/// 目录项类型（serde 形状照抄 codex：`file` / `directory`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryType {
    File,
    Directory,
}

/// 解析后的记忆根与目标路径。
#[derive(Debug, Clone)]
pub(crate) struct ResolvedPath {
    pub(crate) scope: MemoryScope,
    pub(crate) root: PathBuf,
    pub(crate) full: PathBuf,
}

/// 记忆根归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryScope {
    User,
    Project,
}

impl MemoryScope {
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            MemoryScope::User => USER_PREFIX,
            MemoryScope::Project => PROJECT_PREFIX,
        }
    }
}

/// `path` →（记忆根，根内相对路径）。
///
/// 省略 `path` 与无前缀路径都落在项目根（codex 单根语义在本仓的默认落点）；
/// `user:` / `project:` 显式选根。前缀后的路径保持原样，绝对路径与 `..` 不在
/// 这里特判——交给 [`MemoryPaths::ensure_within`] 统一拒绝。
pub(crate) fn split_scope(path: Option<&str>) -> (MemoryScope, String) {
    match path {
        Some(rest) if rest.starts_with(USER_PREFIX) => {
            (MemoryScope::User, rest[USER_PREFIX.len()..].to_string())
        }
        Some(rest) if rest.starts_with(PROJECT_PREFIX) => (
            MemoryScope::Project,
            rest[PROJECT_PREFIX.len()..].to_string(),
        ),
        Some(rest) => (MemoryScope::Project, rest.to_string()),
        None => (MemoryScope::Project, String::new()),
    }
}

/// 把 `path` 解析到某个记忆根内，并复用 `ensure_within` 做两道边界校验：
/// 1. 记忆根自身必须落在授权根内（home / workspace）；
/// 2. 目标路径必须落在记忆根内（词法 + 真实路径复核，拦 symlink 逃逸）。
pub(crate) fn resolve_memory_path(
    paths: &MemoryPaths,
    path: Option<&str>,
) -> Result<ResolvedPath, MemoryError> {
    let (scope, relative) = split_scope(path);
    let (base, root) = match scope {
        MemoryScope::User => (paths.home.clone(), paths.user_memory_root()),
        MemoryScope::Project => (paths.workspace.clone(), paths.project_memory_dir()),
    };
    MemoryPaths::ensure_within(&base, &root)?;

    // codex `resolve_scoped_path` 的词法预检：绝对路径/盘符/`..` 一律拒绝
    // （即使 `a/../b` 归一化后仍在根内）。随后仍由 `ensure_within` 做真实路径复核。
    if Path::new(&relative).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(MemoryError::PathEscape);
    }

    if Path::new(&relative)
        .components()
        .any(|component| matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('.')))
    {
        // codex `resolve_scoped_path`：记忆根内的隐藏组件一律视为不存在。
        return Err(MemoryError::NotFound {
            path: path.unwrap_or_default().to_string(),
        });
    }

    let full = root.join(&relative);
    MemoryPaths::ensure_within(&root, &full)?;
    Ok(ResolvedPath { scope, root, full })
}

/// `memory.list`：列 `path` 下**直接**文件与目录（不递归）。
pub fn list(
    paths: &MemoryPaths,
    request: ListMemoriesRequest,
) -> Result<ListMemoriesResponse, MemoryError> {
    let max_results = request.max_results.min(MAX_LIST_RESULTS);
    let resolved = resolve_memory_path(paths, request.path.as_deref())?;
    let start_index = match request.cursor.as_deref() {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| MemoryError::InvalidCursor {
                cursor: cursor.to_string(),
            })?,
        None => 0,
    };
    let Some(metadata) = metadata_or_none(&resolved.full)? else {
        return Err(MemoryError::NotFound {
            path: request.path.unwrap_or_default(),
        });
    };
    reject_symlink(&resolved.full, &metadata)?;

    let mut entries = if metadata.is_file() {
        vec![MemoryEntry {
            path: display_path(&resolved, &resolved.full),
            entry_type: MemoryEntryType::File,
        }]
    } else if metadata.is_dir() {
        let mut entries = Vec::new();
        for (entry_path, file_type) in read_sorted_dir_entries(&resolved.full)? {
            if is_hidden_path(&entry_path) || file_type.is_symlink() {
                continue;
            }
            let entry_type = if file_type.is_dir() {
                MemoryEntryType::Directory
            } else if file_type.is_file() {
                MemoryEntryType::File
            } else {
                continue;
            };
            entries.push(MemoryEntry {
                path: display_path(&resolved, &entry_path),
                entry_type,
            });
        }
        entries
    } else {
        Vec::new()
    };
    if start_index > entries.len() {
        return Err(MemoryError::InvalidCursor {
            cursor: start_index.to_string(),
        });
    }

    let end_index = start_index.saturating_add(max_results).min(entries.len());
    let next_cursor = (end_index < entries.len()).then(|| end_index.to_string());
    let truncated = next_cursor.is_some();
    Ok(ListMemoriesResponse {
        path: request.path,
        entries: entries.drain(start_index..end_index).collect(),
        next_cursor,
        truncated,
    })
}

/// 非跟随 `symlink_metadata`：缺失 ⇒ `None`，其余 IO 错误上抛。
pub(crate) fn metadata_or_none(path: &Path) -> Result<Option<fs::Metadata>, MemoryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_err(path, err)),
    }
}

/// 记忆根内仍拒绝 symlink 起点/目标（对齐 codex `path::reject_symlink`）。
pub(crate) fn reject_symlink(path: &Path, metadata: &fs::Metadata) -> Result<(), MemoryError> {
    if metadata.file_type().is_symlink() {
        return Err(MemoryError::InvalidPath {
            path: path.display().to_string(),
            reason: "must not be a symlink".to_string(),
        });
    }
    Ok(())
}

/// 按路径字典序列出目录项；目录缺失/条目消失 ⇒ 空/跳过（对齐 codex）。
pub(crate) fn read_sorted_dir_entries(
    dir: &Path,
) -> Result<Vec<(PathBuf, fs::FileType)>, MemoryError> {
    let handle = match fs::read_dir(dir) {
        Ok(handle) => handle,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_err(dir, err)),
    };
    let mut entries = Vec::new();
    for entry in handle {
        let entry = entry.map_err(|err| io_err(dir, err))?;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(io_err(&entry.path(), err)),
        };
        entries.push((entry.path(), file_type));
    }
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(entries)
}

/// 记忆根内实体名以 `.` 开头 ⇒ 隐藏（对齐 codex `is_hidden_path`）。
pub(crate) fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// 相对记忆根的展示路径，带 `user:` / `project:` 前缀（双根消歧）。
pub(crate) fn display_path(resolved: &ResolvedPath, path: &Path) -> String {
    let relative = path.strip_prefix(&resolved.root).unwrap_or(path);
    let joined = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    format!("{}{}", resolved.scope.prefix(), joined)
}

/// 统一 IO 错误包装：带路径，不带内容（规格 §2 隐私要求）。
pub(crate) fn io_err(path: &Path, source: std::io::Error) -> MemoryError {
    MemoryError::Io {
        path: path.to_path_buf(),
        source,
    }
}
