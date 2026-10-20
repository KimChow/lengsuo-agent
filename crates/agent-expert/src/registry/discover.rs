//! 目录扫描辅助：一级目录枚举、包载入（§4.1 发现机制第 1 条）。
//!
//! 与 `registry.rs` 分文件：本文件只做**触盘**（读目录、读 manifest、越界复核），
//! 注册表与覆盖合并逻辑留在 `registry.rs`。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::ExpertError;
use crate::manifest::ExpertManifest;
use crate::paths;
use crate::registry::model::{ExpertOrigin, ExpertScope};
use crate::registry::{ensure_within, ExpertPackage};

/// 列出根目录下的**一级专家目录**（按路径升序，结果确定）。
///
/// 忽略隐藏项（`.` 前缀）与非目录；根目录不存在 ⇒ 返回空列表（跳过该根）。
pub(crate) fn expert_dirs(root: &Path) -> Result<Vec<PathBuf>, ExpertError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ExpertError::io(root, &e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ExpertError::io(root, &e))?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = fs::metadata(&path)
            .map_err(|e| ExpertError::io(&path, &e))?
            .is_dir();
        if is_dir {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// 载入一个包目录；`None` = 该目录不是专家包（无包标记，忽略）。
///
/// 专家目录**存在**而 manifest 缺失/损坏 ⇒ 显式错误（`PluginManifestMissing` /
/// manifest 解析错误），不静默跳过——否则专家凭空消失。
///
/// **包标记**（调和 §4.1「忽略无 plugin.json 的目录」与本卡「manifest 缺失 ⇒
/// 显式错误」）：`.lengsuo-plugin/` 存在、`agents/` 目录存在、或顶层散落
/// `plugin.json` —— 三者任一命中即「这是个专家包目录」，manifest 必须可用；
/// 三者都不命中的普通目录忽略（§4.1 第 1 条）。
pub(crate) fn load_package(
    root: &Path,
    dir: &Path,
    scope: ExpertScope,
) -> Result<Option<ExpertPackage>, ExpertError> {
    // 越界防护：包目录必须落在根内（含 symlink 逃逸）。
    ensure_within(root, dir)?;
    let plugin_dir = dir.join(paths::PLUGIN_DIR);
    let plugin_meta = match fs::symlink_metadata(&plugin_dir) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => return Err(ExpertError::io(&plugin_dir, &e)),
    };
    let has_agents_dir = fs::metadata(dir.join(paths::AGENTS_DIR))
        .map(|meta| meta.is_dir())
        .unwrap_or(false);
    let has_stray_manifest = fs::symlink_metadata(dir.join(paths::MANIFEST_FILE)).is_ok();
    if plugin_meta.is_none() && !has_agents_dir && !has_stray_manifest {
        // 不是专家包目录 ⇒ 忽略（§4.1 第 1 条）。
        return Ok(None);
    }
    if let Some(meta) = plugin_meta {
        if !meta.is_dir() {
            // `.lengsuo-plugin` 是文件而非目录 ⇒ 包结构损坏。
            return Err(ExpertError::PluginManifestMissing {
                path: plugin_dir.join(paths::MANIFEST_FILE),
            });
        }
        ensure_within(root, &plugin_dir)?;
    }
    let manifest_path = plugin_dir.join(paths::MANIFEST_FILE);
    ensure_within(root, &manifest_path)?;
    let text = match fs::read_to_string(&manifest_path) {
        Ok(text) => text,
        // 专家目录存在但 manifest 缺失 ⇒ 显式错误（否则专家凭空消失）。
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(ExpertError::PluginManifestMissing {
                path: manifest_path,
            })
        }
        Err(e) => return Err(ExpertError::io(&manifest_path, &e)),
    };
    // manifest 损坏 ⇒ 显式错误（可读错误码，见 §17）。
    let manifest = ExpertManifest::from_json_str(&text)?;
    // 标记文件不得经 symlink 逃出根。
    let downloaded_at_file = dir.join(paths::DOWNLOADED_AT_FILE);
    ensure_within(root, &downloaded_at_file)?;
    let downloaded_at = fs::read_to_string(downloaded_at_file)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    Ok(Some(ExpertPackage {
        id: manifest.name.clone(),
        manifest,
        dir: dir.to_path_buf(),
        root: root.to_path_buf(),
        scope,
        origin: ExpertOrigin::Local,
        downloaded_at,
        overrides: Vec::new(),
    }))
}
