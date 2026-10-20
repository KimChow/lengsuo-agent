//! 字段校验（§17 #10–#14、#22–#24）：展示字段、计数、分类、不可变字段、
//! 成员命名、连接器声明。
//!
//! | # | 检查什么 | 失败码 | 级别 | 本文件函数 |
//! |---|---|---|---|---|
//! | 10 | `tags` / `quickPrompts` 各恰 3 项 | `TAG_COUNT` / `QUICK_PROMPT_COUNT` | Error | [`tag_count_is_three`] / [`quick_prompt_count_is_three`] |
//! | 11 | `defaultInitPrompt` == `quickPrompts[0]`（en/zh 两侧） | `INIT_PROMPT_MISMATCH` | Error | [`init_prompt_matches_first_quick_prompt`] |
//! | 12 | `displayDescription.zh` 字符数 ∈ [40,50] | `DESCRIPTION_LENGTH` | ⚠️ **Warning**（偏离 D-B） | [`description_zh_length_in_range`] |
//! | 13 | `plugin` == `name`（若声明） | `PLUGIN_NAME_MISMATCH` | Error | [`plugin_matches_name`] |
//! | 14 | `categoryId` ∈ 12 枚举 | `UNKNOWN_CATEGORY` | Error | [`category_is_known`] |
//! | 22 | 不可变字段未被 update 改动 | `IMMUTABLE_FIELD` | Error | [`immutable_fields_intact`] |
//! | 23 | `members[].name` 与 `profession` 同语言侧不相等 | `DUPLICATE_NAME_PROFESSION` | **Warning**（§1.3） | [`member_name_differs_from_profession`] |
//! | 24 | `dependencies.connectors` 解析成功 | `CONNECTOR_DECL_INVALID` | **Warning**（§1.3） | [`connector_declaration_resolves`] |
//!
//! **#14 的特殊处**：`categoryId` 的枚举判定在**结构层**（卡 1 的
//! `CategoryId::from_str`）已经做掉了——未知值在 `ExpertManifest::from_json_str`
//! 阶段就报 `UNKNOWN_CATEGORY`，根本走不到校验器。本项复核一遍，保证
//! 「不依赖结构层」的调用方（直接构造 `ExpertManifest` 的）也不漏判。

use std::collections::BTreeSet;
use std::str::FromStr;

use crate::error::ExpertError;
use crate::manifest::CategoryId;
use crate::validate::ValidateContext;
use crate::validate_errors::ItemError;

/// §6 铁律 4：`tags` 固定 3 个。
pub fn tag_count_is_three(ctx: &ValidateContext) -> Result<(), ItemError> {
    let count = ctx.manifest.tags.len();
    if count == 3 {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::TagCount { count }))
    }
}

/// §6 铁律 4：`quickPrompts` 固定 3 个。
pub fn quick_prompt_count_is_three(ctx: &ValidateContext) -> Result<(), ItemError> {
    let count = ctx.manifest.quick_prompts.len();
    if count == 3 {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::QuickPromptCount { count }))
    }
}

/// §6 铁律 4 / §17 #11：`defaultInitPrompt` 与 `quickPrompts[0]` **en/zh 两侧
/// 都相等**。
pub fn init_prompt_matches_first_quick_prompt(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest = ctx.manifest;
    let Some(first) = manifest.quick_prompts.first() else {
        // 项数不对由 #10 报；本项不重复报。
        return Ok(());
    };
    if manifest.default_init_prompt == *first {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::InitPromptMismatch))
    }
}

/// §6 铁律 5 / §17 #12：`displayDescription.zh` 字符数落在 [40,50]。
///
/// 计数口径：`chars().count()`（规格 §6 铁律 5 明写「按 `chars().count()`」）。
///
/// ⚠️ **偏离 D-B**：真实样本实测 39 字（`senior-developer`）与 51 字
/// （`pandaai-team`），**都落在 [40,50] 之外**。规格级别是 Error，但那会把
/// 两份真实样本判死 ⇒ 本项降为 **Warning**（详见偏离清单）。
pub fn description_zh_length_in_range(ctx: &ValidateContext) -> Result<(), ItemError> {
    let count = ctx.manifest.display_description.zh.chars().count();
    if (40..=50).contains(&count) {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::DescriptionLength { count }))
    }
}

/// §17 #13：`plugin`（若声明）等于 `name`。
///
/// `plugin` 在结构层是 `Option`（实测单专家样本无此键）——未声明即**不判**
/// （不报缺失），声明了才比对。
pub fn plugin_matches_name(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest = ctx.manifest;
    let Some(plugin) = manifest.plugin.as_ref() else {
        return Ok(());
    };
    if plugin == &manifest.name {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::PluginNameMismatch {
            expected: manifest.name.clone(),
            actual: plugin.clone(),
        }))
    }
}

/// §17 #14：`categoryId` ∈ 12 枚举。
///
/// 判定走 `CategoryId::from_str`（与卡 1 结构层同一套枚举，不另立名单）。
pub fn category_is_known(ctx: &ValidateContext) -> Result<(), ItemError> {
    let raw = ctx.manifest.category_id.as_str();
    if CategoryId::from_str(raw).is_ok() {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::UnknownCategory {
            value: raw.to_string(),
        }))
    }
}

/// §7「不可变字段」/ §17 #22：不可变字段未被改动。
///
/// §7 列出四项不可变：`plugin.json.name`、`plugin.json.agentName`、专家目录名、
/// `agents/<agentName>.md` 文件名。**判定口径**（与 §7「实现点」一致）：
/// 「`update_expert` 在写回前比对新旧 `name`/`agentName`，不等即拒」——
///
/// - 本仓 v1 **不提供 update**（§7：「本仓 v1 可不提供 update」），故本项
///   **需要一个基线 manifest** 才能判；没有基线时**不判**（无对照物）。
/// - 给了基线时，比对三项：① `name` ② `agentName` ③ `agents/<agentName>.md`
///   的文件 stem（即「`agents/<agentName>.md` 文件名」不可变）。
///
/// 「谁在何时可改」：**本卡不提供任何改写入口**——不可变字段**谁都不能改**；
/// 改名诉求走「重新创建」（新 id、新目录、删旧包），见 §7「改名诉求」行。
pub fn immutable_fields_intact(ctx: &ValidateContext) -> Result<(), ItemError> {
    let Some(baseline) = ctx.rules.baseline.as_ref() else {
        return Ok(());
    };
    let manifest = ctx.manifest;
    for field in IMMUTABLE_FIELDS {
        let (old, new) = match *field {
            "name" => (&baseline.name, &manifest.name),
            "agentName" => (&baseline.agent_name, &manifest.agent_name),
            _ => continue,
        };
        if old != new {
            return Err(ItemError::Expert(ExpertError::ImmutableField {
                field: (*field).to_string(),
            }));
        }
    }
    // ③ `agents/<agentName>.md` 文件名：主 Agent 人格路径的 stem 必须不变。
    let old_stem = crate::validate::structure::file_stem(baseline.persona().unwrap_or_default());
    let new_stem = crate::validate::structure::file_stem(manifest.persona().unwrap_or_default());
    if !old_stem.is_empty() && old_stem != new_stem {
        return Err(ItemError::Expert(ExpertError::ImmutableField {
            field: "agents/<agentName>.md".to_string(),
        }));
    }
    Ok(())
}

/// §7 列出、且本仓**永不可改**的字段名。
pub(crate) const IMMUTABLE_FIELDS: &[&str] = &["name", "agentName"];

/// §17 #25：同名未已注册。
///
/// §17 表注：#25 是「**register 阶段**」的判定——查的是注册表（卡 3）。
/// 本卡**不查注册表**：由调用方（卡 3/卡 8 的注册流程）把已注册的名字经
/// [`RuleSet::with_registered_names`] 传进来；**没传名单就不判**（无对照物）。
///
/// §6 铁律 11：「同名已存在仍须走『校验 + 注册』，不得跳过」——本项报出
/// `ALREADY_REGISTERED` 后，由调用方选择升级（version 比对）或改名后重建
/// （§7：「改名诉求 → 重新创建」）。
pub fn name_not_already_registered(ctx: &ValidateContext) -> Result<(), ItemError> {
    let name = &ctx.manifest.name;
    if ctx.rules.registered_names.iter().any(|known| known == name) {
        Err(ItemError::Expert(ExpertError::AlreadyRegistered {
            name: name.clone(),
        }))
    } else {
        Ok(())
    }
}

/// §8.3 / §17 #23：`members[].name` 与 `profession` **同语言侧不相等**。
///
/// §8.3「本仓落地」明写：校验器只做**可机械判定的部分**——`name` 的 en/zh 与
/// `profession` 同语言侧不相等；「巧思 / 禁叠字」等主观项**不写校验**。
///
/// **级别 = Warning**（§1.3 + §17 表格）：真实样本 `pandaai-team` 的 5 个团员
/// 存在 `name` 与 `profession` 中文相同（上游实测违规），Error 会把真实样本
/// 判死 ⇒ Warning，不阻断注册。
///
/// **主理人豁免**：§1.3 明写「主理人（`role:"lead"`）不受此条约束（其名是
/// 团名、profession 是团头衔，本就允许接近）」。
pub fn member_name_differs_from_profession(ctx: &ValidateContext) -> Result<(), ItemError> {
    for member in &ctx.manifest.members {
        if member.role == crate::manifest::MemberRole::Lead {
            continue;
        }
        let duplicated =
            member.name.zh == member.profession.zh || member.name.en == member.profession.en;
        if duplicated {
            return Err(ItemError::Expert(ExpertError::DuplicateNameProfession {
                member_id: member.id.clone(),
            }));
        }
    }
    Ok(())
}

/// §17 #24：`dependencies.connectors`（若声明）解析成功。
///
/// §1.3：「本仓连接器面（`agent-mcp`）尚不具备『声明式依赖 → 装载时校验』
/// 能力，故**只解析不强制**」。本项因此只做**机械可判定**的部分：
/// ① 每个连接器名非空；② 连接器名不含路径分隔符与控制字符（不得逃出命名
/// 空间）。解析不到具体连接器实现 ⇒ 不判「连接器不存在」。
///
/// **级别 = Warning**（§1.3 + §17 表格）。
pub fn connector_declaration_resolves(ctx: &ValidateContext) -> Result<(), ItemError> {
    let Some(dependencies) = ctx.manifest.dependencies.as_ref() else {
        return Ok(());
    };
    for connector in &dependencies.connectors {
        if connector.trim().is_empty() {
            return Err(ItemError::Expert(ExpertError::ConnectorDeclInvalid {
                reason: "连接器名为空".to_string(),
            }));
        }
        if connector
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return Err(ItemError::Expert(ExpertError::ConnectorDeclInvalid {
                reason: format!("连接器名含非法字符: {connector}"),
            }));
        }
    }
    Ok(())
}

/// 收集包内声明的连接器名（供其它卡查注册表用；本卡不查）。
#[allow(dead_code)]
pub(crate) fn declared_connectors<'a>(ctx: &'a ValidateContext<'_>) -> BTreeSet<&'a str> {
    ctx.manifest
        .dependencies
        .as_ref()
        .map(|dependencies| {
            dependencies
                .connectors
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default()
}
