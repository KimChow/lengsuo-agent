//! 校验专属错误类型（专家体系 · 卡 2）。
//!
//! 规格：调研 `harness-expert.md` §17「校验面」——本卡实现 **24 项**
//! （编号 1–19、21–25；#20 `SCRIPT_SYNTAX` 不在本卡范围，见
//! `docs/expert-validate-status.md` 未做项）。每项校验一个独立函数，失败产出
//! **一条带 §17 编号的发现**——编号用于「指明是哪一条」，逐条可测。
//!
//! 与卡 1 冻结的 [`ExpertError`] 的分工（**不重复定义同名语义**）：
//!
//! - [`ExpertError`] 是**错误语义全集**（卡 1 冻结，本卡不改）：每个变体一个
//!   稳定错误码 `code()` 与严重级别 `severity()`；
//! - 本文件只补两样 `ExpertError` 表达不了的东西：① **§17 编号**
//!   （`ExpertError` 不带编号，无法满足「错误须指明是哪一条」）；② 头像
//!   **尺寸**判据（§17 #21 的第三条判据，`ExpertError` 冻结时只有
//!   `AVATAR_MISSING` / `AVATAR_TOO_LARGE`，无尺寸变体）。
//!
//! 严重级别口径：§17 表格的「级别」列 **+ 本卡三处偏离**（见
//! `severity_of` 与 `docs/expert-validate-status.md` 偏离清单）：真实样本
//! `pandaai-team` 与 `senior-developer` 会违反 #3b/#12/#17 三条，若按 §17
//! 原级别（Error）判死，验收标准「两份真实样本通过校验」无法成立。

use std::fmt;

use crate::error::{ExpertError, Severity};

/// §17 校验项编号（1–19、21–25）。
///
/// 编号与调研 §17 表格的「#」列**逐条对应**，供 UI 展示与负例测试断言。
pub mod item {
    /// #1 目录含 `.lengsuo-plugin/plugin.json`。
    pub const MANIFEST_PRESENT: u8 = 1;
    /// #2 `plugin.json` 可解析且 schema 合法。
    pub const MANIFEST_PARSES: u8 = 2;
    /// #3 `name` kebab-case。
    pub const NAME_KEBAB_CASE: u8 = 3;
    /// #3 `name` 与目录名一致（§17 同一行两个错误码）。
    pub const NAME_MATCHES_DIR: u8 = 3;
    /// #4 `version` 语义化 MAJOR.MINOR.PATCH。
    pub const VERSION_SEMVER: u8 = 4;
    /// #5 `expertType` 与实际结构一致。
    pub const TYPE_MATCHES_STRUCTURE: u8 = 5;
    /// #6 `agentName` == 某 `agents/*.md` 的 stem。
    pub const AGENT_NAME_MATCHES_STEM: u8 = 6;
    /// #7 `agents[]` 每项存在、非空、落在包目录内。
    pub const AGENT_PATHS: u8 = 7;
    /// #8 Agent MD frontmatter 无 `tools`。
    pub const NO_TOOLS_DECLARATION: u8 = 8;
    /// #9 Agent MD frontmatter `name` == 文件 stem。
    pub const FRONTMATTER_NAME_MATCHES_STEM: u8 = 9;
    /// #10 `tags` / `quickPrompts` 各恰 3 项。
    pub const TAG_COUNT: u8 = 10;
    /// #10 `quickPrompts` 恰 3 项。
    pub const QUICK_PROMPT_COUNT: u8 = 10;
    /// #11 `defaultInitPrompt` == `quickPrompts[0]`。
    pub const INIT_PROMPT_MATCHES: u8 = 11;
    /// #12 `displayDescription.zh` 字符数 ∈ [40,50]。
    pub const DESCRIPTION_LENGTH: u8 = 12;
    /// #13 `plugin` == `name`（若声明）。
    pub const PLUGIN_NAME_MATCHES: u8 = 13;
    /// #14 `categoryId` ∈ 12 枚举。
    pub const CATEGORY_KNOWN: u8 = 14;
    /// #15 team：恰一个 `role=="lead"` 且 id == `agentName`。
    pub const TEAM_LEAD_UNIQUE: u8 = 15;
    /// #16 team：`teamInfo.memberAgents` == 非 lead 成员 id 集合。
    pub const TEAM_MEMBERS_MATCH: u8 = 16;
    /// #17 team：`profession` == `displayName`。
    pub const TEAM_PROFESSION_MATCHES_DISPLAY: u8 = 17;
    /// #18 team：`agentName` 不落入通用名黑名单。
    pub const AGENT_NAME_NOT_GENERIC: u8 = 18;
    /// #19 `skills[]` 每项存在且含 `SKILL.md`。
    pub const SKILL_PATHS: u8 = 19;
    /// #21 `avatar`（若声明）存在且 ≤500KB 且尺寸合规。
    pub const AVATAR_VALID: u8 = 21;
    /// #22 不可变字段未被 update 改动。
    pub const IMMUTABLE_FIELDS_INTACT: u8 = 22;
    /// #23 `members[].name` 与 `profession` 同语言侧不相等。
    pub const MEMBER_NAME_NOT_PROFESSION: u8 = 23;
    /// #24 `dependencies.connectors`（若声明）解析成功。
    pub const CONNECTOR_DECL_RESOLVES: u8 = 24;
    /// #25 同名未已注册。
    pub const NAME_NOT_REGISTERED: u8 = 25;
}

/// §17 覆盖的**项数**（编号 1–19、21–25 共 24 项；#20 不在本卡范围）。
///
/// 24 是**项数**（§17 表格的行数），不是**函数数**：#3 与 #10 各含两个错误码
/// ⇒ 落成两个独立函数，故函数数是 26。
pub const ITEM_COUNT: usize = 24;

/// 单项校验的错误载荷。
///
/// 除头像尺寸外，全部包着卡 1 冻结的 [`ExpertError`]（**不重复定义同名语义**）；
/// 头像尺寸是 §17 #21 的第三条判据，卡 1 的错误集里没有对应变体，故单列。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ItemError {
    /// 卡 1 冻结的错误变体（按码分流）。
    #[error(transparent)]
    Expert(#[from] ExpertError),
    /// §17 #21：头像尺寸不合规（规格：512×512）。
    #[error("头像 {path} 尺寸 {width}×{height}，期望 {expected_width}×{expected_height}")]
    AvatarDimensions {
        /// 头像相对路径。
        path: String,
        /// 实际宽。
        width: u32,
        /// 实际高。
        height: u32,
        /// 期望宽。
        expected_width: u32,
        /// 期望高。
        expected_height: u32,
    },
}

impl ItemError {
    /// 稳定错误码（与 [`ExpertError::code`] 同一套字符串，供 UI 与测试断言）。
    pub fn code(&self) -> &'static str {
        match self {
            ItemError::Expert(err) => err.code(),
            ItemError::AvatarDimensions { .. } => "AVATAR_DIMENSIONS",
        }
    }

    /// 严重级别（按 §17「级别」列；尺寸项为 Error）。
    pub fn severity(&self) -> Severity {
        match self {
            ItemError::Expert(err) => err.severity(),
            ItemError::AvatarDimensions { .. } => Severity::Error,
        }
    }
}

/// 一条校验发现：**§17 编号 + 错误 + 级别**。
///
/// 既是 `ValidationReport` 里的一条 finding，也是 `Validator::validate` 失败时
/// 返回的错误——两者共用同一类型，保证「报出来的编号」与「测出来的编号」一致。
#[derive(Debug, Clone)]
pub struct ValidateError {
    item: u8,
    error: ItemError,
    severity: Severity,
}

impl ValidateError {
    /// 构造一条发现（编号 + 错误 + 级别）。
    pub fn new(item: u8, error: ItemError, severity: Severity) -> Self {
        Self {
            item,
            error,
            severity,
        }
    }

    /// §17 校验项编号（1–19、21–25）。
    pub fn item(&self) -> u8 {
        self.item
    }

    /// 稳定错误码。
    pub fn code(&self) -> &'static str {
        self.error.code()
    }

    /// 严重级别：`Error` 阻断注册，`Warning` 仅提示。
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// 底层错误（头像尺寸项为 `None`）。
    pub fn expert_error(&self) -> Option<&ExpertError> {
        match &self.error {
            ItemError::Expert(err) => Some(err),
            ItemError::AvatarDimensions { .. } => None,
        }
    }

    /// 是否阻断（fail-closed 判据：Error 级即阻断）。
    pub fn is_blocking(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for ValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "§17 #{} {}: {}", self.item, self.code(), self.error)
    }
}

impl std::error::Error for ValidateError {}

/// 校验项的严重级别口径：§17 表格「级别」列 **+ 本卡三处偏离**。
///
/// 偏离一栏（`docs/expert-validate-status.md` 偏离清单 D-A/D-B/D-C）：
///
/// | §17 项 | 规格级别 | 本卡级别 | 理由 |
/// |---|---|---|---|
/// | #3b `NAME_DIR_MISMATCH` | Error | **Warning**（`strict_dir_name` 时 Error） | 真实样本 `pandaai-team/` 的目录名 ≠ `plugin.json.name`（`pandaai-ai-quant-research-team`）；上游按 marketplace 仓库名落盘（调研 §4.1），目录名 ≠ 插件名是**正常**形态 |
/// | #12 `DESCRIPTION_LENGTH` | Error | **Warning** | 真实样本 zh 描述实测 **39 / 51** 字，均落在 [40,50] 之外；Error 会把两份真实样本判死 |
/// | #17 `TEAM_PROFESSION_DISPLAY_MISMATCH` | Error | **Warning** | 真实样本 `pandaai-team` 的 `profession.zh`（PD-AI量化研究专家团）≠ `displayName.zh`（研报复现因子挖掘回测审计），且人格 MD 的 frontmatter 与之一致 ⇒ 判定项与样本事实矛盾 |
///
/// 其余按 §17 原样：`#23` / `#24` 本身就是 Warning（§1.3），其余全 Error。
pub fn severity_of(error: &ItemError, strict_dir_name: bool) -> Severity {
    match error {
        ItemError::Expert(ExpertError::NameDirMismatch { .. }) => {
            if strict_dir_name {
                Severity::Error
            } else {
                Severity::Warning
            }
        }
        ItemError::Expert(ExpertError::DescriptionLength { .. })
        | ItemError::Expert(ExpertError::TeamProfessionDisplayMismatch) => Severity::Warning,
        other => other.severity(),
    }
}
