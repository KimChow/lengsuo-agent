//! 专家包路径常量（**占位**：本卡不做路径，发现/路径属卡 3 / 卡 4）。
//!
//! 调研 §4.1 给出的本仓落地方案（此处仅**记录**约定，不实现逻辑）：
//!
//! ```text
//! ~/.lengsuo/experts/                    # 用户级专家根（跨项目）
//! ├── registry.json                      # 注册表（专家注册/发现索引）
//! └── <expert-id>/                       # 一个专家包 = 一个目录
//!     ├── .lengsuo-plugin/plugin.json    # 原名 .codebuddy-plugin/（§3.3 唯一改名）
//!     ├── agents/…
//!     ├── skills/…
//!     └── avatars/…
//!
//! <workspace>/.lengsuo/experts/          # 项目级专家根（可选，覆盖用户级）
//! └── <expert-id>/…
//! ```
//!
//! 卡 3（发现/注册表）与卡 4（路径常量接入既有路径约定）会覆盖本文件；
//! 本卡只留常量名与说明，避免它们重复发明。

/// 插件目录名（原 `.codebuddy-plugin/`，本仓改名为 `.lengsuo-plugin/`，§3.3）。
pub const PLUGIN_DIR: &str = ".lengsuo-plugin";

/// manifest 文件名（插件目录内的 `plugin.json`）。
pub const MANIFEST_FILE: &str = "plugin.json";

/// 人格目录名（`agents/`）。
pub const AGENTS_DIR: &str = "agents";

/// 技能目录名（`skills/`）。
pub const SKILLS_DIR: &str = "skills";

/// 头像目录名（`avatars/`）。
pub const AVATARS_DIR: &str = "avatars";

/// 可选通用 CLI 目录（`bin/`）。
pub const BIN_DIR: &str = "bin";

/// 用户级专家根相对 home 的路径（`~/.lengsuo/experts`，§4.1）。
pub const USER_EXPERTS_DIR: &str = ".lengsuo/experts";

/// 项目级专家根相对工作区的路径（`<ws>/.lengsuo/experts`，§4.1）。
pub const PROJECT_EXPERTS_DIR: &str = ".lengsuo/experts";

/// 注册表文件名（专家根下 `registry.json`，§4.2）。
pub const REGISTRY_FILE: &str = "registry.json";

/// 下载时间标记文件名（**规范外**，§1.3：保留作注册表展示/排序用）。
pub const DOWNLOADED_AT_FILE: &str = ".downloaded_at";
