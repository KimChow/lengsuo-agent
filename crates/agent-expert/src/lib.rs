//! 专家包（expert package）数据结构层。
//!
//! 专家不是第四套机制，而是「expert 模式的用户自定义实例」（调研 §2，R11-1）：
//! 本 crate 只承载**数据**（manifest + 人格 + 技能挂载），运行时复用既有的
//! expert 模式链路。
//!
//! 真相来源：调研 `harness-expert.md` §3（`plugin.json` 逐字段判定表）。
//!
//! 边界：
//! - 校验 → 卡 2（`validate/`）；发现/注册表 → 卡 3（`registry.rs`）；
//!   路径 → 卡 4（`paths.rs`）；人格注入 → 卡 5（`agent-prompts`）。
//!
//! 字段名与 wire 格式**原样保留**样本的 camelCase 键名——跨端契约，改名会
//! 破坏导入导出互通；唯一改名是插件目录 `.codebuddy-plugin/` →
//! `.lengsuo-plugin/`（§3.3），见 [`paths::PLUGIN_DIR`]。
//!
//! ```no_run
//! use agent_expert::{ExpertManifest, ExpertType};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 解析一份专家包 manifest（`.lengsuo-plugin/plugin.json` 的文本）
//!     let text = std::fs::read_to_string("senior-developer/.lengsuo-plugin/plugin.json")?;
//!     let manifest = ExpertManifest::from_json_str(&text)?;
//!
//!     assert_eq!(manifest.expert_type, ExpertType::Agent);
//!     // 人格文件路径是派生值：`agents/<agentName>.md`
//!     println!("人格: {:?}", manifest.persona());
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]

pub mod error;
pub mod manifest;
pub mod paths;
pub mod registry;
pub mod skills;
pub mod validate;
pub mod validate_errors;

pub use error::{ExpertError, Severity};
pub use manifest::{
    Author, CategoryId, Dependencies, ExpertManifest, ExpertMember, ExpertType, LocalizedText,
    MemberRole, PromptPair, TeamInfo,
};
pub use paths::{
    AGENTS_DIR, AVATARS_DIR, BIN_DIR, DOWNLOADED_AT_FILE, MANIFEST_FILE, PLUGIN_DIR,
    PROJECT_EXPERTS_DIR, REGISTRY_FILE, SKILLS_DIR, USER_EXPERTS_DIR,
};
pub use registry::{
    ExpertOrigin, ExpertPackage, ExpertRegistry, ExpertRoot, ExpertScope, OverrideRecord,
    RegistryEntry, RegistryFile, ShadowedExpert,
};
pub use skills::{
    catalog_roots, mount_expert_skills, mounted_name, resolve_mounted, visible_skills,
    MountedSkill, VisibleSkill,
};
pub use validate::{RuleSet, ValidateContext, ValidationReport, Validator};
pub use validate_errors::{ItemError, ValidateError};
