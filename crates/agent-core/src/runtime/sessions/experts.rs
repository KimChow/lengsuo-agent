//! 专家包目录定位（专家体系 · 卡 6）。
//!
//! 规格 `harness-expert.md` §4.1（双根目录与发现）+ §10.2 步骤 4（召唤时「从注册表取
//! 人格」）。本文件只回答一个问题：**某 id 的包目录在哪、这个 id 在不在**。
//!
//! 根目录：用户级 `<home>/.lengsuo/experts` + 项目级 `<ws>/.lengsuo/experts`，有序
//! `[用户级, 项目级]`——项目级在后 ⇒ **同名时项目级覆盖用户级**（§4.1 第 2 条，
//! 「后注册者优先」）。`home` 取 `$HOME`（Unix）/ `$USERPROFILE`（Windows），未设 ⇒
//! 只有项目级根；`ws` 取会话 `workspace_root`。
//!
//! **为什么逐个目录定位，而不是整盘 `discover`**：`ExpertRegistry::discover_scoped`
//! 的约定是「包标记目录内 manifest 缺失/损坏 ⇒ 显式错误（不静默跳过）」——整盘扫时
//! 一个**与该 id 无关**的坏包会让「专家不存在」「召唤成功」都变成同一个扫盘错误，
//! I3 的错误码就区分不开了。故本文件只扫一级子目录的**名字**定位候选，再按覆盖
//! 顺序逐候选载入（`registry::load_package`，与整盘扫描同一套判定）：**第一个可用
//! 的包**即生效包；候选全部载入失败时，报**优先级最高**候选的包错误（truth 与
//! §4.1 一致：高优先级包坏了不会被低优先级包静默顶替）。
//!
//! 本文件不读人格——那是 [`super::expert_bind`] 的职责。

use std::path::{Path, PathBuf};

use agent_expert::{
    paths, ExpertError, ExpertManifest, ExpertOrigin, ExpertPackage, ExpertRoot, ExpertScope,
};

/// 一次召唤/组装所用的专家根（`[用户级, 项目级]`，有序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertDirs {
    /// 当前会话的工作区（项目级根的基底）。
    pub workspace: PathBuf,
    /// 用户级根；home 不可得时为 `None`。
    pub user_root: Option<PathBuf>,
}

impl ExpertDirs {
    /// 按会话工作区与当前进程的 home 解析双根。
    pub fn for_session(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            user_root: home_dir().map(|home| paths::user_experts_root(&home)),
        }
    }

    /// 有序根列表：`[用户级, 项目级]`（后注册者优先）。
    pub fn roots(&self) -> Vec<ExpertRoot> {
        let mut roots = Vec::with_capacity(2);
        if let Some(user) = &self.user_root {
            roots.push(ExpertRoot::new(user.clone(), ExpertScope::User));
        }
        roots.push(ExpertRoot::new(
            paths::project_experts_root(&self.workspace),
            ExpertScope::Project,
        ));
        roots
    }

    /// 按 id 定位生效包目录（判定见模块文档）。
    ///
    /// `Ok(Some)` 即生效包；`Ok(None)` ⇒ 该 id 在所有根下都没有可用包；
    /// `Err` ⇒ 候选包存在但载入失败（manifest 缺失/损坏，错误码保留）。
    pub(crate) fn locate(&self, expert_id: &str) -> Result<Option<ExpertPackage>, ExpertError> {
        let roots = self.roots();
        // 候选按优先级从高到低排队：先看包标记目录，再看无标记的裸目录（后者只靠
        // manifest 的 `name` 认领）；各段内 [项目级, 用户级]。
        let candidates = roots.iter().rev().flat_map(|root| {
            let marked = std::iter::once(true).chain(std::iter::once(false));
            marked.flat_map(move |want_marked| {
                candidate_dirs(&root.path, expert_id)
                    .into_iter()
                    .filter(move |dir| has_package_marker(dir) == want_marked)
                    .map(move |dir| (root, dir))
            })
        });
        let mut first_error: Option<ExpertError> = None;
        for (root, dir) in candidates {
            match load_package(&root.path, &dir, root.scope) {
                Ok(Some(package)) if package.id == expert_id => return Ok(Some(package)),
                Ok(_) => {}
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }
}

/// 某根下与 `expert_id` 同名的目录（`<root>/<id>`）；不存在或越出根 ⇒ 空。
fn candidate_dirs(root: &Path, expert_id: &str) -> Vec<PathBuf> {
    let dir = root.join(expert_id);
    let within = paths::ensure_within(root, &dir).is_ok();
    if dir.is_dir() && within {
        vec![dir]
    } else {
        Vec::new()
    }
}

/// 包标记判定（与 `agent-expert` 的 `load_package` 同一口径：「这是个专家包目录」）。
fn has_package_marker(dir: &Path) -> bool {
    dir.join(paths::PLUGIN_DIR).exists()
        || dir.join(paths::AGENTS_DIR).is_dir()
        || dir.join(paths::MANIFEST_FILE).exists()
}

/// 载入一个包目录（与 `agent-expert` 整盘发现同一套判定与错误码；`None` = 不是包）。
///
/// 层内约定：`discover_scoped` 对「包标记目录内 manifest 缺失/损坏」显式报错、不静默
/// 跳过。本处逐目录复用同口径，只是**限定在与该 id 相关的目录**上——这样无关的坏包
/// 不会污染召唤结论，而 id 自身的坏包仍按原错误码上报（I3）。
fn load_package(
    root: &Path,
    dir: &Path,
    scope: ExpertScope,
) -> Result<Option<ExpertPackage>, ExpertError> {
    let escape = || ExpertError::PathEscape {
        path: dir.display().to_string(),
    };
    paths::ensure_within(root, dir).map_err(|_| escape())?;
    let plugin_dir = dir.join(paths::PLUGIN_DIR);
    match std::fs::symlink_metadata(&plugin_dir) {
        Ok(meta) if !meta.is_dir() => {
            return Err(ExpertError::PluginManifestMissing {
                path: plugin_dir.join(paths::MANIFEST_FILE),
            })
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ExpertError::io(&plugin_dir, &e)),
    }
    let manifest_path = plugin_dir.join(paths::MANIFEST_FILE);
    let text = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        // 专家目录存在但 manifest 缺失 ⇒ 显式错误（否则专家凭空消失）。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && has_package_marker(dir) => {
            return Err(ExpertError::PluginManifestMissing {
                path: manifest_path,
            })
        }
        Err(_) if !has_package_marker(dir) => return Ok(None),
        Err(e) => return Err(ExpertError::io(&manifest_path, &e)),
    };
    let manifest = ExpertManifest::from_json_str(&text)?;
    Ok(Some(ExpertPackage {
        id: manifest.name.clone(),
        manifest,
        dir: dir.to_path_buf(),
        root: root.to_path_buf(),
        scope,
        origin: ExpertOrigin::Local,
        downloaded_at: None,
        overrides: Vec::new(),
    }))
}

/// 进程 home：`$HOME`（Unix）/ `$USERPROFILE`（Windows）。
fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
}
