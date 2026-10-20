//! 步 1：目录/文件结构检查（`SKILL.md` 存在、附属文件是否越界）。
//!
//! 参考件：09 册 §2.1（技能包 = `SKILL.md` + 可选同目录 `scripts/`
//! `references/` `assets/`）。允许的捆绑目录只有这三个，根目录下散落的
//! 其它文件属越界附属文件 ⇒ P0。
//!
//! 越界判定用 canonical 路径：软链指向技能目录之外 ⇒ P0（与 `catalog`
//! 的授权边界同源思路，但此处判的是「包内结构」而非「授权根边界」）。

use std::fs;
use std::path::{Path, PathBuf};

use super::{AuditStep, BundleFile, Finding, Package, RiskLevel};

/// 允许的捆绑目录名（09 册 §2.1）。
pub(crate) const BUNDLE_DIRS: [&str; 3] = ["scripts", "references", "assets"];

/// 步 1：检查技能目录结构，同时收集包清单（供步 2/3 复用）。
///
/// 返回（包清单, 结论）。结构问题一律 P0：技能包形状不对时，后续步骤的
/// 结论不可信，按 fail-closed 处理。
pub(crate) fn inspect(dir: &Path) -> (Package, Vec<Finding>) {
    let mut findings = Vec::new();
    let skill_md = dir.join(crate::parse::SKILL_FILE);

    // `SKILL.md` 必须是普通文件；缺失或形状不对都不可加载。
    let mut skill_md_ok = false;
    match fs::metadata(&skill_md) {
        Ok(meta) if meta.is_file() => skill_md_ok = true,
        Ok(_) => findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: crate::parse::SKILL_FILE.to_string(),
            message: "SKILL.md 不是普通文件".to_string(),
        }),
        Err(source) => findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: crate::parse::SKILL_FILE.to_string(),
            message: format!("SKILL.md 不可访问: {source}"),
        }),
    }

    // `SKILL.md` 经软链不得越出技能目录（与捆绑路径同一判据）。
    if skill_md_ok && !stays_inside(&skill_md, dir) {
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: crate::parse::SKILL_FILE.to_string(),
            message: "SKILL.md 经软链越出技能目录".to_string(),
        });
        skill_md_ok = false;
    }

    // 技能目录本身必须可读且是目录。
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) => {
            findings.push(Finding {
                level: RiskLevel::P0,
                step: AuditStep::Structure,
                location: dir.display().to_string(),
                message: format!("技能目录不可读: {source}"),
            });
            return (
                Package {
                    dir: dir.to_path_buf(),
                    skill_md: None,
                    files: Vec::new(),
                },
                findings,
            );
        }
    };

    // 根目录下只允许 `SKILL.md` 与三个捆绑目录；其余条目一律越界。
    let mut roots: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                findings.push(Finding {
                    level: RiskLevel::P0,
                    step: AuditStep::Structure,
                    location: dir.display().to_string(),
                    message: format!("读取目录项失败: {source}"),
                });
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == crate::parse::SKILL_FILE {
            continue;
        }
        let is_bundle_dir = BUNDLE_DIRS.contains(&name.as_ref());
        let is_dir = entry.path().is_dir();
        if is_bundle_dir && is_dir {
            roots.push(entry.path());
            continue;
        }
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: name.to_string(),
            message: if is_bundle_dir {
                "捆绑目录不是目录".to_string()
            } else {
                "技能包根下存在越界附属文件（只允许 SKILL.md 与 scripts/references/assets）"
                    .to_string()
            },
        });
    }

    // 捆绑目录逐个递归收集；软链越出技能目录 ⇒ P0。
    let mut files = Vec::new();
    for root in roots {
        collect_files(&root, dir, &mut files, &mut findings);
    }
    files.sort_by(|a, b| a.relative.cmp(&b.relative));

    (
        Package {
            dir: dir.to_path_buf(),
            skill_md: skill_md_ok.then_some(skill_md),
            files,
        },
        findings,
    )
}

/// 递归收集捆绑文件：只读，不修改任何内容。
///
/// 越界软链（canonical 目标不在技能目录内）记 P0 并停止下探。
fn collect_files(
    path: &Path,
    skill_dir: &Path,
    out: &mut Vec<BundleFile>,
    findings: &mut Vec<Finding>,
) {
    // canonical 后必须仍在技能目录内：否则是逃逸软链。
    let canonical = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(source) => {
            findings.push(Finding {
                level: RiskLevel::P0,
                step: AuditStep::Structure,
                location: relative_of(path, skill_dir),
                message: format!("捆绑路径不可解析: {source}"),
            });
            return;
        }
    };
    let base = match skill_dir.canonicalize() {
        Ok(base) => base,
        Err(_) => skill_dir.to_path_buf(),
    };
    if !canonical.starts_with(&base) {
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: relative_of(path, skill_dir),
            message: "捆绑路径经软链越出技能目录".to_string(),
        });
        return;
    }

    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(source) => {
            findings.push(Finding {
                level: RiskLevel::P0,
                step: AuditStep::Structure,
                location: relative_of(path, skill_dir),
                message: format!("捆绑路径元数据不可读: {source}"),
            });
            return;
        }
    };

    if meta.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(source) => {
                findings.push(Finding {
                    level: RiskLevel::P0,
                    step: AuditStep::Structure,
                    location: relative_of(path, skill_dir),
                    message: format!("捆绑目录不可读: {source}"),
                });
                return;
            }
        };
        for entry in entries {
            match entry {
                Ok(entry) => collect_files(&entry.path(), skill_dir, out, findings),
                Err(source) => findings.push(Finding {
                    level: RiskLevel::P0,
                    step: AuditStep::Structure,
                    location: relative_of(path, skill_dir),
                    message: format!("读取目录项失败: {source}"),
                }),
            }
        }
        return;
    }

    if !meta.is_file() {
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Structure,
            location: relative_of(path, skill_dir),
            message: "捆绑项不是普通文件".to_string(),
        });
        return;
    }

    out.push(BundleFile {
        relative: relative_of(path, skill_dir),
        absolute: path.to_path_buf(),
        bytes: meta.len(),
    });
}

/// 相对技能目录的路径（用 `/` 连接，便于跨平台比对）。
fn relative_of(path: &Path, skill_dir: &Path) -> String {
    let relative = path.strip_prefix(skill_dir).unwrap_or(path);
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// 路径经软链解析后是否仍落在技能目录内。
///
/// canonical 失败（路径已消失）时按「不在内」处理——不可解析的路径不当作安全。
fn stays_inside(path: &Path, skill_dir: &Path) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let base = skill_dir
        .canonicalize()
        .unwrap_or_else(|_| skill_dir.to_path_buf());
    canonical.starts_with(&base)
}
