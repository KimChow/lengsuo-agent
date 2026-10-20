//! 工具注册表（开工说明 §1、§4）。
//!
//! 注册表只提供能力描述与元数据，不执行副作用；执行由宿主（网关/后续实现）承担。
//! 浏览器工具以 trait 占位，C/D 对接时再接。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具类别；决定 readonly 权限模式下是否可用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// 只读域逻辑（列目录、读文件）。
    Read,
    /// 写本地状态。
    Write,
    /// 执行命令。
    Exec,
    /// 发起网络访问。
    Network,
    /// 操作浏览器环境。
    Browser,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Read => "read",
            ToolKind::Write => "write",
            ToolKind::Exec => "exec",
            ToolKind::Network => "network",
            ToolKind::Browser => "browser",
        }
    }

    /// readonly 权限模式是否允许该类别。
    pub fn allowed_when_readonly(self) -> bool {
        matches!(self, ToolKind::Read)
    }
}

/// 一个工具的静态描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub kind: ToolKind,
    /// 参数可能触发敏感动作（如命令字符串）：需要过敏感动作正则。
    pub sensitive_arguments: bool,
    pub timeout_ms: u64,
}

/// 工具注册表。
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolSpec>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册（同名覆盖）。
    pub fn register(&mut self, spec: ToolSpec) {
        self.tools.insert(spec.name.clone(), spec);
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name)
    }

    /// 按名称排序列出全部工具。
    pub fn list(&self) -> Vec<&ToolSpec> {
        self.tools.values().collect()
    }

    /// 指定模式白名单内可见的工具（白名单缺失视为无工具）。
    pub fn list_allowed(
        &self,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<&ToolSpec> {
        let Some(allowed) = allowed else {
            return Vec::new();
        };
        self.tools
            .values()
            .filter(|t| allowed.contains(&t.name))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 序列化为网关 `tool:list` 的 data。
    pub fn to_value(&self, allowed: Option<&std::collections::BTreeSet<String>>) -> Vec<Value> {
        let items: Vec<&ToolSpec> = match allowed {
            Some(set) => self
                .tools
                .values()
                .filter(|t| set.contains(&t.name))
                .collect(),
            None => self.tools.values().collect(),
        };
        items
            .iter()
            .map(|t| serde_json::to_value(*t).expect("ToolSpec 可序列化"))
            .collect()
    }
}

/// 记忆写工具的执行体取值键（与 [`crate::runtime::Runtime::execute_memory_tool`]
/// 的 `arg("date")` / `arg("text")` 一一对应）；注册表描述按此拼写，避免两处漂移。
/// 生产与测试共用同一份定义，注册表是工具名的**唯一事实来源**。
pub const MEMORY_WRITE_TOOL_SPECS: [(&str, &str); 4] = [
    (
        "memory.set_user",
        "写入用户级记忆；参数 text（string，必填）。",
    ),
    (
        "memory.set_project",
        "写入项目级记忆；参数 text（string，必填）。",
    ),
    (
        "memory.append_daily",
        "按日期追加当日记忆；参数 date（string，必填，YYYY-MM-DD）、text（string，必填）。",
    ),
    (
        "identity.set_soul",
        "写入 AI 人格（soul）；参数 text（string，必填）。",
    ),
];

/// 平台内置工具的默认注册表（生产路径与测试共用同一份定义，避免两处漂移）。
///
/// 先只收记忆写工具四项（M1）；`fs.*` / `shell.exec` 等名字的执行体状态尚未核实，
/// 由后续各领域卡追加，避免造出「已注册但执行不了」的假象。
pub fn default_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for (name, description) in MEMORY_WRITE_TOOL_SPECS {
        registry.register(ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            kind: ToolKind::Write,
            // 参数都是用户内容，可能含凭据样式串 —— 让权限门的敏感动作扫描生效。
            sensitive_arguments: true,
            timeout_ms: 5_000,
        });
    }
    registry
}

/// 浏览器工具占位 trait（开工说明 §1：C/D 对接时再接实现）。
pub trait BrowserTool: Send + Sync {
    /// 打开指定环境的 URL；v0 不实现，返回 `Unsupported`。
    fn open(&self, env_id: &str, url: &str) -> Result<Value, crate::error::CoreError>;
}

/// 默认空实现：保证 trait 有可链接的占位，且不误报能力。
pub struct NoopBrowserTool;

impl BrowserTool for NoopBrowserTool {
    fn open(&self, _env_id: &str, _url: &str) -> Result<Value, crate::error::CoreError> {
        Err(crate::error::CoreError::Unsupported(
            "浏览器工具未接入（轨道 C 对接后启用）",
        ))
    }
}
