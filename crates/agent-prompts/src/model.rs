//! 提示词引擎的域模型：模式、注入变量、模板段与渲染。

/// 提示词模式；能力面按 只答 → 只读 → 规划 → 全工具 → 专家 递增。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptMode {
    /// 只答不操作，无任何工具说明。
    Quick,
    /// 只读工具白名单说明。
    Ask,
    /// 复用 craft 模板并追加规划门禁。
    Plan,
    /// 全工具说明。
    Craft,
    /// craft 之上追加专家协作段。
    Expert,
}

impl PromptMode {
    /// 固定顺序，便于遍历与快照测试。
    pub const ALL: [PromptMode; 5] = [
        PromptMode::Quick,
        PromptMode::Ask,
        PromptMode::Plan,
        PromptMode::Craft,
        PromptMode::Expert,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PromptMode::Quick => "quick",
            PromptMode::Ask => "ask",
            PromptMode::Plan => "plan",
            PromptMode::Craft => "craft",
            PromptMode::Expert => "expert",
        }
    }
}

/// 注入模板的变量集。全部由调用方提供（引擎不取时钟、不读环境、不做 IO）。
///
/// 字段顺序即模板变量的分组顺序：产品 → 身份 → 环境 → 规则/风格 → 专家人格 →
/// 语言/平台 → 功能开关 → 工具呈现 → 记忆四段（身份/用户/项目/当日）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptVars {
    /// 产品名。
    pub product_name: String,
    /// 数据目录名。
    pub data_folder_name: String,
    /// 助手名称。
    pub ai_name: String,
    /// 用户称呼。
    pub user_name: String,
    /// 人格设定文本（SOUL.md）。
    pub soul: String,
    /// 身份层内容（IDENTITY.md）。
    pub identity: String,
    /// 引导内容（BOOTSTRAP.md）；onboarding 分支使用，本卡仅作数据面。
    pub bootstrap: String,
    /// 工作区路径或描述。
    pub workspace: String,
    /// 日期文本（来源由调用方决定，保证输出确定性）。
    pub date: String,
    /// 用户自定义规则；非空时追加为独立段。
    pub user_rules: String,
    /// 风格层文本；本卡仅作数据面。
    pub tone_style: String,
    /// 专家人格注入点。
    pub plugin_agent_prompt: String,
    /// 子代理人格；可留空。
    pub sub_agent_prompt: String,
    /// 回复语言。
    pub response_language: String,
    /// 是否 Windows 平台（平台条件段用；本卡仅作数据面）。
    pub is_windows: bool,
    /// 浏览器能力开关。
    pub feature_browser_use: bool,
    /// 桌面操作能力开关。
    pub feature_computer_use: bool,
    /// 专家管理能力开关。
    pub feature_expert_management: bool,
    /// 工具结果呈现提示。
    pub tool_result_presentation: String,
    /// 产物目录。
    pub artifact_dir: String,
    /// 记忆·身份层（AI 名/用户名/SOUL）。
    pub memory_identity: String,
    /// 记忆·用户记忆（上限 4000 字符，由注入方保证）。
    pub memory_user: String,
    /// 记忆·项目记忆（上限 3000 字符，由注入方保证）。
    pub memory_project: String,
    /// 记忆·当日日志尾（上限 2000 字符，由注入方保证）。
    pub memory_daily: String,
}

/// 模板可注入键的全集（snake_case，与字段名逐字一致）。
///
/// 布尔字段以 `"true"` / `"false"` 形式可被 `{{key}}` 替换；测试用它遍历断言
/// 「每个键都有值」，避免新增字段时漏挂 key。
pub const PROMPT_VAR_KEYS: [&str; 24] = [
    "product_name",
    "data_folder_name",
    "ai_name",
    "user_name",
    "soul",
    "identity",
    "bootstrap",
    "workspace",
    "date",
    "user_rules",
    "tone_style",
    "plugin_agent_prompt",
    "sub_agent_prompt",
    "response_language",
    "is_windows",
    "feature_browser_use",
    "feature_computer_use",
    "feature_expert_management",
    "tool_result_presentation",
    "artifact_dir",
    "memory_identity",
    "memory_user",
    "memory_project",
    "memory_daily",
];

impl PromptVars {
    /// 按变量名取值；未知键返回 `None`。布尔字段返回 `"true"` / `"false"`。
    pub fn get(&self, key: &str) -> Option<&str> {
        let value = match key {
            "product_name" => &self.product_name,
            "data_folder_name" => &self.data_folder_name,
            "ai_name" => &self.ai_name,
            "user_name" => &self.user_name,
            "soul" => &self.soul,
            "identity" => &self.identity,
            "bootstrap" => &self.bootstrap,
            "workspace" => &self.workspace,
            "date" => &self.date,
            "user_rules" => &self.user_rules,
            "tone_style" => &self.tone_style,
            "plugin_agent_prompt" => &self.plugin_agent_prompt,
            "sub_agent_prompt" => &self.sub_agent_prompt,
            "response_language" => &self.response_language,
            "is_windows" => bool_str(self.is_windows),
            "feature_browser_use" => bool_str(self.feature_browser_use),
            "feature_computer_use" => bool_str(self.feature_computer_use),
            "feature_expert_management" => bool_str(self.feature_expert_management),
            "tool_result_presentation" => &self.tool_result_presentation,
            "artifact_dir" => &self.artifact_dir,
            "memory_identity" => &self.memory_identity,
            "memory_user" => &self.memory_user,
            "memory_project" => &self.memory_project,
            "memory_daily" => &self.memory_daily,
            _ => return None,
        };
        Some(value)
    }
}

/// 布尔变量的文本形态（模板替换只需纯文本，无需真值语义）。
fn bool_str(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// 条件段判定式（07 册 §5 条件段实录的四类形态 + 「或」组合）。
///
/// 判定是**对 [`PromptVars`] 的纯函数**：无 IO、无模型调用、无时钟；
/// 同一份变量与模式必得同一结论（渲染确定性不因此放宽）。
///
/// 形态对应参考实录：
/// - `True`：`IsWindows` / `ExpertManagementEnabled` 一类的布尔开关；
/// - `Eq`：`workMode == "ask"` 一类的等值判断（左键为保留键 `work_mode` 时
///   与模板自身模式比较，其余为变量等值）；
/// - `AnyNonEmpty`：`WorkingMemoryContent or UserLocalMemoryContent or
///   UserMemoryContent` 一类的「任一非空」；
/// - `Contains`：`'中文' in ResponseLanguage` 一类的包含判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// 布尔变量为真（`is_windows` / `feature_expert_management` 等）。
    True(&'static str),
    /// 变量等于字面值；左键为保留键 `work_mode` 时与模板自身模式比较。
    Eq(&'static str, &'static str),
    /// 任一变量非空（`a or b or c` 形态）。
    AnyNonEmpty(&'static [&'static str]),
    /// 变量包含子串（`'中文' in response_language` 形态）。
    Contains(&'static str, &'static str),
    /// 若干条件的「或」组合（`A or B or C`）。
    Any(&'static [Cond]),
}

/// 判定 `work_mode` 时使用的保留键（值取自模板自身模式，不进 [`PromptVars`]）。
pub const WORK_MODE_KEY: &str = "work_mode";

impl Cond {
    /// 判定条件是否成立。
    ///
    /// 纯函数：只看变量值与模板模式，不读文件、不取时钟、不调用模型。
    pub fn holds(&self, vars: &PromptVars, mode: PromptMode) -> bool {
        match self {
            Cond::True(key) => vars.get(key) == Some("true"),
            Cond::Eq(key, value) => {
                if *key == WORK_MODE_KEY {
                    mode.as_str() == *value
                } else {
                    vars.get(key) == Some(*value)
                }
            }
            Cond::AnyNonEmpty(keys) => keys.iter().any(|key| is_filled(vars, key)),
            Cond::Contains(key, needle) => vars
                .get(key)
                .is_some_and(|value| value.contains(needle)),
            Cond::Any(conds) => conds.iter().any(|cond| cond.holds(vars, mode)),
        }
    }
}

/// 变量是否「有内容」：非纯空白；布尔字段的 `false` 视为无内容。
fn is_filled(vars: &PromptVars, key: &str) -> bool {
    match vars.get(key) {
        Some("false") => false,
        Some(value) => !value.trim().is_empty(),
        None => false,
    }
}

/// 模板的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// 字面文本，可含 `{{key}}` 注入点。
    Text(String),
    /// 独立段落槽位：`soul` / 记忆四段 / `user_rules` 带各自标题成段，
    /// 其余已知变量输出裸值，未知键与空值整段省略。
    Var(&'static str),
    /// 规划门禁片段（仅 plan 模板使用）。
    Gate,
    /// 条件段：条件成立才渲染整段，条件不成立则**整段省略**
    /// （与既有空值省略语义一致）；条件是 [`Cond`]（对变量的纯函数判定）。
    If(Cond, Vec<Segment>),
}

/// 一个模式的模板：模式 + 段序列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    mode: PromptMode,
    segments: Vec<Segment>,
}

impl PromptTemplate {
    pub fn new(mode: PromptMode, segments: Vec<Segment>) -> Self {
        Self { mode, segments }
    }

    pub fn mode(&self) -> PromptMode {
        self.mode
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// 渲染为提示词文本：段间以空行分隔，空槽位整段省略，末尾保留单个换行。
    ///
    /// 条件段（[`Segment::If`]）：条件成立才渲染其内层段；不成立则整段省略，
    /// 不留空洞标题、不留多余空行。
    pub fn render(&self, vars: &PromptVars) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(self.segments.len());
        render_segments(&self.segments, vars, self.mode, &mut parts);
        let mut out = parts.join("\n\n");
        out.push('\n');
        out
    }
}

/// 渲染一段段序列（含条件段的内层递归），把非空段追加到 `parts`。
fn render_segments(
    segments: &[Segment],
    vars: &PromptVars,
    mode: PromptMode,
    parts: &mut Vec<String>,
) {
    for segment in segments {
        match segment {
            Segment::Text(text) => push_part(parts, Some(substitute(text, vars))),
            Segment::Var(key) => push_part(parts, slot(key, vars)),
            Segment::Gate => push_part(parts, trim(crate::template::PLAN_GATE)),
            // 条件不成立 ⇒ 整段省略（与空值省略语义一致）。
            Segment::If(cond, inner) => {
                if cond.holds(vars, mode) {
                    render_segments(inner, vars, mode, parts);
                }
            }
        }
    }
}

/// 追加一段：空段（纯空白）不进列表，避免留下空洞标题。
fn push_part(parts: &mut Vec<String>, part: Option<String>) {
    if let Some(part) = part {
        if !part.trim().is_empty() {
            parts.push(part);
        }
    }
}

/// 单遍替换 `{{key}}`：未知键与值中的花括号原样保留，不递归展开。
fn substitute(text: &str, vars: &PromptVars) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = &after[..end];
                match vars.get(key) {
                    Some(value) => out.push_str(value),
                    // 未知键：原样保留花括号（内置模板不含未知键，注入值不再被替换）。
                    None => {
                        out.push_str("{{");
                        out.push_str(key);
                        out.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 槽位渲染：值非空（且非纯空白）时成段，否则整段省略，不留空洞标记。
/// 六段可独立成节（人格设定 / 记忆四段 / 用户自定义规则）；
/// 其余已知变量作为独立段时输出裸值；未知键省略。
fn slot(key: &'static str, vars: &PromptVars) -> Option<String> {
    let value = vars.get(key).and_then(trim)?;
    match key {
        "soul" => Some(format!("人格设定：\n{value}")),
        "memory_identity" => Some(format!("## 身份\n{value}")),
        "memory_user" => Some(format!("## 用户记忆\n{value}")),
        "memory_project" => Some(format!("## 项目记忆\n{value}")),
        "memory_daily" => Some(format!("## 当日日志\n{value}")),
        "user_rules" => Some(format!("## 用户自定义规则\n{value}")),
        _ => Some(value),
    }
}

/// 去掉首尾空白；全空白视为空（返回 `None`）。
fn trim(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
