//! 专家包导出（§12 工作流第 7 步：`package` **退化为** export）。
//!
//! 规格：`harness-expert.md` §12 第 7 步——无云市场，把专家目录打成 zip；
//! **排除** `registry.json` 与 `.in_use/`；分享云链退化为导出文件（§15 / §19.1）。
//!
//! # ZIP 格式（本仓自实现，不引依赖）
//!
//! 仓内没有 zip 依赖且 `Cargo.lock` 属共享禁区，故本文件实现**最小 ZIP 写入器**：
//!
//! - 存储方式固定 **store（method 0，不压缩）**；
//! - 文件头 + 中央目录 + EOCD 三段齐全，文件名 UTF-8（general purpose bit 11
//!   = `0x0800` 置位）；
//! - CRC-32 用纯函数实现（IEEE 多项式 `0xEDB88320`，反射式，初值/终值取反）；
//! - 目录条目与文件条目都写（空目录也保留），目录名以 `/` 结尾；
//! - 条目顺序 = 目录树的字典序（同输入同输出）。
//!
//! 局限（显式记录）：不支持 ZIP64（条目数 > 65535 或单文件 > 4 GiB 报错）、
//! 不支持压缩与加密、不跟随 symlink（跳过，避免把包外文件带进导出物）。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::ExpertError;
use crate::paths;
use crate::registry::{ExpertRegistry, ExpertScope, RegistryEntry};

/// 进程互斥标记目录（§1.3：本仓不做，但导出时必须排除）。
const IN_USE_DIR: &str = ".in_use";

/// 导出结果摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    /// 专家 id。
    pub id: String,
    /// 产出的 zip 路径。
    pub path: PathBuf,
    /// 文件条目数。
    pub files: usize,
    /// 目录条目数（含空目录）。
    pub dirs: usize,
    /// 产出的 zip 字节数。
    pub bytes: u64,
}

/// 把一个已注册专家导出为 zip（§12 第 7 步）。
///
/// - id 未注册 ⇒ 显式错误（`ExpertError::Io`，`NotFound` 语义）；
/// - 排除 `registry.json` 与 `.in_use/`（**任何层级**）；
/// - zip 内路径相对专家包目录（`.lengsuo-plugin/plugin.json` 位于 zip 根）。
pub fn export_expert(
    registry: &ExpertRegistry,
    id: &str,
    out_path: &Path,
) -> Result<ExportSummary, ExpertError> {
    let entry = registry.find(id).ok_or_else(|| not_found(id))?;
    let dir = resolve_dir(registry, entry)?;
    let mut items = Vec::new();
    collect(&dir, "", &mut items)?;

    let files = items.iter().filter(|item| !item.is_dir).count();
    let dirs = items.len() - files;
    let mut zip = Vec::new();
    write_zip(&items, out_path, &mut zip)?;
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| ExpertError::io(parent, &e))?;
        }
    }
    fs::write(out_path, &zip).map_err(|e| ExpertError::io(out_path, &e))?;
    Ok(ExportSummary {
        id: id.to_string(),
        path: out_path.to_path_buf(),
        files,
        dirs,
        bytes: zip.len() as u64,
    })
}

/// 定位已注册专家的包目录。
///
/// 优先用注册表里的运行时目录索引（`register_expert` / 发现流程填充）；索引
/// 缺失时按 §4.1 的标准根（用户级 `~/.lensuo/experts`、项目级
/// `<cwd>/.lensuo/experts`）还原 `registry.json` 里的相对路径。
fn resolve_dir(registry: &ExpertRegistry, entry: &RegistryEntry) -> Result<PathBuf, ExpertError> {
    if let Some(dir) = registry.dir_of(&entry.id) {
        if dir.is_dir() {
            return Ok(dir.to_path_buf());
        }
    }
    let base = match entry.scope {
        ExpertScope::User => std::env::home_dir().map(|home| paths::user_experts_root(&home)),
        ExpertScope::Project => std::env::current_dir()
            .ok()
            .map(|cwd| paths::project_experts_root(&cwd)),
    }
    .and_then(|root| root.parent().map(Path::to_path_buf))
    .ok_or_else(|| not_found(&entry.id))?;
    let dir = base.join(&entry.path);
    paths::ensure_within(&base, &dir).map_err(|_| ExpertError::PathEscape {
        path: dir.display().to_string(),
    })?;
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(not_found(&entry.id))
    }
}

/// 未注册 id 的显式错误（错误契约无 NotFound 变体，用 `Io` 承载语义）。
fn not_found(id: &str) -> ExpertError {
    ExpertError::Io {
        path: PathBuf::from(id),
        reason: format!("专家未注册 ({})", ErrorKind::NotFound),
    }
}

/// 一条待写 zip 的条目。
struct ZipEntry {
    name: String,
    data: Vec<u8>,
    is_dir: bool,
    crc: u32,
}

/// 递归收集目录内容（字典序；跳过 `registry.json`、`.in_use/` 与 symlink）。
fn collect(root: &Path, rel: &str, out: &mut Vec<ZipEntry>) -> Result<(), ExpertError> {
    let mut children = Vec::new();
    for item in fs::read_dir(root).map_err(|e| ExpertError::io(root, &e))? {
        let item = item.map_err(|e| ExpertError::io(root, &e))?;
        children.push(item.path());
    }
    children.sort();
    for path in children {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if name == paths::REGISTRY_FILE || name == IN_USE_DIR {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|e| ExpertError::io(&path, &e))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let entry_name = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if meta.is_dir() {
            out.push(ZipEntry {
                name: format!("{entry_name}/"),
                data: Vec::new(),
                is_dir: true,
                crc: 0,
            });
            collect(&path, &entry_name, out)?;
        } else if meta.is_file() {
            let data = fs::read(&path).map_err(|e| ExpertError::io(&path, &e))?;
            let crc = crc32(&data);
            out.push(ZipEntry {
                name: entry_name,
                data,
                is_dir: false,
                crc,
            });
        }
    }
    Ok(())
}

/// 写最小 ZIP（store + UTF-8 文件名 + 中央目录 + EOCD）。
fn write_zip(entries: &[ZipEntry], out_path: &Path, out: &mut Vec<u8>) -> Result<(), ExpertError> {
    if entries.len() > u16::MAX as usize {
        return Err(zip_error(out_path, "条目数超出非 ZIP64 上限"));
    }
    let mut central = Vec::new();
    for entry in entries {
        let name = entry.name.as_bytes();
        if name.len() > u16::MAX as usize || entry.data.len() > u32::MAX as usize {
            return Err(zip_error(out_path, "条目超出非 ZIP64 上限"));
        }
        let offset = out.len() as u32;
        write_u32(out, 0x0403_4b50);
        write_u16(out, 20);
        write_u16(out, 0x0800);
        write_u16(out, 0);
        write_u16(out, 0);
        write_u16(out, 0x0021);
        write_u32(out, entry.crc);
        write_u32(out, entry.data.len() as u32);
        write_u32(out, entry.data.len() as u32);
        write_u16(out, name.len() as u16);
        write_u16(out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(&entry.data);

        write_u32(&mut central, 0x0201_4b50);
        write_u16(&mut central, 20);
        write_u16(&mut central, 20);
        write_u16(&mut central, 0x0800);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0x0021);
        write_u32(&mut central, entry.crc);
        write_u32(&mut central, entry.data.len() as u32);
        write_u32(&mut central, entry.data.len() as u32);
        write_u16(&mut central, name.len() as u16);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u32(&mut central, if entry.is_dir { 0x10 } else { 0 });
        write_u32(&mut central, offset);
        central.extend_from_slice(name);
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    write_u32(out, 0x0605_4b50);
    write_u16(out, 0);
    write_u16(out, 0);
    write_u16(out, entries.len() as u16);
    write_u16(out, entries.len() as u16);
    write_u32(out, central.len() as u32);
    write_u32(out, central_offset);
    write_u16(out, 0);
    Ok(())
}

/// ZIP 结构错误（无专属错误变体，`Io` 承载并保留输出路径）。
fn zip_error(path: &Path, reason: &str) -> ExpertError {
    ExpertError::Io {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// CRC-32（IEEE 802.3，反射式多项式 `0xEDB88320`，初值/终值取反）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
