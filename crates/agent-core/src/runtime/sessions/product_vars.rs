//! 产品与身份文件类提示词变量取值（x-prompt-vars-wire 卡）。
//!
//! 与 [`super::memory_vars`] 同目录、同风格：只负责**取值**（读盘 + 归一），
//! 不渲染、不做模板条件判断。`context.rs` 的 `prompt_vars()` 消费本模块结果。
//!
//! ## 读取口径
//!
//! - **身份四文件**：复用 `agent-memory` 的身份目录口径
//!   （[`MemoryPaths::identity_dir`] = `<home>/identity`，`home` = `<workdir>/.lengsuo`），
//!   文件名 `SOUL.md` / `IDENTITY.md` / `USER.md` / `BOOTSTRAP.md`。
//!   其中 `SOUL.md` / `USER.md` / `AI_NAME` 的正文读取已有既有实现
//!   （`agent_memory::MemoryEngine::read_identity`，`context.rs` 继续复用）；
//!   本模块只补**其余文件的正文与四个路径**。
//! - **产品配置**：`<home>/product.json`。仓内尚未创建该文件；本卡只定义读取口径
//!   与缺文件时的显式降级（缺文件 / 解析失败 / 字段缺失 ⇒ 空串或 `false`）。
//!   字段为 snake_case：`product_name` / `data_folder_name` / `is_windows`。
//!
//! ## 降级口径
//!
//! 任何读取失败（缺失、无权限、UTF-8 非法）都降级为空串 / `false`，**不 panic、
//! 不阻断回合**：身份与产品属产品人格，缺内容时模板按既有空值语义整段省略。
//!
//! ## `is_windows` 为什么读配置而不是编译期判定
//!
//! 研发机是 Linux，而产品目标含 Windows 客户端：`cfg!(windows)` /
//! `std::env::consts::OS` 会让 Windows 条件段在研发与 CI 上永远不渲染，
//! 等于白接线。故取运行时显式配置字段（product.json 的 `is_windows`）；
//! 仓内没有会话/工作区级平台字段，缺字段时按 `false` 显式降级。

use std::path::Path;

use agent_memory::MemoryPaths;

use crate::runtime::Runtime;

/// 产品配置文件名（`<home>/product.json`）。
pub(crate) const PRODUCT_FILE: &str = "product.json";

/// 身份文件名（与 `agent-memory` 的身份目录约定一致）。
pub(crate) const SOUL_FILE: &str = "SOUL.md";
pub(crate) const IDENTITY_FILE: &str = "IDENTITY.md";
pub(crate) const USER_FILE: &str = "USER.md";
pub(crate) const BOOTSTRAP_FILE: &str = "BOOTSTRAP.md";

/// 产品 / 身份文件类变量的原始取值（未脱敏）。
///
/// 脱敏由调用方（`context.rs` 的 `prompt_vars()`）统一完成；路径类变量不脱敏
/// （路径不是凭据）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProductVars {
    /// 产品名（product.json）。
    pub product_name: String,
    /// 数据目录名（product.json）。
    pub data_folder_name: String,
    /// IDENTITY.md 正文。
    pub identity: String,
    /// USER.md 正文。
    pub user: String,
    /// BOOTSTRAP.md 正文。
    pub bootstrap: String,
    /// SOUL.md 路径。
    pub soul_path: String,
    /// IDENTITY.md 路径。
    pub identity_path: String,
    /// USER.md 路径。
    pub user_path: String,
    /// BOOTSTRAP.md 路径。
    pub bootstrap_path: String,
    /// 是否 Windows 平台（product.json 的显式字段；缺省 `false`）。
    pub is_windows: bool,
}

impl Runtime {
    /// 取产品 / 身份文件类变量。缺文件与读取失败一律显式降级，不 panic。
    pub(crate) fn product_vars(&self, workspace_root: &str) -> ProductVars {
        let paths = MemoryPaths::new(self.workdir.join(".lengsuo"), workspace_root);
        let dir = paths.identity_dir();
        let product = read_product_config(&paths.home);
        ProductVars {
            product_name: product.product_name,
            data_folder_name: product.data_folder_name,
            identity: read_text(&dir.join(IDENTITY_FILE)),
            user: read_text(&dir.join(USER_FILE)),
            bootstrap: read_text(&dir.join(BOOTSTRAP_FILE)),
            soul_path: path_text(&dir.join(SOUL_FILE)),
            identity_path: path_text(&dir.join(IDENTITY_FILE)),
            user_path: path_text(&dir.join(USER_FILE)),
            bootstrap_path: path_text(&dir.join(BOOTSTRAP_FILE)),
            is_windows: product.is_windows,
        }
    }
}

/// product.json 的已解析字段（本卡只消费三个；其余键忽略）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProductConfig {
    product_name: String,
    data_folder_name: String,
    is_windows: bool,
}

/// 读 product.json；缺文件 / 非 JSON 对象 / 字段类型不符均按缺省值降级。
fn read_product_config(home: &Path) -> ProductConfig {
    let Ok(raw) = std::fs::read_to_string(home.join(PRODUCT_FILE)) else {
        return ProductConfig::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return ProductConfig::default();
    };
    let field = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    ProductConfig {
        product_name: field("product_name"),
        data_folder_name: field("data_folder_name"),
        is_windows: value
            .get("is_windows")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

/// 文件正文；任何读取失败 ⇒ 空串（身份/产品缺失不阻断回合）。
fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 路径的展示文本（不脱敏：路径不是凭据）。
fn path_text(path: &Path) -> String {
    path.display().to_string()
}
