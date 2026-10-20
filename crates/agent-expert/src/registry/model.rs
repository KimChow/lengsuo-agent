//! 注册表数据模型（§4.2 `registry.json` 逐字段）。
//!
//! 与 `registry.rs` 分文件：本文件只放**可序列化**的 wire 结构，发现/读写逻辑
//! 留在 `registry.rs`（对齐本 crate `validate/{mod,structure,...}.rs` 的分层）。

use serde::{Deserialize, Serialize};

use crate::manifest::{CategoryId, ExpertType};

/// 注册表 schema 版本（§4.2 `schemaVersion`）。
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

/// 来源层级：用户级 / 项目级（`registry.json` 的 `scope` 键）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpertScope {
    /// 用户级（`~/.lengsuo/experts`）。
    User,
    /// 项目级（`<ws>/.lengsuo/experts`）。
    Project,
}

impl Default for ExpertScope {
    /// 缺省（`registry.json` 未写 `scope` 时）按用户级处理。
    fn default() -> Self {
        Self::User
    }
}

/// 来源类型（§4.2 `source`：`local | imported`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpertOrigin {
    /// 本地创建/自建包。
    Local,
    /// 导入得到的包（导入流程属卡 8）。
    Imported,
}

/// 注册表条目里的一条覆盖记录（谁被谁覆盖）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRecord {
    /// 被覆盖者的来源层级。
    pub scope: ExpertScope,
    /// 被覆盖者的包路径（相对其 `.lengsuo` 基底，形如 `experts/<id>`）。
    pub path: String,
}

/// 注册表条目（§4.2 `experts[]` 的一项）。
///
/// 字段逐条对应 §4.2 样例；`scope` / `overrides` 是**本仓扩展**（§4.2 样例未给，
/// 但卡 3 要求「覆盖关系要能查出来（注册表里记录来源层级）」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryEntry {
    /// 唯一键（= 目录名 = `plugin.json.name`，不可变）。
    pub id: String,
    /// 版本（取自 `plugin.json.version`）。
    pub version: String,
    /// 专家类型（`agent` / `team`）。
    pub expert_type: ExpertType,
    /// 分类（12 枚举之一）。
    pub category_id: CategoryId,
    /// 包路径：相对该 scope 的 `.lengsuo` 基底（形如 `experts/senior-developer`，
    /// §4.2 样例字面值）；基底不是 `.lengsuo` 时退化为相对根目录的路径。
    pub path: String,
    /// 下载时间（规范外 `.downloaded_at`，缺失则不写该键）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloaded_at: Option<String>,
    /// 注册时间（重建时取当前 UTC）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<String>,
    /// 来源类型（`local` | `imported`）。
    pub source: ExpertOrigin,
    /// 来源层级（`user` | `project`）——**本仓扩展**，供 UI 显示「来自项目」。
    ///
    /// `#[serde(default)]`：§4.2 样例的注册表没有这一键；读到旧格式条目时按
    /// 用户级处理，随后由「漂移即重建」规则校正为磁盘真相。
    #[serde(default)]
    pub scope: ExpertScope,
    /// 被本条覆盖的同名低优先级来源——**本仓扩展**。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideRecord>,
}

/// `registry.json` 文件结构（§4.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryFile {
    /// schema 版本。
    pub schema_version: u32,
    /// 已注册专家表。
    pub experts: Vec<RegistryEntry>,
}
