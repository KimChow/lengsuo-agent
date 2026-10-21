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

/// 提示词变体：同一模式下的**任务类型**维度（模式 × 变体 = 模板矩阵）。
///
/// 变体只追加「本轮关注什么」一段，**不改**能力面、公共层与硬规则；
/// 与模式、与风格层都正交。
///
/// 未知变体名回落 [`PromptVariant::Default`]（⛔ 不 panic、不返回 `Option`）：
/// 宿主把外部输入（配置 / 会话标签）转成变体时不因脏值崩掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromptVariant {
    /// 通用：不给模板加变体侧重。
    Default,
    /// 代码类：实现细节、边界与错误处理优先。
    Code,
    /// 编码类：完整实现、步骤顺序与异常状态优先。
    Coding,
    /// 设计类：结构、层次与可读性优先。
    Design,
}

impl PromptVariant {
    /// 固定顺序，便于遍历与快照测试。
    pub const ALL: [PromptVariant; 4] = [
        PromptVariant::Default,
        PromptVariant::Code,
        PromptVariant::Coding,
        PromptVariant::Design,
    ];

    /// 变体名识别：大小写不敏感，`-` / `_` 与无分隔等价；未知值回落 `Default`。
    ///
    /// ⛔ 永不 panic：脏输入的归一化结果是「默认变体」，不是错误。
    pub fn from_name(name: &str) -> Self {
        let normalized = name.trim().to_ascii_lowercase().replace(['-', '_'], "");
        match normalized.as_str() {
            "code" => PromptVariant::Code,
            "coding" => PromptVariant::Coding,
            "design" => PromptVariant::Design,
            _ => PromptVariant::Default,
        }
    }

    /// 变体名的稳定小写形态（`"default"` / `"code"` / `"coding"` / `"design"`）。
    pub fn name(self) -> &'static str {
        match self {
            PromptVariant::Default => "default",
            PromptVariant::Code => "code",
            PromptVariant::Coding => "coding",
            PromptVariant::Design => "design",
        }
    }
}

/// 注入模板的变量集。全部由调用方提供（引擎不取时钟、不读环境、不做 IO）。
///
/// 字段顺序即模板变量的分组顺序：产品 → 身份 → 环境 → 规则/风格 → 专家人格 →
/// 语言/平台 → 功能开关 → 工具呈现 → 记忆四段（身份/用户/项目/当日）→ 专家人格正文。
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
    /// 用户资料正文（USER.md）；`user_name` 只是称呼，不是档案。
    pub user: String,
    /// 引导内容（BOOTSTRAP.md）；仅 onboarding 分支渲染（`templates/identity.rs`）。
    pub bootstrap: String,
    /// 身份分支：`"onboarding"`（初始引导）或 `"normal"`（常态）。
    pub identity_mode: String,
    /// SOUL.md 路径（身份段 `Path:` 行）。
    pub soul_path: String,
    /// IDENTITY.md 路径。
    pub identity_path: String,
    /// USER.md 路径。
    pub user_path: String,
    /// BOOTSTRAP.md 路径（仅 onboarding 分支）。
    pub bootstrap_path: String,
    /// 工作区路径或描述。
    pub workspace: String,
    /// 日期文本（来源由调用方决定，保证输出确定性）。
    pub date: String,
    /// 用户自定义规则；非空时追加为独立段。
    pub user_rules: String,
    /// 风格层文本；本卡仅作数据面。
    pub tone_style: String,
    /// 专家人格注入点（参考端 `PluginAgentPrompt`，**预留**：当前无模板消费）。
    ///
    /// 与 [`Self::expert_persona`] 的分工见后者文档。
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
    /// 专家人格正文（来源：专家包 `agents/<agentName>.md` 正文）。
    ///
    /// 只承载**正文**与专家名：段标题、边界标签与声明由模板提供（见
    /// `templates/expert.rs`），空串或纯空白 ⇒ 人格槽整段省略（[`Segment::Var`] 语义）。
    /// 脱敏由调用方在填入前完成（本 crate 不依赖 `agent-core` 的 `Redactor`）。
    /// 与 `plugin_agent_prompt` 是两个变量：后者是预留的通用注入点，本字段是
    /// expert 模板的专用人格槽。
    pub expert_persona: String,
}

/// 模板可注入键的全集（snake_case，与字段名逐字一致）。
///
/// 布尔字段以 `"true"` / `"false"` 形式可被 `{{key}}` 替换；测试用它遍历断言
/// 「每个键都有值」，避免新增字段时漏挂 key。
pub const PROMPT_VAR_KEYS: [&str; 31] = [
    "product_name",
    "data_folder_name",
    "ai_name",
    "user_name",
    "soul",
    "identity",
    "user",
    "bootstrap",
    "identity_mode",
    "soul_path",
    "identity_path",
    "user_path",
    "bootstrap_path",
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
    "expert_persona",
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
            "user" => &self.user,
            "bootstrap" => &self.bootstrap,
            "identity_mode" => &self.identity_mode,
            "soul_path" => &self.soul_path,
            "identity_path" => &self.identity_path,
            "user_path" => &self.user_path,
            "bootstrap_path" => &self.bootstrap_path,
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
            "expert_persona" => &self.expert_persona,
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
            Cond::Contains(key, needle) => {
                vars.get(key).is_some_and(|value| value.contains(needle))
            }
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

/// 一个模板：模式 + 变体 + 段序列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    mode: PromptMode,
    variant: PromptVariant,
    segments: Vec<Segment>,
}

impl PromptTemplate {
    /// 构造**默认变体**的模板（签名不变 ⇒ 既有调用方零改动）。
    pub fn new(mode: PromptMode, segments: Vec<Segment>) -> Self {
        Self::new_with_variant(mode, PromptVariant::Default, segments)
    }

    /// 构造带变体标记的模板（变体只作为**元数据**随模板携带）。
    pub fn new_with_variant(
        mode: PromptMode,
        variant: PromptVariant,
        segments: Vec<Segment>,
    ) -> Self {
        Self {
            mode,
            variant,
            segments,
        }
    }

    pub fn mode(&self) -> PromptMode {
        self.mode
    }

    /// 本模板的变体（回落后的**实际**变体，不是宿主传入的原始值）。
    pub fn variant(&self) -> PromptVariant {
        self.variant
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// 渲染为提示词文本：段间以空行分隔，空槽位整段省略，末尾保留单个换行。
    ///
    /// 条件段（[`Segment::If`]）：条件成立才渲染其内层段；不成立则整段省略，
    /// 不留空洞标题、不留多余空行。
    ///
    /// 渲染管线在组装器（[`crate::compose`]）里：域模型只描述「段是什么」，
    /// 「怎么拼成文本」属组装职责。
    pub fn render(&self, vars: &PromptVars) -> String {
        crate::compose::render_template(self, vars)
    }
}
