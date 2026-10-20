//! 专家包初始化（§12 工作流第 2 步：`init` 建目录骨架，**拒绝覆盖**）。
//!
//! 规格：`harness-expert.md` §12（第 2/3 步）与 §19.2（最小可用闭环第 1 步）。
//!
//! 行为口径：
//!
//! 1. **拒绝覆盖**：目标目录已存在（含空目录、悬空 symlink）⇒ 显式错误，
//!    **绝不写入任何内容**。`ExpertError` 没有「已存在」变体，故用
//!    [`ExpertError::Io`] 承载 `ErrorKind::AlreadyExists`（不改冻结的错误契约）。
//! 2. **目录骨架**：`.lengsuo-plugin/`、`agents/`、`skills/`、`avatars/` 四者
//!    路径全部取自 [`crate::paths`]，不写死字符串。
//! 3. **`plugin.json` 由程序序列化**（§12 第 3 步的理由：不让模型手写 JSON，
//!    防字段漂移）；入参是结构化 [`ExpertDraft`]，转换失败即 fail-closed。
//!
//! 分层：本文件只做「建骨架 + 序列化草稿」；校验属 `validate/`，注册属
//! `registry::ops`，导出属 `export`。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::ExpertError;
use crate::manifest::{CategoryId, ExpertManifest, ExpertType, LocalizedText, TeamInfo};
use crate::paths;

/// 建包草稿（**简写入参**）：只含建模必需字段。
///
/// 其余 manifest 必填项由程序派生（见 [`ExpertDraft::to_manifest`]），正文与
/// 展示素材由工作流第 3 步「生成内容」补全——本类型不另立一套字段，转换目标
/// 始终是卡 1 的 [`ExpertManifest`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertDraft {
    /// 专家 id（= 目录名 = `plugin.json.name`，kebab-case 由校验器判定）。
    pub name: String,
    /// 展示名（花名，双语）。
    pub display_name: LocalizedText,
    /// 语义化版本。
    pub version: String,
    /// 专家类型（单专家 / 专家团）。
    pub expert_type: ExpertType,
    /// 12 个分类之一。
    pub category_id: CategoryId,
    /// 英文一句话描述（映射到 `description`，并派生 `displayDescription`）。
    pub description: String,
}

impl ExpertDraft {
    /// 转换到 [`ExpertManifest`]；必填项缺失/为空 ⇒ fail-closed。
    ///
    /// 派生规则（草稿没有的字段）：
    /// - `agentName` = `name`，`agents = ["./agents/<name>.md"]`（工作流第 3 步
    ///   按此路径写人格正文）；
    /// - `profession` = `displayName`（团型的展示名即职业头衔，§17 #17）；
    /// - `displayDescription` = `description`（en/zh 同文，正文补全后可改）；
    /// - `tags` / `quickPrompts` 留空：由第 3 步生成内容后补齐（校验器会拦住
    ///   未补齐就注册的包，这正是「先校验后写」的保护点）。
    pub fn to_manifest(&self) -> Result<ExpertManifest, ExpertError> {
        require("name", &self.name)?;
        require("version", &self.version)?;
        require("description", &self.description)?;
        require("displayName.zh", &self.display_name.zh)?;
        require("displayName.en", &self.display_name.en)?;

        let name = self.name.trim().to_string();
        let description = self.description.trim().to_string();
        let team_info = match self.expert_type {
            ExpertType::Team => Some(TeamInfo {
                lead_agent: name.clone(),
                member_agents: Vec::new(),
                workflow: None,
            }),
            ExpertType::Agent => None,
        };
        Ok(ExpertManifest {
            name: name.clone(),
            version: self.version.trim().to_string(),
            description: description.clone(),
            author: None,
            homepage: None,
            license: None,
            keywords: Vec::new(),
            expert_type: self.expert_type,
            agent_name: name.clone(),
            team_info,
            agents: vec![format!("./{}/{name}.md", paths::AGENTS_DIR)],
            skills: Vec::new(),
            display_name: self.display_name.clone(),
            profession: self.display_name.clone(),
            display_description: LocalizedText {
                zh: description.clone(),
                en: description,
            },
            avatar: None,
            category_id: self.category_id,
            default_init_prompt: LocalizedText {
                zh: String::new(),
                en: String::new(),
            },
            tags: Vec::new(),
            quick_prompts: Vec::new(),
            plugin: None,
            members: Vec::new(),
            dependencies: None,
            persona: None,
            prompt_pair: None,
        })
    }
}

/// 必填项非空检查（fail-closed；空串/纯空白都算缺失）。
fn require(field: &str, value: &str) -> Result<(), ExpertError> {
    if value.trim().is_empty() {
        Err(ExpertError::MissingField {
            field: field.to_string(),
        })
    } else {
        Ok(())
    }
}

/// 初始化结果摘要（成功建包后返回）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertSummary {
    /// 专家 id（= `plugin.json.name`）。
    pub id: String,
    /// 包目录。
    pub dir: PathBuf,
    /// 版本。
    pub version: String,
    /// 专家类型。
    pub expert_type: ExpertType,
    /// 分类。
    pub category_id: CategoryId,
}

/// 建目录骨架并写入 `plugin.json`（§12 第 2 步）。
///
/// - 目录已存在 ⇒ `ExpertError::Io`（`AlreadyExists` 语义），**原内容一字不动**；
/// - 草稿必填项为空 ⇒ `ExpertError::MissingField`，**不触盘**。
pub fn init_expert(dir: &Path, draft: &ExpertDraft) -> Result<ExpertSummary, ExpertError> {
    // 先转换（fail-closed），再做任何文件系统操作。
    let manifest = draft.to_manifest()?;
    create_new_dir(dir)?;
    for sub in [
        paths::PLUGIN_DIR,
        paths::AGENTS_DIR,
        paths::SKILLS_DIR,
        paths::AVATARS_DIR,
    ] {
        let path = dir.join(sub);
        fs::create_dir_all(&path).map_err(|e| ExpertError::io(&path, &e))?;
    }
    let manifest_path = dir.join(paths::manifest_rel_path());
    let text = manifest.to_json_string()?;
    fs::write(&manifest_path, format!("{text}\n").as_bytes())
        .map_err(|e| ExpertError::io(&manifest_path, &e))?;
    Ok(ExpertSummary {
        id: manifest.name,
        dir: dir.to_path_buf(),
        version: manifest.version,
        expert_type: manifest.expert_type,
        category_id: manifest.category_id,
    })
}

/// 创建**新**目录；已存在 ⇒ 显式拒绝（含悬空 symlink，`create_dir` 不跟随）。
fn create_new_dir(dir: &Path) -> Result<(), ExpertError> {
    match dir.symlink_metadata() {
        Ok(_) => return Err(already_exists(dir)),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(ExpertError::io(dir, &e)),
    }
    if let Some(parent) = dir.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| ExpertError::io(parent, &e))?;
        }
    }
    match fs::create_dir(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(already_exists(dir)),
        Err(e) => Err(ExpertError::io(dir, &e)),
    }
}

/// 「目录已存在」错误：`Io` 承载 `AlreadyExists` 语义（错误契约不新增变体）。
fn already_exists(dir: &Path) -> ExpertError {
    ExpertError::Io {
        path: dir.to_path_buf(),
        reason: format!("目录已存在，拒绝覆盖 ({})", ErrorKind::AlreadyExists),
    }
}
