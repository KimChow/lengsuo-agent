//! 专家目录发现与注册表读写（专家体系 · 卡 3）。
//!
//! 规格：调研 `harness-expert.md` §4.1（目录与发现）+ §4.2（`registry.json` 格式）
//!
//! 另对齐 §18 卡 3；参考实现 `11-专家与云智能体.md`（本地对照资料，不入库） §2.1
//! （专家是可管理的资产：创建/校验/注册）与 §6（专家导入文件损坏 ⇒ 校验失败给
//! 可读错误）。
//!
//! 口径摘要（逐条对应 §4.1「发现机制」四项）：
//!
//! 1. **双根扫描**：用户级 `~/.lengsuo/experts` + 项目级 `<ws>/.lengsuo/experts`；
//!    根目录不存在 ⇒ **跳过**（不是错误）。
//! 2. **同名覆盖**：项目级覆盖用户级（对齐 `agent-skills` 的「有序目录 + 后注册者
//!    优先」语义，§4.1 第 2 条）；被覆盖者**记进 `overrides`**，供 UI 显示来源。
//! 3. **注册表是索引不是真相**（§4.1 第 3 条）：目录扫描出的包为真相源；
//!    `registry.json` 缺失 ⇒ 扫盘重建（不报错）；与磁盘漂移 ⇒ 以目录为准。
//! 4. **专家目录存在而 manifest 缺失/损坏 ⇒ 显式错误**（否则专家凭空消失）。
//!
//! 分层：**wire** 数据模型（`registry.json` 逐字段）在 [`model`]；运行时结构
//! （[`ExpertPackage`]/[`ExpertRoot`]/[`ShadowedExpert`]）与发现/读写逻辑在本文件。
//!
//! 越界防护在本 crate 自建（[`crate::paths::ensure_within`]），口径对齐
//! `agent-memory::MemoryPaths::ensure_within`（**不改 `agent-memory`**）。
//!
//! 边界：本卡只做「发现 + 注册表读写」；校验属卡 2，注册表对外操作（init/register/
//! unregister/export）属卡 8。

mod discover;
mod model;
mod ops;
mod time;

use time::now_rfc3339_millis;

pub use model::{
    ExpertOrigin, ExpertScope, OverrideRecord, RegistryEntry, RegistryFile, REGISTRY_SCHEMA_VERSION,
};
pub use ops::{register_expert, unregister_expert};

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::ExpertError;
use crate::manifest::{CategoryId, ExpertManifest, ExpertType};
use crate::paths;
use crate::registry::discover::{expert_dirs, load_package};

/// 一个专家根（路径 + 来源层级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertRoot {
    /// 根目录（`<base>/.lengsuo/experts`）。
    pub path: PathBuf,
    /// 来源层级。
    pub scope: ExpertScope,
}

impl ExpertRoot {
    /// 构造一个专家根。
    pub fn new(path: impl Into<PathBuf>, scope: ExpertScope) -> Self {
        Self {
            path: path.into(),
            scope,
        }
    }
}

/// 被覆盖的同名专家（低优先级来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedExpert {
    /// 被覆盖者的来源层级。
    pub scope: ExpertScope,
    /// 被覆盖者的包目录。
    pub dir: PathBuf,
}

/// 扫描出的专家包（一个目录 = 一个包）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpertPackage {
    /// 唯一键（= `plugin.json.name`；§4.2 `id` = 目录名 = `plugin.json.name`）。
    pub id: String,
    /// 已解析的 manifest。
    pub manifest: ExpertManifest,
    /// 包目录。
    pub dir: PathBuf,
    /// 所属专家根。
    pub root: PathBuf,
    /// 来源层级（UI 显示「来自项目」用）。
    pub scope: ExpertScope,
    /// 来源类型。
    pub origin: ExpertOrigin,
    /// 下载时间标记（规范外字段 `.downloaded_at`，§1.3；缺失为 `None`）。
    pub downloaded_at: Option<String>,
    /// 被本包覆盖的同名低优先级包（§4.1 第 2 条：记录被覆盖者）。
    pub overrides: Vec<ShadowedExpert>,
}

impl ExpertPackage {
    /// 覆盖来源类型（如导入流程标记 `imported`）。
    pub fn with_origin(mut self, origin: ExpertOrigin) -> Self {
        self.origin = origin;
        self
    }
}

/// 专家注册表（已注册专家表）。
///
/// 注册表是**索引不是真相**（§4.1 第 3 条）：真相源是目录扫描；本结构只存索引与
/// 元数据，**不存人格正文**（正文永远从包目录读，防两处真相）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpertRegistry {
    entries: Vec<RegistryEntry>,
    /// 运行时目录索引（`id -> 包目录`）：**不序列化**，供导出定位（卡 8）。
    dirs: BTreeMap<String, PathBuf>,
}

impl ExpertRegistry {
    /// 扫描专家根，返回**生效**的专家包（同名只保留优先级最高的一份）。
    ///
    /// `roots` 有序：**后注册者优先**（与 `agent-skills` 的 `SkillRegistry` 一致），
    /// 即 `[用户级, 项目级]` ⇒ 项目级覆盖用户级。输出按 id 升序（同输入同输出）。
    ///
    /// 根目录不存在 ⇒ 该根跳过（不报错）；包目录存在而 manifest 缺失/损坏 ⇒ 显式
    /// 错误。
    pub fn discover(roots: &[PathBuf]) -> Result<Vec<ExpertPackage>, ExpertError> {
        let scoped: Vec<ExpertRoot> = roots
            .iter()
            .map(|path| ExpertRoot::new(path.clone(), infer_scope(path)))
            .collect();
        Self::discover_scoped(&scoped)
    }

    /// 同 [`ExpertRegistry::discover`]，但来源层级由调用方显式给出。
    pub fn discover_scoped(roots: &[ExpertRoot]) -> Result<Vec<ExpertPackage>, ExpertError> {
        let mut by_id: BTreeMap<String, ExpertPackage> = BTreeMap::new();
        for root in roots {
            for dir in expert_dirs(&root.path)? {
                let Some(package) = load_package(&root.path, &dir, root.scope)? else {
                    continue;
                };
                match by_id.remove(&package.id) {
                    // 同名冲突：后来者生效，被覆盖者入 `overrides`（§4.1 第 2 条）。
                    Some(previous) => {
                        let mut chain = vec![ShadowedExpert {
                            scope: previous.scope,
                            dir: previous.dir,
                        }];
                        chain.extend(previous.overrides);
                        let mut package = package;
                        package.overrides = chain;
                        by_id.insert(package.id.clone(), package);
                    }
                    None => {
                        by_id.insert(package.id.clone(), package);
                    }
                }
            }
        }
        Ok(by_id.into_values().collect())
    }

    /// 由扫描结果生成注册表条目（`registeredAt` 取当前 UTC）。
    pub fn from_packages(packages: &[ExpertPackage]) -> Self {
        let now = now_rfc3339_millis();
        let entries = packages
            .iter()
            .map(|item| RegistryEntry {
                id: item.id.clone(),
                version: item.manifest.version.clone(),
                expert_type: item.manifest.expert_type,
                category_id: item.manifest.category_id,
                path: relative_path(item),
                downloaded_at: item.downloaded_at.clone(),
                registered_at: Some(now.clone()),
                source: item.origin,
                scope: item.scope,
                // 被覆盖者可能来自另一个根 ⇒ 按其 `.lengsuo` 基底取相对路径。
                overrides: item
                    .overrides
                    .iter()
                    .map(|shadowed| OverrideRecord {
                        scope: shadowed.scope,
                        path: relative_to(&scope_base_path(&shadowed.dir), &shadowed.dir),
                    })
                    .collect(),
            })
            .collect();
        Self {
            entries,
            ..Self::default()
        }
    }

    /// 读取注册表；**文件缺失 ⇒ 扫盘重建**（不报错）。
    ///
    /// 重建范围 = 注册表文件所在目录（`registry.json` 位于用户级专家根，§4.1）；
    /// 需要跨双根重建请用 [`ExpertRegistry::load_registry_with_roots`]。
    pub fn load_registry(path: &Path) -> Result<Self, ExpertError> {
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::load_registry_with_roots(path, &[root])
    }

    /// 读取注册表；缺失 ⇒ 按给定根扫盘重建；与磁盘漂移 ⇒ **以目录为准**。
    ///
    /// 「读取」不写盘（无副作用）：重建结果由调用方 [`ExpertRegistry::save_registry`]
    /// 落盘。
    pub fn load_registry_with_roots(path: &Path, roots: &[PathBuf]) -> Result<Self, ExpertError> {
        let scanned = Self::discover(roots)?;
        let disk = Self::from_packages(&scanned);
        match fs::read_to_string(path) {
            Ok(text) => {
                let file: RegistryFile =
                    serde_json::from_str(&text).map_err(|e| ExpertError::ManifestParse {
                        reason: format!("registry.json 解析失败: {e}"),
                    })?;
                // schema 版本不同 ⇒ 索引格式已变 ⇒ 以目录为准重建。
                if file.schema_version != REGISTRY_SCHEMA_VERSION {
                    return Ok(disk);
                }
                let loaded = Self {
                    entries: file.experts,
                    ..Self::default()
                };
                // 注册表是索引：与磁盘漂移时以目录为准（§4.1 第 3 条）。
                Ok(if loaded.same_truth_as(&disk) {
                    loaded
                } else {
                    disk
                })
            }
            // 缺失 ⇒ 重建，不报错。
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(disk),
            Err(e) => Err(ExpertError::io(path, &e)),
        }
    }

    /// 写注册表（原子写：临时文件 + rename；父目录自动创建）。
    pub fn save_registry(&self, path: &Path) -> Result<(), ExpertError> {
        let file = RegistryFile {
            schema_version: REGISTRY_SCHEMA_VERSION,
            experts: self.entries.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| ExpertError::ManifestParse {
            reason: format!("registry.json 序列化失败: {e}"),
        })?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| ExpertError::io(parent, &e))?;
            }
        }
        let tmp = path.with_file_name(format!(
            ".{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::write(&tmp, text.as_bytes()).map_err(|e| ExpertError::io(&tmp, &e))?;
        fs::rename(&tmp, path).map_err(|e| ExpertError::io(path, &e))?;
        Ok(())
    }

    /// 已注册条目。
    pub fn entries(&self) -> &[RegistryEntry] {
        &self.entries
    }

    /// 按 id 查条目。
    pub fn find(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// 某 id 的来源层级（UI 显示「来自项目」用）。
    pub fn scope_of(&self, id: &str) -> Option<ExpertScope> {
        self.find(id).map(|entry| entry.scope)
    }

    /// 覆盖关系（返回 `(生效条目, 被覆盖记录)` 对）。
    pub fn overrides(&self) -> impl Iterator<Item = (&RegistryEntry, &OverrideRecord)> {
        self.entries
            .iter()
            .flat_map(|entry| entry.overrides.iter().map(move |record| (entry, record)))
    }

    /// 真相字段比对（忽略时间戳与覆盖明细）：注册表与磁盘是否一致。
    ///
    /// 按 id 配对（注册表条目的顺序不承载语义）。
    fn same_truth_as(&self, other: &Self) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        let disk: BTreeMap<&str, &RegistryEntry> = other
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        self.entries.iter().all(|entry| {
            disk.get(entry.id.as_str())
                .is_some_and(|disk| truth_key(entry) == truth_key(disk))
        })
    }
}

/// 真相字段：id / version / type / category / path / scope。
fn truth_key(
    entry: &RegistryEntry,
) -> (String, String, ExpertType, CategoryId, String, ExpertScope) {
    (
        entry.id.clone(),
        entry.version.clone(),
        entry.expert_type,
        entry.category_id,
        entry.path.clone(),
        entry.scope,
    )
}

/// 解析根内的相对路径；`..` 与 symlink 逃逸 ⇒ 拒（`PATH_ESCAPE`）。
///
/// 供后续卡（导出/会话绑定）按注册表里的相对路径还原包目录用。
pub fn resolve_within(root: &Path, relative: &Path) -> Result<PathBuf, ExpertError> {
    let joined = root.join(relative);
    ensure_within(root, &joined)?;
    Ok(joined)
}

/// 越界防护的 crate 内封装（错误统一为 `ExpertError::PathEscape`）。
fn ensure_within(base: &Path, candidate: &Path) -> Result<(), ExpertError> {
    paths::ensure_within(base, candidate).map_err(|_| ExpertError::PathEscape {
        path: candidate.display().to_string(),
    })
}

/// 来源层级推断：根在 home 之下 ⇒ 用户级；其余 ⇒ 项目级。
///
/// `discover` 只能拿到裸路径列表，层级靠位置无法判定（技能层级的 `SkillRegistry`
/// 同样只靠**顺序**定优先级）；需要精确层级时用 [`ExpertRegistry::discover_scoped`]。
fn infer_scope(root: &Path) -> ExpertScope {
    match std::env::home_dir() {
        Some(home) if root.starts_with(&home) => ExpertScope::User,
        _ => ExpertScope::Project,
    }
}

/// 包路径（相对其 `.lengsuo` 基底，形如 `experts/<id>`；§4.2 样例字面值）。
fn relative_path(package: &ExpertPackage) -> String {
    match package.root.parent() {
        Some(base) if base.file_name().is_some_and(|name| name == ".lengsuo") => {
            relative_to(base, &package.dir)
        }
        _ => relative_to(&package.root, &package.dir),
    }
}

/// 由包目录反推该层级的 `.lengsuo` 基目录（`<base>/.lengsuo/experts/<id>` 的上两级）；
/// 结构不符时退化为包目录的父目录。
fn scope_base_path(dir: &Path) -> PathBuf {
    dir.ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == ".lengsuo"))
        .unwrap_or_else(|| dir.parent().unwrap_or(dir))
        .to_path_buf()
}

/// 相对路径表示（`/` 分隔）。
fn relative_to(base: &Path, target: &Path) -> String {
    match target.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => target
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}
