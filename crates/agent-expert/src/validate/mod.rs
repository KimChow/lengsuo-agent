//! 专家包校验器（专家体系 · 卡 2）。
//!
//! 规格：`harness-expert.md` §17「校验面」25 项（编号 1–25）
//! + §6「铁律逐条落地」12 条 + §7「不可变字段」。
//!
//! 边界语义：专家导入文件损坏 ⇒ 校验失败并给出**可读错误**（不抛 panic）。
//!
//! **fail-closed**：任一 Error 级发现 ⇒ 整体不通过（对齐 `agent-skills`
//! `SkillRegistry::load` 的 fail-closed 口径）。Warning 级不阻断，只在报告里
//! 列出（§1.3：`DUPLICATE_NAME_PROFESSION` / `CONNECTOR_DECL_INVALID` 是
//! Warning，不阻断注册）。
//!
//! **分层**：24 项按「结构 / 字段 / 团队 / 资源」拆成四个子模块，每项校验
//! **一个独立函数**（便于逐条测试与逐条报错）：
//!
//! | 子模块 | §17 项 | 覆盖面 |
//! |---|---|---|
//! | [`structure`] | #1–#9 | 目录、manifest、name/version/type、人格文件 |
//! | [`fields`] | #10–#14、#22–#24 | 展示字段、计数、分类、不可变字段、成员命名、连接器 |
//! | [`team`] | #15–#18 | 团队型专属（lead、成员集合、profession、通用名） |
//! | [`assets`] | #19、#21 | 技能目录、头像（存在 / 体积 / 尺寸） |
//!
//! **#20 `SCRIPT_SYNTAX` 不在本卡范围**：它是「`skills/**/scripts/*.py` 可
//! `ast.parse`」——需要 Python 语法解析器（Rust 侧无对应物，且本仓无 Python
//! 运行时依赖）。记入 `docs/expert-validate-status.md` 未做项。
//!
//! **#25 `ALREADY_REGISTERED` 的判定方**（§17 表注：「register 阶段」）：
//! 查的是**注册表**（卡 3），本卡不查注册表 ⇒ 由调用方把已注册名单经
//! [`RuleSet::with_registered_names`] 传进来，没传名单就不判（无对照物）。
//!
//! **24 项 vs 26 个函数**：§17 的 **24 是项数**（表格行数，编号 1–19、21–25）。
//! 其中 #3（`INVALID_NAME` / `NAME_DIR_MISMATCH`）与 #10（`TAG_COUNT` /
//! `QUICK_PROMPT_COUNT`）各含**两个错误码** ⇒ 各落成**两个独立函数**（§17
//! 「每项校验一个独立函数」+ 「错误须指明是哪一条」），故函数数是 **26**。

mod assets;
mod fields;
mod structure;
mod team;

use std::path::Path;

use crate::error::Severity;
use crate::manifest::ExpertManifest;
use crate::validate_errors::{severity_of, ItemError, ValidateError};

pub use crate::validate_errors::item;

/// 校验上下文：包目录 + manifest + 规则开关。
///
/// 「包目录」是**必填**：#1（manifest 存在）、#3b（目录名）、#7（路径落在包
/// 目录内）、#19（技能目录）、#21（头像文件）都要触盘，脱离目录无法判定。
pub struct ValidateContext<'a> {
    /// 专家包根目录（`name` 与目录名一致时的那个目录）。
    pub dir: &'a Path,
    /// 已解析的 manifest。
    pub manifest: &'a ExpertManifest,
    /// 规则开关（见 [`RuleSet`]）。
    pub rules: &'a RuleSet,
}

/// 校验规则开关（哪些非规格项启用）。
///
/// 规格外的自由度只有两处：**目录名口径**（§17 #3b 的 `name` == 目录名）与
/// **头像尺寸**（§17 #21 的第三条判据）。两处默认都按规格原文的宽松口径，
/// 本仓自建包可收紧。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSet {
    /// 目录名必须等于 `plugin.json.name`（否则 `NAME_DIR_MISMATCH` 升为 Error）。
    ///
    /// 真实样本 `pandaai-team/` 的目录名 ≠ `plugin.json.name`
    /// （`pandaai-ai-quant-research-team`）——上游按 marketplace 仓库名落盘
    /// （调研 §4.1），目录名 ≠ 插件名是**正常**形态，故默认不阻断。
    pub strict_dir_name: bool,
    /// 头像必须 512×512（否则报 `AVATAR_DIMENSIONS`）。
    ///
    /// 规格 §3 目录结构写「头像（512×512，≤500KB）」，但 §17 #21 只列了
    /// 「存在且 ≤500KB」两条判据。**默认关**（照 §17 逐条实现，不加规格外
    /// 阻断），本仓自建包可开启。
    pub strict_avatar_dimensions: bool,
    /// §17 #22 的**比对基线**（已注册/已落盘的旧 manifest）。
    ///
    /// §7「实现点」：「`update_expert` 在写回前比对新旧 `name`/`agentName`，
    /// 不等即拒」。本仓 v1 不提供 update，故**没有基线就不判 #22**
    /// （无对照物）；给了基线才比对三项不可变字段。
    pub baseline: Option<ExpertManifest>,
    /// §17 #25「同名已注册」的**已注册名单**（外部注入）。
    ///
    /// §17 表注 #25 是「register 阶段」的判定：查的是**注册表**（卡 3）。
    /// 本卡不查注册表 ⇒ 由调用方（卡 3/卡 8 的注册流程）把已注册的名字
    /// 传进来；**没传名单就不判 #25**（无对照物）。
    pub registered_names: Vec<String>,
}

impl RuleSet {
    /// 默认规则集：规格逐条实现，目录名宽松，头像尺寸不校验，无 #22 基线。
    pub fn new() -> Self {
        Self {
            strict_dir_name: false,
            strict_avatar_dimensions: false,
            baseline: None,
            registered_names: Vec::new(),
        }
    }

    /// 收紧目录名口径（`NAME_DIR_MISMATCH` 升为 Error）。
    pub fn strict_dir_name(mut self, strict: bool) -> Self {
        self.strict_dir_name = strict;
        self
    }

    /// 收紧头像尺寸口径（512×512）。
    pub fn strict_avatar_dimensions(mut self, strict: bool) -> Self {
        self.strict_avatar_dimensions = strict;
        self
    }

    /// 设置 §17 #22 的比对基线（旧 manifest）。
    pub fn with_baseline(mut self, baseline: ExpertManifest) -> Self {
        self.baseline = Some(baseline);
        self
    }

    /// 设置 §17 #25 的已注册名单（注册表里的专家名）。
    pub fn with_registered_names<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.registered_names = names.into_iter().map(Into::into).collect();
        self
    }
}

/// 一项校验：`§17 编号` + 检查函数。
type Check = (u8, fn(&ValidateContext) -> Result<(), ItemError>);

/// 全部校验项（**唯一定义处**）：23 项（§17 的 1–19、21–24）。
///
/// #20（脚本语法）与 #25（同名已注册）不在本卡范围，见模块文档。
const CHECKS: &[Check] = &[
    // —— 结构（#1–#9）——
    (item::MANIFEST_PRESENT, structure::manifest_present),
    (item::MANIFEST_PARSES, structure::manifest_parses),
    (item::NAME_KEBAB_CASE, structure::name_is_kebab_case),
    (item::NAME_MATCHES_DIR, structure::name_matches_dir_name),
    (item::VERSION_SEMVER, structure::version_is_semver),
    (
        item::TYPE_MATCHES_STRUCTURE,
        structure::type_matches_structure,
    ),
    (
        item::AGENT_NAME_MATCHES_STEM,
        structure::agent_name_matches_stem,
    ),
    (item::AGENT_PATHS, structure::agent_paths_valid),
    (
        item::NO_TOOLS_DECLARATION,
        structure::agent_frontmatter_has_no_tools,
    ),
    (
        item::FRONTMATTER_NAME_MATCHES_STEM,
        structure::agent_frontmatter_name_matches_stem,
    ),
    // —— 字段（#10–#14、#22–#24）——
    (item::TAG_COUNT, fields::tag_count_is_three),
    (
        item::QUICK_PROMPT_COUNT,
        fields::quick_prompt_count_is_three,
    ),
    (
        item::INIT_PROMPT_MATCHES,
        fields::init_prompt_matches_first_quick_prompt,
    ),
    (
        item::DESCRIPTION_LENGTH,
        fields::description_zh_length_in_range,
    ),
    (item::PLUGIN_NAME_MATCHES, fields::plugin_matches_name),
    (item::CATEGORY_KNOWN, fields::category_is_known),
    (
        item::IMMUTABLE_FIELDS_INTACT,
        fields::immutable_fields_intact,
    ),
    (
        item::MEMBER_NAME_NOT_PROFESSION,
        fields::member_name_differs_from_profession,
    ),
    (
        item::CONNECTOR_DECL_RESOLVES,
        fields::connector_declaration_resolves,
    ),
    // —— 团队（#15–#18）——
    (item::TEAM_LEAD_UNIQUE, team::team_lead_is_unique),
    (
        item::TEAM_MEMBERS_MATCH,
        team::team_member_agents_match_members,
    ),
    (
        item::TEAM_PROFESSION_MATCHES_DISPLAY,
        team::team_profession_matches_display_name,
    ),
    (
        item::AGENT_NAME_NOT_GENERIC,
        team::team_agent_name_not_generic,
    ),
    // —— 资源（#19、#21）——
    (item::SKILL_PATHS, assets::skill_paths_valid),
    (item::AVATAR_VALID, assets::avatar_valid),
    // —— 注册面（#25）——
    (
        item::NAME_NOT_REGISTERED,
        fields::name_not_already_registered,
    ),
];

/// 校验器：规则集 + 报告聚合。
///
/// 用法：
///
/// ```no_run
/// use agent_expert::{ExpertManifest, Validator};
/// use std::path::Path;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = Path::new("senior-developer");
/// let manifest = ExpertManifest::from_json_str(&std::fs::read_to_string(
///     dir.join(".lengsuo-plugin/plugin.json"))?)?;
/// let report = Validator::new().validate_dir(dir, &manifest)?;
/// println!("{} 项检查，{} 条发现", report.checked, report.findings.len());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Validator {
    rules: RuleSet,
}

impl Validator {
    /// 默认规则集。
    pub fn new() -> Self {
        Self {
            rules: RuleSet::new(),
        }
    }

    /// 指定规则集。
    pub fn with_rules(rules: RuleSet) -> Self {
        Self { rules }
    }

    /// 当前规则集。
    pub fn rules(&self) -> &RuleSet {
        &self.rules
    }

    /// 校验专家包（**目录 + manifest**）。
    ///
    /// 这是**规范入口**：#1/#3b/#7/#19/#21 都要触盘，只给 manifest 判不了。
    /// 返回 `Ok(report)` 当且仅当**没有 Error 级发现**；Error 级发现时返回
    /// `Err`（带上**第一条**阻断发现，fail-closed 不静默跳过）。
    pub fn validate_dir(
        &self,
        dir: &Path,
        manifest: &ExpertManifest,
    ) -> Result<ValidationReport, ValidateError> {
        let report = self.report_dir(dir, manifest);
        match report.blocking().first() {
            Some(finding) => Err((*finding).clone()),
            None => Ok(report),
        }
    }

    /// 校验并返回完整报告（**不因 Error 级失败**）。
    ///
    /// 与 [`Validator::validate_dir`] 的区别：后者 fail-closed（Error ⇒
    /// `Err`）；本方法把全部发现收集齐，供 UI 一次性展示。
    pub fn report_dir(&self, dir: &Path, manifest: &ExpertManifest) -> ValidationReport {
        let ctx = ValidateContext {
            dir,
            manifest,
            rules: &self.rules,
        };
        let mut report = ValidationReport {
            checked: 0,
            findings: Vec::new(),
        };
        for (number, check) in CHECKS {
            report.checked += 1;
            if let Err(error) = check(&ctx) {
                let severity = severity_of(&error, self.rules.strict_dir_name);
                report
                    .findings
                    .push(ValidateError::new(*number, error, severity));
            }
        }
        report
    }
}

/// 校验报告：跑了几项、有哪些发现。
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    /// 已执行的校验项数。
    pub checked: usize,
    /// 全部发现（Error + Warning，按 §17 编号升序）。
    pub findings: Vec<ValidateError>,
}

impl ValidationReport {
    /// Error 级发现（阻断项）。
    pub fn blocking(&self) -> Vec<&ValidateError> {
        self.findings
            .iter()
            .filter(|finding| finding.is_blocking())
            .collect()
    }

    /// Warning 级发现（不阻断）。
    pub fn warnings(&self) -> Vec<&ValidateError> {
        self.findings
            .iter()
            .filter(|finding| !finding.is_blocking())
            .collect()
    }

    /// 是否通过（无 Error 级发现）。
    pub fn is_ok(&self) -> bool {
        self.blocking().is_empty()
    }

    /// 最严重的一条发现（Error 优先；用于 fail-closed 的对外表述）。
    pub fn first_blocking(&self) -> Option<&ValidateError> {
        self.findings
            .iter()
            .find(|finding| finding.severity() == Severity::Error)
    }
}
