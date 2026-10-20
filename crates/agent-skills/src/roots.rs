//! 三级技能根解析：内置 → 用户级 → 项目级（低优先级 → 高优先级）。
//!
//! 规格对照参考客户端 09 册 §2.1 与 18 册 §9.1：用户级
//! `<home>/<dataFolder>/skills/`（跨项目复用）、项目级
//! `<workspace>/<dataFolder>/skills/`（团队共享）、内置级随安装包分发。
//! 本仓数据目录为 `.lengsuo`；内置级没有约定位置，由宿主显式传入，
//! 缺省为 `None`。
//!
//! `SkillRegistry::with_dirs` 的既有语义是**后注册者优先**，因此
//! [`SkillRoots::as_dirs`] 必须按「内置、用户、项目」返回，项目级最后。

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::SkillsError;

/// 用户级技能根的环境变量覆盖（空串视为未设置）。
pub const USER_DIR_ENV: &str = "LENGSUO_SKILLS_USER_DIR";
/// 项目级技能根的环境变量覆盖（空串视为未设置）。
pub const PROJECT_DIR_ENV: &str = "LENGSUO_SKILLS_PROJECT_DIR";

/// 本仓的技能数据目录名（对齐九册的 `dataFolderName`）。
const DATA_FOLDER: &str = ".lengsuo";
/// 数据目录下的技能子目录名。
const SKILLS_DIR: &str = "skills";

/// 三级技能根，字段顺序即优先级顺序（低 → 高）。
///
/// 用户级与项目级在 [`SkillRoots::resolve_default`] 中总有值（默认路径）；
/// 目录是否真实存在、是否可读，由 [`SkillRoots::as_dirs`] 在注册前判定。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillRoots {
    builtin: Option<PathBuf>,
    user: Option<PathBuf>,
    project: Option<PathBuf>,
}

impl SkillRoots {
    /// 解析默认根：内置（`None`）→ 用户级 → 项目级（后者覆盖前者）。
    ///
    /// 用户级默认 `<home>/.lengsuo/skills/`，项目级默认
    /// `<workspace>/.lengsuo/skills/`；对应环境变量设置且非空时覆盖默认值。
    /// 内置级由宿主经 [`SkillRoots::with_builtin`] 传入。
    pub fn resolve_default(home: &Path, workspace: &Path) -> Self {
        let user = env_override(USER_DIR_ENV).unwrap_or_else(|| default_skills_dir(home));
        let project =
            env_override(PROJECT_DIR_ENV).unwrap_or_else(|| default_skills_dir(workspace));
        Self {
            builtin: None,
            user: Some(user),
            project: Some(project),
        }
    }

    /// 指定随安装包分发的内置技能根（低优先级）。
    pub fn with_builtin(mut self, builtin: impl Into<PathBuf>) -> Self {
        self.builtin = Some(builtin.into());
        self
    }

    /// 内置技能根（未传入时为 `None`）。
    pub fn builtin(&self) -> Option<&Path> {
        self.builtin.as_deref()
    }

    /// 用户级技能根。
    pub fn user(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// 项目级技能根。
    pub fn project(&self) -> Option<&Path> {
        self.project.as_deref()
    }

    /// 转成 `SkillRegistry::with_dirs` 需要的顺序化 Vec（高优先级在后）。
    ///
    /// 逐级探测目录：
    /// - 不存在（`NotFound`）⇒ 跳过该级，不报错（首次运行尚无目录）；
    /// - 存在但不可读（权限、路径不是目录等）⇒ 返回 [`SkillsError::Io`]，
    ///   不静默跳过。
    ///
    /// 探测与后续扫描之间存在 TOCTOU 窗口，本函数只负责注册前的一次判定。
    pub fn as_dirs(&self) -> Result<Vec<PathBuf>, SkillsError> {
        let mut dirs = Vec::with_capacity(3);
        // 数组顺序即注册顺序：内置 → 用户 → 项目，后者覆盖前者。
        for root in [
            self.builtin.as_deref(),
            self.user.as_deref(),
            self.project.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            match fs::read_dir(root) {
                Ok(_) => dirs.push(root.to_path_buf()),
                Err(source) if source.kind() == ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(SkillsError::Io {
                        path: root.to_path_buf(),
                        source,
                    })
                }
            }
        }
        Ok(dirs)
    }
}

/// 读取覆盖变量；空串按「未设置」处理，避免空路径把扫描根退化成 `/`。
fn env_override(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `<base>/.lengsuo/skills/`。
fn default_skills_dir(base: &Path) -> PathBuf {
    base.join(DATA_FOLDER).join(SKILLS_DIR)
}
