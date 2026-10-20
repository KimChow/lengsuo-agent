//! 结构校验（§17 #1–#9）：目录、manifest、标识、人格文件。
//!
//! 逐条对应调研 §17 表格：
//!
//! | # | 检查什么 | 失败码 | 本文件函数 |
//! |---|---|---|---|
//! | 1 | 目录含 `.lengsuo-plugin/plugin.json` | `PLUGIN_MANIFEST_MISSING` | [`manifest_present`] |
//! | 2 | `plugin.json` 可解析且 schema 合法 | `MANIFEST_PARSE_FAILED` | [`manifest_parses`] |
//! | 3 | `name` kebab-case 且 == 目录名 | `INVALID_NAME` / `NAME_DIR_MISMATCH` | [`name_is_kebab_case`] / [`name_matches_dir_name`] |
//! | 4 | `version` 语义化 MAJOR.MINOR.PATCH | `INVALID_VERSION` | [`version_is_semver`] |
//! | 5 | `expertType` 与实际结构一致 | `TYPE_STRUCTURE_MISMATCH` | [`type_matches_structure`] |
//! | 6 | `agentName` == 某 `agents/*.md` 的 stem | `AGENT_NAME_MISMATCH` | [`agent_name_matches_stem`] |
//! | 7 | `agents[]` 存在、非空、落在包目录内 | `AGENT_PATH_MISSING` / `PATH_ESCAPE` | [`agent_paths_valid`] |
//! | 8 | Agent MD frontmatter 无 `tools` | `FORBIDDEN_TOOL_DECLARATION` | [`agent_frontmatter_has_no_tools`] |
//! | 9 | Agent MD frontmatter `name` == 文件 stem | `AGENT_FRONTMATTER_NAME_MISMATCH` | [`agent_frontmatter_name_matches_stem`] |

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::error::ExpertError;
use crate::manifest::{ExpertManifest, ExpertType};
use crate::paths::{MANIFEST_FILE, PLUGIN_DIR};
use crate::validate::ValidateContext;
use crate::validate_errors::ItemError;

/// §17 #1：目录存在且含 `.lengsuo-plugin/plugin.json`。
pub fn manifest_present(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest_path = ctx.dir.join(PLUGIN_DIR).join(MANIFEST_FILE);
    if manifest_path.is_file() {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::PluginManifestMissing {
            path: manifest_path,
        }))
    }
}

/// §17 #2：`plugin.json` 可解析为 JSON 且 schema 合法。
///
/// 解析在**进入校验器之前**完成（`ExpertManifest::from_json_str`），故本项
/// 复核「磁盘上的 manifest 文本确实能被解析」——避免调用方传入与磁盘不一致的
/// manifest 对象时，坏包被当成好包放过。
pub fn manifest_parses(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest_path = ctx.dir.join(PLUGIN_DIR).join(MANIFEST_FILE);
    let text = fs::read_to_string(&manifest_path)
        .map_err(|source| ItemError::Expert(ExpertError::io(&manifest_path, &source)))?;
    ExpertManifest::from_json_str(&text)
        .map(|_| ())
        .map_err(ItemError::Expert)
}

/// §17 #3a：`name` 是 kebab-case（§6 铁律 1）。
pub fn name_is_kebab_case(ctx: &ValidateContext) -> Result<(), ItemError> {
    let name = &ctx.manifest.name;
    if is_kebab_case(name) {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::InvalidName {
            value: name.clone(),
        }))
    }
}

/// §17 #3b：`name` 与目录名一致。
///
/// 真实样本 `pandaai-team/` 的目录名 ≠ `plugin.json.name`，故默认降为
/// Warning（见 `RuleSet::strict_dir_name` 与偏离清单 D-A）。
pub fn name_matches_dir_name(ctx: &ValidateContext) -> Result<(), ItemError> {
    let name = &ctx.manifest.name;
    let dir = dir_name(ctx.dir);
    if name == &dir {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::NameDirMismatch {
            name: name.clone(),
            dir,
        }))
    }
}

/// §17 #4：`version` 是语义化 MAJOR.MINOR.PATCH。
pub fn version_is_semver(ctx: &ValidateContext) -> Result<(), ItemError> {
    let version = &ctx.manifest.version;
    if is_semver(version) {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::InvalidVersion {
            value: version.clone(),
        }))
    }
}

/// §17 #5：`expertType` 与实际结构一致。
///
/// 规格原文：`agent ⇒ 1 agent，无 teamInfo`；`team ⇒ 有 teamInfo 且
/// members ≥ 2`。样本实测两份都成立（单专家 1 agent 无 teamInfo；团队型
/// 6 agents / 6 members / 有 teamInfo）。
pub fn type_matches_structure(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest = ctx.manifest;
    let reason = match manifest.expert_type {
        ExpertType::Agent => {
            if manifest.agents.len() != 1 {
                Some(format!(
                    "expertType=agent 但 agents 有 {} 项（应恰 1 项）",
                    manifest.agents.len()
                ))
            } else if manifest.team_info.is_some() {
                Some("expertType=agent 但声明了 teamInfo".to_string())
            } else if !manifest.members.is_empty() {
                Some(format!(
                    "expertType=agent 但 members 有 {} 项（应为空）",
                    manifest.members.len()
                ))
            } else {
                None
            }
        }
        ExpertType::Team => {
            if manifest.team_info.is_none() {
                Some("expertType=team 但缺 teamInfo".to_string())
            } else if manifest.members.len() < 2 {
                Some(format!(
                    "expertType=team 但 members 有 {} 项（应 ≥ 2 项，含主理人）",
                    manifest.members.len()
                ))
            } else {
                None
            }
        }
    };
    match reason {
        Some(reason) => Err(ItemError::Expert(ExpertError::TypeStructureMismatch {
            reason,
        })),
        None => Ok(()),
    }
}

/// §17 #6：`agentName` == 某 `agents/*.md` 的 stem，且该路径在 `agents[]` 中。
pub fn agent_name_matches_stem(ctx: &ValidateContext) -> Result<(), ItemError> {
    let manifest = ctx.manifest;
    let actual = manifest
        .agents
        .iter()
        .map(|path| file_stem(path))
        .collect::<Vec<_>>();
    if actual.iter().any(|stem| stem == &manifest.agent_name) {
        Ok(())
    } else {
        Err(ItemError::Expert(ExpertError::AgentNameMismatch {
            expected: manifest.agent_name.clone(),
            actual: actual.join(", "),
        }))
    }
}

/// §17 #7：`agents[]` 每项存在、非空、落在包目录内。
///
/// 「落在包目录内」复用 §6 铁律 3 的越界口径（`agent-memory::ensure_within`
/// 的等价实现，见 `resolve_within`）：拒绝 `..`、绝对路径与越出包根的符号链接。
pub fn agent_paths_valid(ctx: &ValidateContext) -> Result<(), ItemError> {
    for declared in &ctx.manifest.agents {
        let resolved = resolve_within(ctx.dir, declared)?;
        if !resolved.is_file() {
            return Err(ItemError::Expert(ExpertError::AgentPathMissing {
                path: declared.clone(),
            }));
        }
        let size = fs::metadata(&resolved)
            .map_err(|source| ItemError::Expert(ExpertError::io(&resolved, &source)))?
            .len();
        if size == 0 {
            return Err(ItemError::Expert(ExpertError::AgentPathMissing {
                path: declared.clone(),
            }));
        }
    }
    Ok(())
}

/// §17 #8：Agent MD 的 frontmatter **无 `tools`**（§6 铁律 6）。
///
/// 工具面由 `Mode::Expert` 白名单决定，人格文件不得自己声明工具。
///
/// **不重复报 #7**：文件缺失/越界由 §17 #7 独占报错，本项只判**读得到**的
/// 人格文件（否则一个坏文件会连报 #7/#8/#9 三条，噪声掩盖真因）。
pub fn agent_frontmatter_has_no_tools(ctx: &ValidateContext) -> Result<(), ItemError> {
    for declared in &ctx.manifest.agents {
        let resolved = match resolve_within(ctx.dir, declared) {
            Ok(resolved) if resolved.is_file() => resolved,
            _ => continue,
        };
        let front = read_frontmatter(&resolved)?;
        if has_key(&front, "tools") {
            return Err(ItemError::Expert(ExpertError::ForbiddenToolDeclaration {
                path: declared.clone(),
            }));
        }
    }
    Ok(())
}

/// §17 #9：Agent MD 的 frontmatter `name` == 文件 stem。
///
/// **不重复报 #7**（同 [`agent_frontmatter_has_no_tools`]）。
pub fn agent_frontmatter_name_matches_stem(ctx: &ValidateContext) -> Result<(), ItemError> {
    for declared in &ctx.manifest.agents {
        let resolved = match resolve_within(ctx.dir, declared) {
            Ok(resolved) if resolved.is_file() => resolved,
            _ => continue,
        };
        let front = read_frontmatter(&resolved)?;
        let actual = front_value(&front, "name")
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        let expected = file_stem(declared);
        if actual == expected {
            continue;
        }
        return Err(ItemError::Expert(
            ExpertError::AgentFrontmatterNameMismatch { expected, actual },
        ));
    }
    Ok(())
}

/// kebab-case 判定（§6 铁律 1 的正则 `^[a-z0-9]+(-[a-z0-9]+)*$` 的等价实现）。
///
/// **手写而非正则**：本 crate 无 `regex` 依赖，且 `Cargo.toml` 属共享区
/// （本卡不可改）⇒ 按字符类逐段判定，语义与该正则**逐字等价**
/// （小写字母/数字分段，段间单 `-`，无空段、无首尾 `-`）。
pub(crate) fn is_kebab_case(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// 语义化版本 MAJOR.MINOR.PATCH 判定（§17 #4）。
pub(crate) fn is_semver(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// 路径末段的文件名（去目录、去 `.md` 后缀）。
pub(crate) fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

/// 包目录的末段名（§17 #3b 的「目录名」）。
pub(crate) fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 把包内相对路径解析为绝对路径，并确认**落在包目录内**。
///
/// 越界防护口径（§6 铁律 3）：先按词法判 `..` / 绝对路径，再按 `canonicalize`
/// 后的真实路径确认仍在包根之内（后者能挡住符号链接逃逸）。
pub(crate) fn resolve_within(dir: &Path, declared: &str) -> Result<PathBuf, ItemError> {
    let relative = declared.trim_start_matches("./");
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(ItemError::Expert(ExpertError::PathEscape {
            path: declared.to_string(),
        }));
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ItemError::Expert(ExpertError::PathEscape {
            path: declared.to_string(),
        }));
    }
    let joined = dir.join(candidate);
    let root = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let real = fs::canonicalize(&joined).unwrap_or(joined.clone());
    if !real.starts_with(&root) {
        return Err(ItemError::Expert(ExpertError::PathEscape {
            path: declared.to_string(),
        }));
    }
    Ok(joined)
}

/// 读取 MD 文件的 YAML frontmatter（`---` 包裹的那一段）。
///
/// 只做**键值扫描**（不引入 YAML 依赖）：逐行找 `key:` 或 `key: value`。
/// 嵌套结构（样本的 `displayName:` / `skills:` 列表）只取顶层键名。
pub(crate) fn read_frontmatter(path: &Path) -> Result<Vec<(String, String)>, ItemError> {
    let text = fs::read_to_string(path)
        .map_err(|source| ItemError::Expert(ExpertError::io(path, &source)))?;
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(Vec::new());
    }
    let mut front = Vec::new();
    for line in lines {
        let trimmed = line.trim_end();
        if trimmed.trim() == "---" {
            break;
        }
        if trimmed.starts_with(char::is_whitespace) || trimmed.trim().is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        front.push((key.trim().to_string(), value.trim().to_string()));
    }
    Ok(front)
}

/// frontmatter 里是否有某个键（顶层）。
pub(crate) fn has_key(front: &[(String, String)], key: &str) -> bool {
    front.iter().any(|(name, _)| name == key)
}

/// frontmatter 某键的值（顶层）。
pub(crate) fn front_value<'a>(front: &'a [(String, String)], key: &str) -> Option<&'a str> {
    front
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}
