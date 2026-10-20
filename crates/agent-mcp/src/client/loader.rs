//! `mcp.json` 加载器：路径约定、三条强校验与合并语义。
//!
//! 规格对齐 本地对照资料（不入库） 18 册 §9.2（原文亦见
//! `crates/agent-prompts/assets/safety-rules.md`）：
//! - 路径固定为 `<data_root>/mcp.json`——**无点前缀**，不是 `.mcp.json`；
//! - 格式与 Claude Desktop 相同：`{"mcpServers": {"<name>": {...}}}`；
//! - 读写前强校验：非法 JSON、`mcpServers` 非对象、server 名称为空；
//! - 合并**先读现有文件再并入**，只新增/覆盖同名 server，其他 server 与
//!   顶层其他键原样保留（不做「读-改-整表覆盖」）。
//!
//! 本模块只覆盖 stdio（`command`/`args`/`env`）。HTTP/SSE（`url`/`headers`）
//! 不在本卡范围：缺少字符串 `command` 的条目会显式报错，不会被静默丢弃。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::McpConfigError;

use super::{ClientConfig, StdioServerCommand};

/// 内存形态的 `mcp.json`：server 名 → 待 spawn 的 stdio 命令。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpJson {
    servers: BTreeMap<String, StdioServerCommand>,
}

impl McpJson {
    /// 全部 server 的只读视图。
    #[must_use]
    pub fn servers(&self) -> &BTreeMap<String, StdioServerCommand> {
        &self.servers
    }

    /// 取出内部的 server 表。
    #[must_use]
    pub fn into_servers(self) -> BTreeMap<String, StdioServerCommand> {
        self.servers
    }

    /// server 数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.servers.len()
    }

    /// 是否没有任何 server。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

/// 默认配置文件路径：`<data_root>/mcp.json`，**无点前缀**。
///
/// 参考规格专门强调不是 `.mcp.json`；本仓历史上出现过带点写法，必须避免。
#[must_use]
pub fn default_config_path(data_root: &Path) -> PathBuf {
    data_root.join("mcp.json")
}

/// 读取并解析 `mcp.json`。
///
/// # Errors
/// - [`McpConfigError::Io`]：文件不存在或不可读；
/// - [`McpConfigError::InvalidJson`]：内容不是合法 JSON（**不**返回空表）；
/// - [`McpConfigError::RootNotObject`]：顶层不是对象；
/// - [`McpConfigError::McpServersNotObject`]：`mcpServers` 存在但不是对象；
/// - [`McpConfigError::EmptyServerName`]：server 名为空串或全空白；
/// - [`McpConfigError::InvalidServer`]：条目不是 stdio 配置（含 HTTP/SSE）。
///
/// 合法 JSON 但缺少 `mcpServers` 视为空表；文件不存在则显式报错，避免把
/// 「还没配置」与「读失败」混为一谈。
pub fn load_from(path: &Path) -> Result<McpJson, McpConfigError> {
    let text = read_text(path)?;
    let root = parse_root(path, &text)?;
    let Some(servers) = root.get("mcpServers") else {
        return Ok(McpJson::default());
    };
    let Some(servers) = servers.as_object() else {
        return Err(McpConfigError::McpServersNotObject);
    };

    let mut parsed = BTreeMap::new();
    for (name, raw) in servers {
        validate_server_name(name)?;
        parsed.insert(name.clone(), parse_server(name, raw)?);
    }
    Ok(McpJson { servers: parsed })
}

/// 先读现有 `mcp.json` 再并入给定 server，只新增/覆盖同名项。
///
/// 未出现在 `servers` 里的 server（以及顶层其他键）原样保留；现有文件非法
/// 时直接报错且不改写原文件。文件不存在时新建。
///
/// # Errors
/// 与 [`load_from`] 相同的读取/校验错误，或写入失败产生的
/// [`McpConfigError::Io`]。
pub fn merge_into(
    path: &Path,
    servers: &BTreeMap<String, StdioServerCommand>,
) -> Result<(), McpConfigError> {
    // 入参先校验，任何失败都发生在触碰磁盘之前。
    for name in servers.keys() {
        validate_server_name(name)?;
    }

    let mut root = match std::fs::read_to_string(path) {
        Ok(text) => parse_root(path, &text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(io_error(path, error)),
    };
    let existing = mcp_servers_mut(&mut root)?;
    // 现有文件里的空名同样是非法配置，合并前拦下。
    for name in existing.keys() {
        validate_server_name(name)?;
    }
    for (name, command) in servers {
        existing.insert(name.clone(), command_to_value(name, command)?);
    }

    write_root(path, &root)
}

/// stdio 命令 → 客户端配置：先用 [`ClientConfig::default()`]，只替换 `command`。
///
/// 不改 [`ClientConfig`] 的字段或默认值（既有契约，`config.rs`）。
impl From<StdioServerCommand> for ClientConfig {
    fn from(command: StdioServerCommand) -> Self {
        Self {
            command,
            ..Self::default()
        }
    }
}

/// 供外部（含集成测试）按值比较两条启动命令。
///
/// 字段本身私有，这里只用 crate 内已有的只读访问器；不改变命令结构。
impl PartialEq for StdioServerCommand {
    fn eq(&self, other: &Self) -> bool {
        self.program() == other.program()
            && self.args_slice() == other.args_slice()
            && self.env_iter() == other.env_iter()
    }
}

impl Eq for StdioServerCommand {}

fn read_text(path: &Path) -> Result<String, McpConfigError> {
    std::fs::read_to_string(path).map_err(|error| io_error(path, error))
}

fn parse_root(path: &Path, text: &str) -> Result<Map<String, Value>, McpConfigError> {
    let value: Value = serde_json::from_str(text).map_err(|error| McpConfigError::InvalidJson {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    match value {
        Value::Object(root) => Ok(root),
        _ => Err(McpConfigError::RootNotObject),
    }
}

fn mcp_servers_mut(
    root: &mut Map<String, Value>,
) -> Result<&mut Map<String, Value>, McpConfigError> {
    if !root.contains_key("mcpServers") {
        root.insert("mcpServers".to_string(), Value::Object(Map::new()));
    }
    match root.get_mut("mcpServers") {
        Some(Value::Object(servers)) => Ok(servers),
        _ => Err(McpConfigError::McpServersNotObject),
    }
}

fn validate_server_name(name: &str) -> Result<(), McpConfigError> {
    if name.trim().is_empty() {
        Err(McpConfigError::EmptyServerName)
    } else {
        Ok(())
    }
}

fn invalid(name: &str, reason: &str) -> McpConfigError {
    McpConfigError::InvalidServer {
        name: name.to_string(),
        reason: reason.to_string(),
    }
}

fn parse_server(name: &str, raw: &Value) -> Result<StdioServerCommand, McpConfigError> {
    let entry = raw
        .as_object()
        .ok_or_else(|| invalid(name, "server 条目必须是对象"))?;
    let program = entry
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid(
                name,
                "仅支持 stdio：缺少字符串字段 command（HTTP/SSE 暂不支持）",
            )
        })?;
    if program.trim().is_empty() {
        return Err(invalid(name, "command 不能为空"));
    }

    let mut command = StdioServerCommand::new(program);
    if let Some(raw_args) = entry.get("args") {
        let args = raw_args
            .as_array()
            .ok_or_else(|| invalid(name, "args 必须是字符串数组"))?;
        let mut parsed = Vec::with_capacity(args.len());
        for arg in args {
            let arg = arg
                .as_str()
                .ok_or_else(|| invalid(name, "args 必须是字符串数组"))?;
            parsed.push(arg.to_string());
        }
        command = command.with_args(parsed);
    }
    if let Some(raw_env) = entry.get("env") {
        let env = raw_env
            .as_object()
            .ok_or_else(|| invalid(name, "env 必须是字符串到字符串的对象"))?;
        for (key, value) in env {
            let value = value
                .as_str()
                .ok_or_else(|| invalid(name, "env 必须是字符串到字符串的对象"))?;
            command = command.with_env(key, value);
        }
    }
    Ok(command)
}

fn command_to_value(name: &str, command: &StdioServerCommand) -> Result<Value, McpConfigError> {
    if command.program().trim().is_empty() {
        return Err(invalid(name, "command 不能为空"));
    }
    let mut entry = Map::new();
    entry.insert(
        "command".to_string(),
        Value::String(command.program().to_string()),
    );
    if !command.args_slice().is_empty() {
        entry.insert(
            "args".to_string(),
            Value::Array(
                command
                    .args_slice()
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    if !command.env_iter().is_empty() {
        let env = command
            .env_iter()
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        entry.insert("env".to_string(), Value::Object(env));
    }
    Ok(Value::Object(entry))
}

fn write_root(path: &Path, root: &Map<String, Value>) -> Result<(), McpConfigError> {
    let mut text = serde_json::to_string_pretty(root).map_err(|error| McpConfigError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| io_error(path, error))
}

fn io_error(path: &Path, error: std::io::Error) -> McpConfigError {
    McpConfigError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    }
}
