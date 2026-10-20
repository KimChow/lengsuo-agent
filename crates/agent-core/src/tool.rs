//! 工具注册表（开工说明 §1、§4）。
//!
//! 注册表只提供能力描述与元数据，不执行副作用；执行由宿主（网关/后续实现）承担。
//! 浏览器工具以 trait 占位，C/D 对接时再接。

use std::collections::BTreeMap;

use agent_memory::{
    ListMemoriesRequest, ReadMemoryRequest, SearchMatchMode, SearchMemoriesRequest,
    DEFAULT_LIST_MAX_RESULTS, DEFAULT_READ_MAX_TOKENS, DEFAULT_SEARCH_MAX_RESULTS,
    MAX_LIST_RESULTS, MAX_SEARCH_RESULTS,
};
use serde::de::DeserializeOwned;
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

/// 记忆检索工具名（与 `crate::mode::MEMORY_READONLY_TOOLS` 逐字一致）。
pub const MEMORY_LIST_TOOL: &str = "memory.list";
pub const MEMORY_SEARCH_TOOL: &str = "memory.search";
pub const MEMORY_READ_TOOL: &str = "memory.read";

/// 三个检索工具名全表（装配与测试的单一来源）。
pub const MEMORY_SEARCH_TOOLS: [&str; 3] = [MEMORY_LIST_TOOL, MEMORY_SEARCH_TOOL, MEMORY_READ_TOOL];

/// 「当前会话不可查」纪律原文（调研 §5.2，照 wb 措辞；search / read 描述必含）。
pub const MEMORY_CURRENT_SESSION_NOT_SEARCHABLE: &str = "检索本地记忆文件。注意：本工具**对本会话零访问**——查询必须自包含（描述要找什么 + 时间范围背景）；**不得用于查当前对话**，那是会话历史的职责。";

/// `path` 双根语义（本仓两个记忆根；codex 只有一个——唯一刻意偏离）。
pub const MEMORY_PATH_SEMANTICS: &str = "path 相对记忆根：user: = ~/.lengsuo/memory，project: = <ws>/.lengsuo/memory；省略前缀时默认 project 根，返回路径带 user:/project: 前缀。";

/// 三个检索工具的 `ToolSpec` 描述（参数名/默认值/上限照抄 codex）。
pub const MEMORY_SEARCH_TOOL_SPECS: [(&str, &str); 3] = [
    (
        MEMORY_LIST_TOOL,
        concat!(
            "列出记忆根下直接子项（文件/目录，不递归）。",
            "参数 path（可选）、cursor（可选）、max_results（可选，默认 2000，上限 2000，最小 1）。",
            "path 相对记忆根：user: = ~/.lengsuo/memory，project: = <ws>/.lengsuo/memory；",
            "省略前缀时默认 project 根，返回路径带 user:/project: 前缀。",
        ),
    ),
    (
        MEMORY_SEARCH_TOOL,
        concat!(
            "检索本地记忆文件。注意：本工具**对本会话零访问**——查询必须自包含",
            "（描述要找什么 + 时间范围背景）；**不得用于查当前对话**，那是会话历史的职责。",
            "参数 queries（必填，长度 ≥1，每项去空白后非空）、",
            "match_mode（可选：any | all_on_same_line | all_within_lines{line_count}，默认 any）、",
            "path、cursor、context_lines（默认 0）、case_sensitive（默认 true）、",
            "normalized（默认 false）、max_results（默认 200，上限 200，最小 1）。",
            "path 相对记忆根：user: = ~/.lengsuo/memory，project: = <ws>/.lengsuo/memory；",
            "省略前缀时默认 project 根，返回路径带 user:/project: 前缀。",
        ),
    ),
    (
        MEMORY_READ_TOOL,
        concat!(
            "检索本地记忆文件。注意：本工具**对本会话零访问**——查询必须自包含",
            "（描述要找什么 + 时间范围背景）；**不得用于查当前对话**，那是会话历史的职责。",
            "读取一个记忆文件。参数 path（必填）、line_offset（可选，1-indexed，默认 1）、",
            "max_lines（可选，最小 1，默认不限）。",
            "path 相对记忆根：user: = ~/.lengsuo/memory，project: = <ws>/.lengsuo/memory；",
            "省略前缀时默认 project 根，返回路径带 user:/project: 前缀。",
        ),
    ),
];

/// `memory.list` 参数（照抄 codex `tools/list.rs:27-34`，`deny_unknown_fields`）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryListArgs {
    path: Option<String>,
    cursor: Option<String>,
    max_results: Option<usize>,
}

/// `memory.search` 参数（照抄 codex `tools/search.rs:28-42`，`deny_unknown_fields`）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemorySearchArgs {
    queries: Vec<String>,
    match_mode: Option<SearchMatchMode>,
    path: Option<String>,
    cursor: Option<String>,
    context_lines: Option<usize>,
    case_sensitive: Option<bool>,
    normalized: Option<bool>,
    max_results: Option<usize>,
}

/// `memory.read` 参数（照抄 codex `tools/read.rs:25-33`，`deny_unknown_fields`）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryReadArgs {
    path: String,
    line_offset: Option<usize>,
    max_lines: Option<usize>,
}

/// codex `tools/mod.rs:91-93` 的同口径夹取：默认值兜底后夹到 `[1, max]`。
pub fn clamp_max_results(requested: Option<usize>, default: usize, max: usize) -> usize {
    requested.unwrap_or(default).clamp(1, max)
}

/// 参数反序列化：形状错误 ⇒ 可读错误（不 panic、不静默忽略未知字段）。
fn parse_memory_args<T: DeserializeOwned>(tool: &str, args: &Value) -> Result<T, String> {
    serde_json::from_value(args.clone()).map_err(|err| format!("{tool} 参数非法: {err}"))
}

/// `memory.list` 参数 → 后端请求（schema 层与实现层都夹 `max_results`）。
pub fn parse_memory_list_args(args: &Value) -> Result<ListMemoriesRequest, String> {
    let parsed: MemoryListArgs = parse_memory_args(MEMORY_LIST_TOOL, args)?;
    if parsed.max_results == Some(0) {
        return Err(format!("{MEMORY_LIST_TOOL} 参数非法: max_results 必须 ≥ 1"));
    }
    Ok(ListMemoriesRequest {
        path: parsed.path,
        cursor: parsed.cursor,
        max_results: clamp_max_results(
            parsed.max_results,
            DEFAULT_LIST_MAX_RESULTS,
            MAX_LIST_RESULTS,
        ),
    })
}

/// `memory.search` 参数 → 后端请求（默认值照抄 codex；空查询/0 上限在校验层拒绝）。
pub fn parse_memory_search_args(args: &Value) -> Result<SearchMemoriesRequest, String> {
    let parsed: MemorySearchArgs = parse_memory_args(MEMORY_SEARCH_TOOL, args)?;
    if parsed.queries.is_empty() || parsed.queries.iter().any(|query| query.trim().is_empty()) {
        return Err(format!(
            "{MEMORY_SEARCH_TOOL} 参数非法: queries 不能为空或包含空字符串"
        ));
    }
    if parsed.max_results == Some(0) {
        return Err(format!(
            "{MEMORY_SEARCH_TOOL} 参数非法: max_results 必须 ≥ 1"
        ));
    }
    let match_mode = parsed.match_mode.unwrap_or(SearchMatchMode::Any);
    if matches!(
        match_mode,
        SearchMatchMode::AllWithinLines { line_count: 0 }
    ) {
        return Err(format!(
            "{MEMORY_SEARCH_TOOL} 参数非法: all_within_lines.line_count 必须 ≥ 1"
        ));
    }
    Ok(SearchMemoriesRequest {
        queries: parsed.queries,
        match_mode,
        path: parsed.path,
        cursor: parsed.cursor,
        context_lines: parsed.context_lines.unwrap_or(0),
        case_sensitive: parsed.case_sensitive.unwrap_or(true),
        normalized: parsed.normalized.unwrap_or(false),
        max_results: clamp_max_results(
            parsed.max_results,
            DEFAULT_SEARCH_MAX_RESULTS,
            MAX_SEARCH_RESULTS,
        ),
    })
}

/// `memory.read` 参数 → 后端请求（`max_tokens` 固定，不由模型控制）。
pub fn parse_memory_read_args(args: &Value) -> Result<ReadMemoryRequest, String> {
    let parsed: MemoryReadArgs = parse_memory_args(MEMORY_READ_TOOL, args)?;
    if parsed.line_offset == Some(0) {
        return Err(format!(
            "{MEMORY_READ_TOOL} 参数非法: line_offset 必须 ≥ 1（1-indexed）"
        ));
    }
    if parsed.max_lines == Some(0) {
        return Err(format!("{MEMORY_READ_TOOL} 参数非法: max_lines 必须 ≥ 1"));
    }
    Ok(ReadMemoryRequest {
        path: parsed.path,
        line_offset: parsed.line_offset.unwrap_or(1),
        max_lines: parsed.max_lines,
        max_tokens: DEFAULT_READ_MAX_TOKENS,
    })
}

/// 平台内置工具的默认注册表（生产路径与测试共用同一份定义，避免两处漂移）。
///
/// 先收记忆写工具四项（M1）+ 检索只读工具三项（M2）；`fs.*` / `shell.exec`
/// 等名字的执行体状态尚未核实，由后续各领域卡追加，避免造出「已注册但执行不了」
/// 的假象。
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
    for (name, description) in MEMORY_SEARCH_TOOL_SPECS {
        registry.register(ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            kind: ToolKind::Read,
            // 只读检索：不写任何状态；查询串不触发「敏感动作强制 ask」。
            sensitive_arguments: false,
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
