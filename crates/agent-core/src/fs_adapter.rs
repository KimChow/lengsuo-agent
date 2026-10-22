//! `fs.*` 工具的执行适配：把工具调用参数翻译成 [`agent_tools::FsToolbox`] 的调用。
//!
//! 分层纪律（照 `executor.rs` 的既有做法）：
//! - 本层只做**形状校验与结果转译**；路径安全闸一律交给 `agent-tools` 的
//!   [`agent_tools::FsToolbox`]（其内部 `pathguard` 做词法闸 → 符号链接解析 →
//!   工作区包含判定 → 受保护分量 deny）。⛔ 本层**不**自己写路径检查，
//!   否则会出现两套守卫、两套语义。
//! - 越界 / 不存在 / 不是目录 等失败一律转成**显式错误**（`Err`），
//!   绝不返回空内容或静默成功。
//!
//! 形状约定（`ToolCall.args` 的 JSON 形状）：
//! - `fs.read`：`{ "path": String, "max_bytes": Option<u64> }`
//! - `fs.write`：`{ "path": String, "content": String }`
//! - `fs.list`：`{ "path": Option<String>, "offset": Option<usize>, "limit": Option<usize> }`
//! - `fs.glob`：`{ "pattern": String, "root": Option<String>, "limit": Option<usize> }`
//! - `fs.search`：`{ "pattern": String, "root": Option<String>, "limit": Option<usize> }`
//! - `fs.delete`：`{ "path": String }`
//!
//! 覆盖防呆：`fs.write` 走 `FsToolbox::write(.., expect_read = true)`——
//! 已存在但本次未读过的文件 ⇒ 拒绝（`agent-tools` 的既有协议防呆，见 `edit.rs`）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use agent_tools::{FsToolbox, ToolError};

use crate::r#loop::ToolExecutor;

/// 读类工具的默认字节上限（与 `agent-tools` 的单文件硬上限一致）。
const DEFAULT_MAX_BYTES: u64 = agent_tools::MAX_SINGLE_READ_BYTES;

/// 列举/匹配类工具的默认条数上限。
const DEFAULT_LIMIT: usize = 200;

/// 列举/匹配类工具的条数上限（防止一次拉爆结果集）。
const MAX_LIMIT: usize = 1_000;

/// 本适配层认领的六个文件工具名（与 [`crate::tool_fs::tool_specs`] 逐字一致）。
pub const FS_TOOL_NAMES: [&str; 6] = [
    "fs.read",
    "fs.write",
    "fs.list",
    "fs.glob",
    "fs.search",
    "fs.delete",
];

/// `fs.*` 工具的执行适配器：持一个工作区锚定的 [`FsToolbox`]。
///
/// `FsToolbox` 内部记录「本次已读/已写」的路径集合（`seen`），覆盖防呆依赖它，
/// 因此**同一个适配器实例**应当服务同一条会话链。
pub struct FsToolExecutor {
    toolbox: FsToolbox,
}

impl FsToolExecutor {
    /// 以工作区为安全闸基准构造。相对路径按当前目录展开为绝对路径。
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            toolbox: FsToolbox::new(workspace),
        }
    }

    /// 安全闸的工作区根（展示与调试用）。
    pub fn workspace(&self) -> &Path {
        &self.toolbox.workspace
    }

    /// 按工具名分派到对应实现。未知名字 ⇒ 显式错误（不 panic、不静默成功）。
    pub fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "fs.read" => self.read(args),
            "fs.write" => self.write(args),
            "fs.list" => self.list(args),
            "fs.glob" => self.glob(args),
            "fs.search" => self.search(args),
            "fs.delete" => self.delete(args),
            other => Err(format!(
                "无可用执行器认领工具: {other}（本适配层认领 {:?}）",
                FS_TOOL_NAMES
            )),
        }
    }

    fn read(&self, args: &Value) -> Result<Value, String> {
        let path = required_str(args, "path")?;
        let max_bytes = optional_u64(args, "max_bytes")?.unwrap_or(DEFAULT_MAX_BYTES);
        let content = self.toolbox.read(&path, max_bytes).map_err(render)?;
        Ok(json!({"path": path, "content": content}))
    }

    fn write(&self, args: &Value) -> Result<Value, String> {
        let path = required_str(args, "path")?;
        let content = required_str(args, "content")?;
        // `expect_read = true`：覆盖已存在文件前必须先读过（既有协议防呆）。
        self.toolbox.write(&path, &content, true).map_err(render)?;
        Ok(json!({"path": path, "written": true}))
    }

    fn list(&self, args: &Value) -> Result<Value, String> {
        let path = optional_str(args, "path")?.unwrap_or_else(|| ".".to_string());
        let offset = optional_usize(args, "offset")?.unwrap_or(0);
        let limit = optional_usize(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);
        let (entries, total) = self
            .toolbox
            .list_dir(&path, offset, limit)
            .map_err(render)?;
        let items: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "path": e.rel_path,
                    "size": e.size,
                    "is_dir": e.is_dir,
                    "modified_ms": e.modified_ms,
                })
            })
            .collect();
        Ok(json!({"path": path, "total": total, "entries": items}))
    }

    fn glob(&self, args: &Value) -> Result<Value, String> {
        let pattern = required_str(args, "pattern")?;
        let root = optional_str(args, "root")?.unwrap_or_else(|| ".".to_string());
        let limit = optional_usize(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);
        let paths = self.toolbox.glob(&root, &pattern, limit).map_err(render)?;
        Ok(json!({"pattern": pattern, "root": root, "paths": paths}))
    }

    fn search(&self, args: &Value) -> Result<Value, String> {
        let pattern = required_str(args, "pattern")?;
        let root = optional_str(args, "root")?.unwrap_or_else(|| ".".to_string());
        let limit = optional_usize(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .min(MAX_LIMIT);
        let (hits, total) = self
            .toolbox
            .search(&root, &pattern, limit)
            .map_err(render)?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "name": h.name,
                    "path": h.rel_path,
                    "size": h.size,
                    "is_dir": h.is_dir,
                })
            })
            .collect();
        Ok(json!({"pattern": pattern, "root": root, "total": total, "hits": items}))
    }

    fn delete(&self, args: &Value) -> Result<Value, String> {
        let path = required_str(args, "path")?;
        self.toolbox.remove(&path).map_err(render)?;
        Ok(json!({"path": path, "deleted": true}))
    }
}

impl ToolExecutor for FsToolExecutor {
    fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        self.dispatch(name, args)
    }
}

/// `agent-tools` 的错误 → 工具错误说明（变体可辨，宿主据此分类）。
///
/// ⛔ 不回显参数内容：路径可能含用户敏感信息，只保留 `agent-tools` 已渲染的说明。
fn render(error: ToolError) -> String {
    error.to_string()
}

/// 必填字符串参数：缺失或类型不对 ⇒ 显式错误（点名参数，不回显值）。
fn required_str(args: &Value, key: &str) -> Result<String, String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        Some(Value::String(_)) => Err(format!("参数 {key} 不得为空字符串")),
        Some(_) => Err(format!("参数 {key} 必须是字符串")),
        None => Err(format!("缺少必填参数 {key}")),
    }
}

/// 可选字符串参数：`None` 与显式 `null` 都视为缺省。
fn optional_str(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("参数 {key} 必须是字符串")),
    }
}

/// 可选整数参数：类型不对 ⇒ 显式错误（不回显值）。
fn optional_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("参数 {key} 必须是非负整数")),
        Some(_) => Err(format!("参数 {key} 必须是非负整数")),
    }
}

/// 可选无符号整数参数（分页/上限用）。
fn optional_usize(args: &Value, key: &str) -> Result<Option<usize>, String> {
    match optional_u64(args, key)? {
        None => Ok(None),
        Some(v) => usize::try_from(v)
            .map(Some)
            .map_err(|_| format!("参数 {key} 超出可表示范围")),
    }
}
