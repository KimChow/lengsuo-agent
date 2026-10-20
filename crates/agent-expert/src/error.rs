//! 专家包错误类型（错误码全集 + 严重级别）。
//!
//! 规格：调研 `harness-expert.md` §17「校验面」——25 项检查、每项一个错误码；
//! §1.3 规定 `DUPLICATE_NAME_PROFESSION` 与 `CONNECTOR_DECL_INVALID` 是
//! **Warning**（不阻断注册），其余为 **Error**（阻断）。
//!
//! 本文件是卡 1 冻结的错误契约：校验卡（卡 2）**不改本文件**，需要文件级
//! 专属变体时另建 `validate_errors.rs`，不得重复定义同名语义。

use std::path::PathBuf;

/// 严重级别：Error 阻断注册，Warning 仅提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// 阻断（校验 fail-closed：任一 Error 即整体失败）。
    Error,
    /// 仅提示，不阻断注册。
    Warning,
}

/// 专家包相关错误全集。
///
/// 每个变体对应调研 §17 表格的一个错误码；`code()` 返回该码（稳定字符串，
/// 供 UI 展示与卡 2 的断言使用），`severity()` 返回严重级别。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExpertError {
    /// 专家目录缺少 `.lengsuo-plugin/plugin.json`（§17 #1）。
    #[error("专家包缺少 manifest: {path}")]
    PluginManifestMissing { path: PathBuf },

    /// `plugin.json` 不是合法 JSON，或结构与字段类型不符（§17 #2）。
    #[error("manifest 解析失败: {reason}")]
    ManifestParse { reason: String },

    /// 必填字段缺失（§17 #2 派生；§3.1 判定表的「必填」列）。
    #[error("manifest 缺少必填字段: {field}")]
    MissingField { field: String },

    /// 字段类型错（§17 #2 派生）。
    #[error("manifest 字段 {field} 类型错：期望 {expected}，实际 {actual}")]
    TypeMismatch {
        field: String,
        expected: &'static str,
        actual: &'static str,
    },

    /// `name` 不是 kebab-case（§17 #3 / 铁律 1）。
    #[error("name 不是 kebab-case: {value}")]
    InvalidName { value: String },

    /// `name` 与目录名不一致（§17 #3）。
    #[error("name({name}) 与目录名({dir}) 不一致")]
    NameDirMismatch { name: String, dir: String },

    /// `version` 不是 MAJOR.MINOR.PATCH（§17 #4）。
    #[error("version 不是语义化版本: {value}")]
    InvalidVersion { value: String },

    /// `expertType` 声明与实际结构不一致（§17 #5）。
    #[error("expertType 与实际结构不一致: {reason}")]
    TypeStructureMismatch { reason: String },

    /// `agentName` 与 `agents/*.md` 的 stem 不符（§17 #6 / 铁律 2）。
    #[error("agentName({expected}) 与 agents 文件名({actual}) 不一致")]
    AgentNameMismatch { expected: String, actual: String },

    /// `agents[]` 指向的文件不存在（§17 #7）。
    #[error("agents 路径不存在: {path}")]
    AgentPathMissing { path: String },

    /// 路径越出专家包目录（§17 #7 / 铁律 3）。
    #[error("路径越出专家包目录: {path}")]
    PathEscape { path: String },

    /// Agent MD 的 front-matter 声明了 `tools`（§17 #8 / 铁律 6）。
    #[error("人格文件禁止声明 tools: {path}")]
    ForbiddenToolDeclaration { path: String },

    /// Agent MD 的 front-matter `name` 与文件 stem 不符（§17 #9）。
    #[error("人格文件 frontmatter name({actual}) 与文件名({expected}) 不一致")]
    AgentFrontmatterNameMismatch { expected: String, actual: String },

    /// `tags` 项数不为 3（§17 #10 / 铁律 4）。
    #[error("tags 应为 3 项，实际 {count} 项")]
    TagCount { count: usize },

    /// `quickPrompts` 项数不为 3（§17 #10 / 铁律 4）。
    #[error("quickPrompts 应为 3 项，实际 {count} 项")]
    QuickPromptCount { count: usize },

    /// `defaultInitPrompt` 与 `quickPrompts[0]` 不一致（§17 #11 / 铁律 4）。
    #[error("defaultInitPrompt 与 quickPrompts[0] 不一致")]
    InitPromptMismatch,

    /// `displayDescription.zh` 字符数不在 40–50 区间（§17 #12 / 铁律 5）。
    #[error("displayDescription.zh 长度 {count} 不在 40–50 区间")]
    DescriptionLength { count: usize },

    /// `plugin` 与 `name` 不一致（§17 #13）。
    #[error("plugin({actual}) 与 name({expected}) 不一致")]
    PluginNameMismatch { expected: String, actual: String },

    /// `categoryId` 不在 12 个分类枚举内（§17 #14）。
    #[error("未知 categoryId: {value}")]
    UnknownCategory { value: String },

    /// 团队型没有且仅有一个 `role=="lead"` 的成员（§17 #15 / 铁律 8）。
    #[error("members 中主理人缺失或不唯一")]
    TeamLeadMissing,

    /// `teamInfo.memberAgents` 与非主理人成员 id 集合不一致（§17 #16 / 铁律 8）。
    #[error("teamInfo.memberAgents({actual}) 与成员集合({expected}) 不一致")]
    TeamMembersMismatch { expected: String, actual: String },

    /// 团队型的 `profession` 与 `displayName` 不一致（§17 #17 / 铁律 9）。
    #[error("team 型 profession 与 displayName 不一致")]
    TeamProfessionDisplayMismatch,

    /// 团队型主理人用了通用名（§17 #18 / 铁律 7）。
    #[error("主理人 agentName 不得使用通用名: {name}")]
    GenericAgentName { name: String },

    /// `skills[]` 指向的目录不存在或缺 `SKILL.md`（§17 #19）。
    #[error("技能路径不存在或缺 SKILL.md: {path}")]
    SkillPathMissing { path: String },

    /// `skills/**/scripts/*.py` 语法不可解析（§17 #20）。
    #[error("脚本语法错误 {path}: {reason}")]
    ScriptSyntax { path: String, reason: String },

    /// 声明了 `avatar` 但文件不存在（§17 #21 / 铁律 10）。
    #[error("头像文件不存在: {path}")]
    AvatarMissing { path: String },

    /// 头像超过 500KB（§17 #21 / 铁律 10）。
    #[error("头像 {path} 体积 {bytes} 字节，超出上限 {limit} 字节")]
    AvatarTooLarge {
        path: String,
        bytes: u64,
        limit: u64,
    },

    /// 不可变字段被改动（§17 #22）。
    #[error("不可变字段不得修改: {field}")]
    ImmutableField { field: String },

    /// `members[].name` 与 `profession` 同语言侧相等（§17 #23，**Warning**）。
    #[error("成员 {member_id} 的 name 与 profession 重复")]
    DuplicateNameProfession { member_id: String },

    /// `dependencies.connectors` 声明无法解析（§17 #24，**Warning**）。
    #[error("connector 声明不可用: {reason}")]
    ConnectorDeclInvalid { reason: String },

    /// 同名专家已注册（§17 #25）。
    #[error("专家已注册: {name}")]
    AlreadyRegistered { name: String },

    /// 文件系统访问失败。
    ///
    /// ⚠️ 存 `reason: String` 而非 `std::io::Error`：`std::io::Error` **不实现
    /// `Clone`**，而本枚举需要 `Clone`（校验器要聚合/拷贝错误）。代价是丢掉
    /// `#[source]` 链，错误分类由 `code()` 承担。
    #[error("IO 失败 {path:?}: {reason}")]
    Io { path: PathBuf, reason: String },
}

impl ExpertError {
    /// 由 `std::io::Error` 构造 IO 错误。
    pub fn io(path: impl Into<PathBuf>, source: &std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            reason: source.to_string(),
        }
    }
}

impl ExpertError {
    /// 错误码（调研 §17 表格「失败报什么错（code）」列）。
    ///
    /// 码是稳定字符串：UI 直接展示，卡 2 的负例测试按码断言。
    pub fn code(&self) -> &'static str {
        match self {
            Self::PluginManifestMissing { .. } => "PLUGIN_MANIFEST_MISSING",
            Self::ManifestParse { .. } => "MANIFEST_PARSE_FAILED",
            Self::MissingField { .. } => "MISSING_FIELD",
            Self::TypeMismatch { .. } => "TYPE_MISMATCH",
            Self::InvalidName { .. } => "INVALID_NAME",
            Self::NameDirMismatch { .. } => "NAME_DIR_MISMATCH",
            Self::InvalidVersion { .. } => "INVALID_VERSION",
            Self::TypeStructureMismatch { .. } => "TYPE_STRUCTURE_MISMATCH",
            Self::AgentNameMismatch { .. } => "AGENT_NAME_MISMATCH",
            Self::AgentPathMissing { .. } => "AGENT_PATH_MISSING",
            Self::PathEscape { .. } => "PATH_ESCAPE",
            Self::ForbiddenToolDeclaration { .. } => "FORBIDDEN_TOOL_DECLARATION",
            Self::AgentFrontmatterNameMismatch { .. } => "AGENT_FRONTMATTER_NAME_MISMATCH",
            Self::TagCount { .. } => "TAG_COUNT",
            Self::QuickPromptCount { .. } => "QUICK_PROMPT_COUNT",
            Self::InitPromptMismatch => "INIT_PROMPT_MISMATCH",
            Self::DescriptionLength { .. } => "DESCRIPTION_LENGTH",
            Self::PluginNameMismatch { .. } => "PLUGIN_NAME_MISMATCH",
            Self::UnknownCategory { .. } => "UNKNOWN_CATEGORY",
            Self::TeamLeadMissing => "TEAM_LEAD_MISSING",
            Self::TeamMembersMismatch { .. } => "TEAM_MEMBERS_MISMATCH",
            Self::TeamProfessionDisplayMismatch => "TEAM_PROFESSION_DISPLAY_MISMATCH",
            Self::GenericAgentName { .. } => "GENERIC_AGENT_NAME",
            Self::SkillPathMissing { .. } => "SKILL_PATH_MISSING",
            Self::ScriptSyntax { .. } => "SCRIPT_SYNTAX",
            Self::AvatarMissing { .. } => "AVATAR_MISSING",
            Self::AvatarTooLarge { .. } => "AVATAR_TOO_LARGE",
            Self::ImmutableField { .. } => "IMMUTABLE_FIELD",
            Self::DuplicateNameProfession { .. } => "DUPLICATE_NAME_PROFESSION",
            Self::ConnectorDeclInvalid { .. } => "CONNECTOR_DECL_INVALID",
            Self::AlreadyRegistered { .. } => "ALREADY_REGISTERED",
            Self::Io { .. } => "IO_ERROR",
        }
    }

    /// 严重级别（调研 §17「级别」列）。
    pub fn severity(&self) -> Severity {
        match self {
            Self::DuplicateNameProfession { .. } | Self::ConnectorDeclInvalid { .. } => {
                Severity::Warning
            }
            _ => Severity::Error,
        }
    }
}
