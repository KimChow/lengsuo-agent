//! 注册表对外操作（§12 第 6 步 / §19.2 第 2–3 步）：`register` / `unregister`。
//!
//! 与 `registry.rs` 分文件：本文件只放**操作入口**，扫描/覆盖/读写逻辑仍留在
//! 卡 3 的 `registry.rs` / `discover.rs`（不重写）。
//!
//! **铁律 11/12：先校验后写**。`register_expert` 的顺序不可换：
//! `load_package` → `Validator::validate_dir`（fail-closed）→ 写前按目录重建
//! → 修改内存索引 → `save_registry`。校验不过时**一个字都不写**，内存注册表
//! 也保持不变（写成功后才整体替换）。
//!
//! **同名专家再次注册**：不静默跳过、不静默覆盖——仍走「校验 + 注册」路径，
//! 校验通过后更新索引项；同 id 但路径不同时，把旧条目记进
//! [`OverrideRecord`]（复用卡 3 的覆盖结构）。
//!
//! 注册表是**索引不是真相**（§4.1 第 3 条）：每次注册前先扫描 `dir` 所在专家
//! 根并重建该根（同 scope）的索引；磁盘上已消失的条目被丢弃，其它 scope 的
//! 条目保留。

use std::path::{Path, PathBuf};

use crate::error::ExpertError;
use crate::manifest::ExpertManifest;
use crate::paths;
use crate::registry::discover::load_package;
use crate::registry::model::OverrideRecord;
use crate::registry::time::now_rfc3339_millis;
use crate::registry::{
    infer_scope, relative_path, ExpertPackage, ExpertRegistry, ExpertScope, RegistryEntry,
};
use crate::validate::Validator;

/// 注册一个专家包（§12 第 6 步）。
///
/// 返回写入索引的 [`RegistryEntry`]；注册表文件写到 `dir` 所在专家根的
/// `registry.json`（§4.1）。校验失败 ⇒ `Err`，**不落盘、不改内存**。
pub fn register_expert(
    registry: &mut ExpertRegistry,
    dir: &Path,
) -> Result<RegistryEntry, ExpertError> {
    let root = dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let scope = infer_scope(&root);
    let package =
        load_package(&root, dir, scope)?.ok_or_else(|| ExpertError::PluginManifestMissing {
            path: dir.join(paths::manifest_rel_path()),
        })?;

    // ① 先校验：fail-closed（任一 Error 即停），此时尚未写任何东西。
    validate(dir, &package.manifest)?;

    // ② 写前重建：以目录为准，只重建本根所属 scope 的索引。
    let previous = registry.find(&package.id).cloned();
    let previous_dir = registry.dir_of(&package.id).map(Path::to_path_buf);
    let discovered = ExpertRegistry::discover(std::slice::from_ref(&root))?;
    let mut rebuilt = ExpertRegistry::from_packages(&discovered);
    for entry in registry.entries() {
        if entry.scope != scope {
            rebuilt.entries.push(entry.clone());
        }
    }

    // ③ 同 id 再次注册：更新索引项（不跳过）；路径变化时记一笔覆盖关系。
    let mut entry = entry_from(&package, scope);
    if let Some(previous) = previous {
        // 有运行时目录索引时按真实目录判断，否则退回注册表里的相对路径 + 层级。
        let same_location = match previous_dir {
            Some(old) => old == dir,
            None => previous.path == entry.path && previous.scope == entry.scope,
        };
        if same_location {
            entry.registered_at = previous.registered_at;
            entry.overrides = previous.overrides;
        } else {
            let mut chain = vec![OverrideRecord {
                scope: previous.scope,
                path: previous.path,
            }];
            chain.extend(previous.overrides);
            entry.overrides = chain;
        }
    }
    rebuilt.upsert(entry.clone());
    rebuilt.bind_dir(&entry.id, dir.to_path_buf());

    // ④ 全部成功后才落盘，再整体替换内存状态（失败则旧状态原样保留）。
    rebuilt.save_registry(&root.join(paths::REGISTRY_FILE))?;
    *registry = rebuilt;
    Ok(entry)
}

/// 注销一个专家（§19.2 第 3 步的逆操作）：**只删注册表索引项，不删包目录**。
///
/// 未注册的 id ⇒ 显式错误（不静默成功）。本函数只改内存索引；持久化由调用方
/// 随后调用 [`ExpertRegistry::save_registry`]（与卡 3 的 load/save 分工一致）。
pub fn unregister_expert(
    registry: &mut ExpertRegistry,
    id: &str,
) -> Result<RegistryEntry, ExpertError> {
    let index = registry
        .entries
        .iter()
        .position(|entry| entry.id == id)
        .ok_or_else(|| not_registered(id))?;
    let entry = registry.entries.remove(index);
    registry.dirs.remove(id);
    Ok(entry)
}

/// 校验失败时把第一条阻断发现还原成 `ExpertError`（错误契约不新增变体）。
fn validate(dir: &Path, manifest: &ExpertManifest) -> Result<(), ExpertError> {
    Validator::new()
        .validate_dir(dir, manifest)
        .map(|_| ())
        .map_err(|finding| {
            finding
                .expert_error()
                .cloned()
                .unwrap_or_else(|| ExpertError::ManifestParse {
                    reason: format!("校验失败 §17 #{}: {finding}", finding.item()),
                })
        })
}

/// 由扫描出的包构造索引条目（`registeredAt` 取当前 UTC；§4.2）。
fn entry_from(package: &ExpertPackage, scope: ExpertScope) -> RegistryEntry {
    RegistryEntry {
        id: package.id.clone(),
        version: package.manifest.version.clone(),
        expert_type: package.manifest.expert_type,
        category_id: package.manifest.category_id,
        path: relative_path(package),
        downloaded_at: package.downloaded_at.clone(),
        registered_at: Some(now_rfc3339_millis()),
        source: package.origin,
        scope,
        overrides: Vec::new(),
    }
}

/// 未注册 id 的显式错误（`ExpertError` 无 NotFound 变体，用 `Io` 承载语义）。
fn not_registered(id: &str) -> ExpertError {
    ExpertError::Io {
        path: PathBuf::from(id),
        reason: "专家未注册 (NotFound)".to_string(),
    }
}

impl ExpertRegistry {
    /// 插入或替换同 id 条目（索引按 id 升序，同输入同输出）。
    fn upsert(&mut self, entry: RegistryEntry) {
        match self.entries.iter_mut().find(|slot| slot.id == entry.id) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
        self.entries.sort_by(|left, right| left.id.cmp(&right.id));
    }

    /// 绑定 id → 包目录（运行时索引，不序列化；供导出定位）。
    pub(crate) fn bind_dir(&mut self, id: &str, dir: PathBuf) {
        self.dirs.insert(id.to_string(), dir);
    }

    /// 包目录（先查运行时索引；没有则 `None`，由调用方回退到标准根）。
    pub(crate) fn dir_of(&self, id: &str) -> Option<&Path> {
        self.dirs.get(id).map(PathBuf::as_path)
    }
}
