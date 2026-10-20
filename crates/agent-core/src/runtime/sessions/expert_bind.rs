//! 专家召唤校验与人格取用（专家体系 · 卡 6）：**纯逻辑**，不碰会话表。
//!
//! 规格 `harness-expert.md` §10.2 步骤 4（组装期按 `expert_id` 从注册表取人格）
//! 与 §11.2 I3（失败可重试：召唤动作必须**先**校验、**后**写字段）。
//!
//! 分层（对齐注册表卡的「注册表是索引不是真相」）：
//!
//! 1. **定位**（[`super::experts::ExpertDirs::locate`]）：按 id 在双根里找包目录，
//!    只载入与 id 相关的候选（与 id 无关的坏包不影响召唤）。
//! 2. **校验**（[`BindTarget::resolve`]）：磁盘 manifest 已是真相（清单一律重新从
//!    包目录读，不信索引元数据）⇒ §17 全表校验（fail-closed）⇒ 读人格正文。
//!    任一失败 ⇒ [`CoreError::ExpertUnavailable`]，调用方据此 **不写**
//!    `Session.expert_id`，会话保持原状（I3：失败可重试）。
//! 3. **组装期**（[`BindTarget::resolve`] 的降级形态）：读不到 ⇒ 不注入人格段，
//!    **不改会话状态**（`expert_id` 是召唤时已校验过的状态）。
//!
//! 召唤与组装共用 [`BindTarget::resolve`] + [`load_persona`] 同一套读取逻辑，故同一
//! 人格文件在两个面读出的正文一致。

use std::path::{Path, PathBuf};

use agent_expert::{ExpertError, ExpertManifest, Validator};

use crate::error::CoreError;

use super::experts::ExpertDirs;

/// 注册表里没有该 id 时的错误码（与专家包错误码区分）。
pub(crate) const EXPERT_NOT_FOUND: &str = "EXPERT_NOT_FOUND";

/// 一次成功校验的召唤目标：id、展示名、包目录、已读出的（未脱敏）人格正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindTarget {
    /// 专家 id（= `plugin.json.name` = 目录名）。
    pub expert_id: String,
    /// 展示名（中文花名；供 UI 显示，不进模型输入）。
    pub display_name: String,
    /// 包目录（人格文件的所在包）。
    pub dir: PathBuf,
    /// 人格正文（已去掉 front-matter；**未脱敏**，脱敏在调用方做）。
    pub persona: String,
}

impl BindTarget {
    /// 按 id 在双根中**定位并校验**专家包（召唤入口；纯判定，不碰会话表）。
    ///
    /// 失败分支（判定顺序即错误码顺序）：
    ///
    /// | # | 分支 | 错误码 |
    /// |---|---|---|
    /// | 1 | 两个根下都没有该 id 的包（或包 manifest 的 `name` ≠ id） | `EXPERT_NOT_FOUND` |
    /// | 2 | 候选包目录内缺 `.lengsuo-plugin/plugin.json` | `PLUGIN_MANIFEST_MISSING` |
    /// | 3 | manifest 不可读 / 不可解析 | `IO_ERROR` / `MANIFEST_PARSE_FAILED` |
    /// | 4 | 包未通过 §17 校验（fail-closed，取第一条阻断发现；#3b 目录名按忠实口径开启） | 该发现的稳定码（如 `TAG_COUNT`） |
    /// | 5 | 人格正文缺失或为空 | `AGENT_PATH_MISSING` |
    ///
    /// 定位口径见 [`ExpertDirs::locate`]：只载入与 id 相关的候选目录，**与 id 无关的
    /// 坏包不会让召唤失败**（否则 I3 的「不存在」与「包损坏」两码无法区分）。
    /// 校验口径与卡 2 一致：任一 Error 级发现即整体失败（`Validator::validate_dir`）。
    pub(crate) fn resolve(dirs: &ExpertDirs, expert_id: &str) -> Result<Self, CoreError> {
        let package = match dirs.locate(expert_id) {
            Ok(Some(package)) => package,
            Ok(None) => return Err(not_found(expert_id)),
            Err(e) => return Err(from_expert(expert_id, e)),
        };
        let dir = package.dir;
        let manifest = package.manifest;
        // §17 #3b：`name` == 目录名（**忠实口径**：规格原文是 Error 级；调用方已按 id
        // 定位到此处，故这里比的是 manifest `name` 与磁盘目录名）。
        let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string());
        if dir_name.as_deref() != Some(manifest.name.as_str()) {
            let mismatch = ExpertError::NameDirMismatch {
                name: manifest.name.clone(),
                dir: dir_name.unwrap_or_default(),
            };
            return Err(from_expert(expert_id, mismatch));
        }
        // §17 其余各项（fail-closed）：损坏包在此拦下 ⇒ 召唤失败且不写 `expert_id`（I3）。
        if let Err(finding) = Validator::new().validate_dir(&dir, &manifest) {
            return Err(unavailable(expert_id, finding.code(), finding.to_string()));
        }
        let persona = load_persona(&dir, &manifest)?
            .ok_or_else(|| unavailable(expert_id, "AGENT_PATH_MISSING", "人格正文为空或缺失"))?;
        Ok(BindTarget {
            expert_id: expert_id.to_string(),
            display_name: manifest.display_name.zh.clone(),
            dir,
            persona,
        })
    }
}

/// 读人格正文：manifest 声明的人格文件（§17 #6/#7 派生值 `persona`）。
///
/// 文件缺失/为空 ⇒ `Ok(None)`；其它 IO 错误 ⇒ `Err`。
fn load_persona(dir: &Path, manifest: &ExpertManifest) -> Result<Option<String>, CoreError> {
    let Some(declared) = manifest.persona() else {
        return Ok(None);
    };
    let relative = declared.trim_start_matches("./");
    let path = dir.join(relative);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let body = strip_front_matter(&text);
    let body = body.trim();
    if body.is_empty() {
        return Ok(None);
    }
    Ok(Some(body.to_string()))
}

/// 去掉 Agent MD 的 front-matter（`---` 包裹的头部），只留人格正文。
///
/// front-matter 是**元数据**（`name` 等），不是人格内容；注入模型时只给正文。
fn strip_front_matter(text: &str) -> &str {
    let trimmed = text.trim_start_matches('\u{feff}');
    let rest = match trimmed.strip_prefix("---") {
        Some(rest) => rest,
        None => return trimmed,
    };
    let rest = rest.trim_start_matches('\n');
    let rest = match rest.split_once("\n---") {
        Some((_, after)) => after,
        None => return trimmed,
    };
    rest.trim_start_matches('-').trim_start_matches('\n')
}

/// 「注册表里没有该专家」（与包错误码区分：I3 要求两类失败可区分）。
fn not_found(expert_id: &str) -> CoreError {
    unavailable(expert_id, EXPERT_NOT_FOUND, "注册表中无此专家")
}

/// 构造「专家不可用」错误（错误码 = 稳定字符串）。
fn unavailable(expert_id: &str, code: impl Into<String>, reason: impl Into<String>) -> CoreError {
    CoreError::ExpertUnavailable {
        expert_id: expert_id.to_string(),
        code: code.into(),
        reason: reason.into(),
    }
}

/// 专家包错误 ⇒ 「专家不可用」（保留其稳定错误码）。
fn from_expert(expert_id: &str, e: ExpertError) -> CoreError {
    unavailable(expert_id, e.code(), e.to_string())
}
