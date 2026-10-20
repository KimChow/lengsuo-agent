//! 专家包技能挂载（规格 `harness-expert.md` §16.1 / §16.2）。
//!
//! 只做「解析声明 + 定位包内技能目录 + 生成对外名」这一层；扫描与三道门
//! （授权目录边界 / 禁用 / 调用来源）全在 `agent-skills`，本模块**只调用**它
//! （`SkillCatalog::new(dirs, disabled)` + `list()`），不改它一行。
//!
//! 三条铁律（§16.2）：
//! 1. **私有**：包内 `skills/<x>/` 只在该专家激活时挂进 `SkillCatalog` 的根列表；
//!    全局技能根不经本模块，仍全局可见；
//! 2. **前缀装载时加**：对外名 `<expert-name>__<skill-dir-name>`（双下划线），
//!    **不强制**源目录名带前缀——样本实测目录名无统一前缀；
//! 3. **不做**间接映射层：本仓技能只有一个来源，无「同一技能两个名字」的诉求。
//!
//! 成员级绑定（`team.json` / 成员 MD frontmatter 的 `skills:`）v1 **只解析不生效**：
//! 团队运行未就绪（§14），本模块不读、不挂成员技能。
//!
//! 错误口径与 §17 校验面一致：
//! - 声明路径越出专家包目录（含 `..` 与绝对路径）⇒ [`ExpertError::PathEscape`]
//!   （越界防护复用 [`crate::paths::ensure_within`]，与卡 3 同口径）；
//! - 目录不存在或缺少 `SKILL.md` ⇒ [`ExpertError::SkillPathMissing`]，
//!   **不静默跳过**（fail-closed，与 `SkillCatalog::list()` 一致）。

use std::path::{Component, Path, PathBuf};

use agent_skills::{AccessError, SkillCatalog, SKILL_FILE};

use crate::error::ExpertError;
use crate::paths::ensure_within;

/// 专家名与技能名之间的分隔：**双下划线**（kebab-case 的 `-` 不参与分隔）。
pub const NAMESPACE_SEP: &str = "__";

/// 一条挂载结果：对外名 → 包内技能目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedSkill {
    /// 所属专家（`plugin.json.name`）；用于「激活专家 A 时 B 的技能不可见」。
    pub expert_name: String,
    /// 包内技能目录名（原样，**不加**前缀）。
    pub skill_name: String,
    /// 对外技能名：`<expert-name>__<skill-dir-name>`。
    pub mounted_name: String,
    /// 包内技能目录绝对路径（canonical）。
    pub skill_dir: PathBuf,
    /// `plugin.json.skills[]` 的原始值（如 `./skills/<x>`），便于诊断回指。
    pub source: String,
}

/// 目录里可见、且属于声明集的技能条目（对外名 + 源名 + 路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleSkill {
    /// 对外名：`<expert-name>__<skill-dir-name>`。
    pub mounted_name: String,
    /// 源名（`SKILL.md` 的 `name`）：`agent-skills` 目录的查找键。
    pub source_name: String,
    /// canonical 后的 `SKILL.md` 路径。
    pub path: PathBuf,
}

/// 对外名：`<expert-name>__<skill-dir-name>`。
///
/// 前缀只在这里加——源目录名保持原样（§16.2「装载后生成对外名」）。
pub fn mounted_name(expert_name: &str, skill_name: &str) -> String {
    format!("{expert_name}{NAMESPACE_SEP}{skill_name}")
}

/// 把某专家包声明的技能挂进技能目录，返回「对外技能名 → 包内技能目录」的映射。
///
/// 顺序与 `declared` 一致；任一声明不合法即整体失败（不返回部分结果）。
/// 调用方把 [`catalog_roots`] 的结果交给 `SkillCatalog::new` 即完成挂载。
pub fn mount_expert_skills(
    expert_name: &str,
    expert_dir: &Path,
    declared: &[String],
) -> Result<Vec<MountedSkill>, ExpertError> {
    let mut mounted = Vec::with_capacity(declared.len());
    for source in declared {
        let skill_dir = resolve_skill_dir(expert_dir, source)?;
        let skill_name = skill_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            // 声明指向专家包根本身（`.`）：目录名取包子目录名，仍保有唯一名。
            .unwrap_or_else(|| {
                expert_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        mounted.push(MountedSkill {
            expert_name: expert_name.to_string(),
            mounted_name: mounted_name(expert_name, &skill_name),
            skill_name,
            skill_dir,
            source: source.clone(),
        });
    }
    Ok(mounted)
}

/// 拼装交给 `SkillCatalog::new(dirs, disabled)` 的根目录列表。
///
/// 次序：全局根在前、专家技能根在后（后注册者优先，与 `SkillCatalog::new`
/// 的 `dirs` 语义一致）。
///
/// 专家侧的根是**技能容器目录**（技能目录的父目录，包内即 `skills/`），
/// 不是单个技能目录：`agent-skills` 的授权模型要求「根 = 技能目录的父目录」
/// （`SkillCatalog::scan` 按根的一级子目录找 `SKILL.md`，且要求技能目录的父
/// 目录正好等于某个根），把单个技能目录当根会扫不到任何技能。
///
/// 私有性靠根列表保证：未激活专家的容器不放进 `roots`，其技能即不可见。
pub fn catalog_roots(global_roots: &[PathBuf], mounted: &[MountedSkill]) -> Vec<PathBuf> {
    let mut roots = global_roots.to_vec();
    for container in skill_containers(mounted) {
        if !roots.contains(&container) {
            roots.push(container);
        }
    }
    roots
}

/// 技能容器目录（技能目录的父目录）去重列表，保持首次出现顺序。
fn skill_containers(mounted: &[MountedSkill]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for m in mounted {
        let Some(parent) = m.skill_dir.parent() else {
            continue;
        };
        if !out.iter().any(|p| p == parent) {
            out.push(parent.to_path_buf());
        }
    }
    out
}

/// 对外名 → 挂载条目（宿主按对外名请求技能时用）。
pub fn resolve_mounted<'a>(
    mounted: &'a [MountedSkill],
    mounted_name: &str,
) -> Option<&'a MountedSkill> {
    mounted.iter().find(|m| m.mounted_name == mounted_name)
}

/// 可见集：目录里可见、**且**属于本专家声明集的技能。
///
/// `agent-skills` 的目录按 `SKILL.md` 的 `name` 归并，宿主加载键是源名；
/// 对外名（带前缀）是本层的别名。过滤只保留路径落在声明技能目录内的条目，
/// 使「未声明的同级目录」不进入本专家的可见集。
pub fn visible_skills(
    catalog: &SkillCatalog,
    mounted: &[MountedSkill],
) -> Result<Vec<VisibleSkill>, AccessError> {
    let summaries = catalog.list()?;
    let mut out = Vec::new();
    for m in mounted {
        for summary in &summaries {
            let in_dir = summary.path.parent() == Some(m.skill_dir.as_path());
            if in_dir {
                out.push(VisibleSkill {
                    mounted_name: m.mounted_name.clone(),
                    source_name: summary.name.clone(),
                    path: summary.path.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| a.mounted_name.cmp(&b.mounted_name));
    Ok(out)
}

/// §17 #7/#19 同口径：先越界防护（`..` / 绝对路径 / symlink 逃逸），再要求
/// 目录存在且含 `SKILL.md`。
fn resolve_skill_dir(expert_dir: &Path, source: &str) -> Result<PathBuf, ExpertError> {
    let declared = Path::new(source);
    // 绝对路径或任何 `..` 段：词法上直接判逃逸（与 `ensure_within` 的
    // `PathEscape::Lexical` 同类），在触盘前拦下。
    if declared.is_absolute()
        || declared.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(ExpertError::PathEscape {
            path: source.to_string(),
        });
    }
    // 专家包目录按调用方给出解析为绝对路径（同 `Path::join` 语义）。
    let resolved = expert_dir.join(declared);
    // 符号链接 / 真实路径复核：跑到授权根之外即拒。
    ensure_within(expert_dir, &resolved).map_err(|_| ExpertError::PathEscape {
        path: source.to_string(),
    })?;

    if !resolved.is_dir() || !resolved.join(SKILL_FILE).is_file() {
        return Err(ExpertError::SkillPathMissing {
            path: source.to_string(),
        });
    }
    // canonical 路径：与 `SkillCatalog` 落库的 path 同一口径，便于比对。
    let canonical = resolved
        .canonicalize()
        .map_err(|source| ExpertError::io(&resolved, &source))?;
    Ok(canonical)
}
