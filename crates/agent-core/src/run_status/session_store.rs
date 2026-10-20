//! 运行状态的落盘载体（h3-run-status-persist）。
//!
//! 规格：wb 26 册 §1.5 的落盘侧。`RunStatus` 与 `Session` 同生命周期——**每次状态
//! 迁移都写一份完整快照**（`{dir}/run-status-{sessionId}.json`），崩在写的中间也
//! 不会留下半截 JSON（同目录临时文件 + rename 原子替换）。
//!
//! 授权外沿（与 `blobs.rs` 同口径）：落盘目录必须是**非 symlink 的真实目录**，
//! 状态文件必须是**非 symlink 的普通文件**，否则拒绝读写——不把 symlink 当合法
//! 部署形态。
//!
//! 不做版本化迁移：记录是纯投影（会话 id + 运行态 + 活动时刻），坏了就报错，
//! 不猜、不静默丢弃。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CoreError;

use super::status::RunStatus;

/// 落盘文件名前缀（会话 id 夹在中间）。
const FILE_PREFIX: &str = "run-status-";
/// 落盘文件名后缀。
const FILE_SUFFIX: &str = ".json";

/// 落盘文件内容（camelCase，与 `RunStatus` 同一口径）。
///
/// `updated_at` 是 wb 26 册 §1.5 `restore(sessionId, rawStatus, {updatedAt, …})`
/// 的 `updatedAt`：**末次可见活动时刻**，既用于 30 分钟 stale 判定，也用于
/// 崩溃终结时的时间戳（wb：「时间戳用 updatedAt」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedRunStatus {
    /// 会话 id（wb 的 `sessionId`）：文件名之外的**内容级**归属校验，
    /// 防止把别的会话的状态当本会话的读。
    pub session_id: Uuid,
    /// 运行态三元组与观测字段（P0-1 契约面，本卡不改其字段与事件定义）。
    pub run_status: RunStatus,
    /// 末次可见活动时刻（wb 的 `updatedAt`）。
    pub updated_at: u64,
}

/// 运行状态的落盘存储。
#[derive(Debug, Clone)]
pub struct SessionStatusStore {
    dir: PathBuf,
}

impl SessionStatusStore {
    /// 指定落盘目录（不存在时**首次写入**时自动创建）。
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    /// 落盘目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 会话状态文件路径（只由 id 拼装，无路径穿越面）。
    pub fn path_of(&self, session_id: Uuid) -> PathBuf {
        self.dir
            .join(format!("{FILE_PREFIX}{session_id}{FILE_SUFFIX}"))
    }

    /// 写一份快照（原子替换）。
    pub fn save(&self, record: &PersistedRunStatus) -> Result<(), CoreError> {
        self.ensure_dir()?;
        let path = self.path_of(record.session_id);
        let bytes = serde_json::to_vec_pretty(record).map_err(CoreError::Json)?;
        write_atomic(&path, &bytes)
    }

    /// 读回一份快照；没有 ⇒ `None`。
    ///
    /// 文件坏掉 ⇒ 报错（不静默当「没有」）：静默忽略会让一个坏状态看起来像
    /// 「从未持久化」，掩盖数据损坏。
    pub fn load(&self, session_id: Uuid) -> Result<Option<PersistedRunStatus>, CoreError> {
        let path = self.path_of(session_id);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(CoreError::Io {
                    path: path.clone(),
                    source: e,
                })
            }
        };
        // 非普通文件（含 symlink）不读：fail-closed，不跟随链接读外部内容。
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(CoreError::Io {
                path,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "运行状态文件不是普通文件（symlink 文件不授权）",
                ),
            });
        }
        let raw = fs::read(&path).map_err(|e| CoreError::Io {
            path: path.clone(),
            source: e,
        })?;
        let record: PersistedRunStatus = serde_json::from_slice(&raw).map_err(CoreError::Json)?;
        if record.session_id != session_id {
            return Err(CoreError::InvalidConfig(format!(
                "会话不匹配: 落盘文件记的是 {}，读的是 {}",
                record.session_id, session_id
            )));
        }
        Ok(Some(record))
    }

    /// 已落盘的会话 id（升序；启动扫描用）。
    ///
    /// 只列真实目录下的普通文件；symlink、目录等一律不列。
    pub fn sessions(&self) -> Vec<Uuid> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut ids: Vec<Uuid> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
            .filter_map(|e| {
                let name = e.file_name().to_str()?.to_string();
                let stem = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
                Uuid::parse_str(stem).ok()
            })
            .collect();
        ids.sort();
        ids
    }

    /// 删除一份快照；不存在视为已清理。
    pub fn remove(&self, session_id: Uuid) -> Result<(), CoreError> {
        let path = self.path_of(session_id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::Io { path, source: e }),
        }
    }

    /// 落盘目录必须是**非 symlink 的真实目录**（不存在则创建）。
    fn ensure_dir(&self) -> Result<(), CoreError> {
        match fs::symlink_metadata(&self.dir) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
            Ok(_) => Err(self.untrusted_dir()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.dir).map_err(|e| CoreError::Io {
                    path: self.dir.clone(),
                    source: e,
                })?;
                match fs::symlink_metadata(&self.dir) {
                    Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
                    _ => Err(self.untrusted_dir()),
                }
            }
            Err(e) => Err(CoreError::Io {
                path: self.dir.clone(),
                source: e,
            }),
        }
    }

    fn untrusted_dir(&self) -> CoreError {
        CoreError::Io {
            path: self.dir.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "运行状态目录不是真实目录（symlink 目录不授权）",
            ),
        }
    }
}

/// 原子写：同目录临时文件 + rename；写失败不留残file。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "run-status".to_owned());
    let tmp = dir.join(format!(".{name}.tmp-{}", Uuid::new_v4()));
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| CoreError::Io {
                path: tmp.clone(),
                source: e,
            })?;
        file.write_all(bytes).map_err(|e| CoreError::Io {
            path: tmp.clone(),
            source: e,
        })?;
        file.flush().map_err(|e| CoreError::Io {
            path: tmp.clone(),
            source: e,
        })?;
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(CoreError::Io {
                path: path.to_path_buf(),
                source: e,
            })
        }
    }
}
