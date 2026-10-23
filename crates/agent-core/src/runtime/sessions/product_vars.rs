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
//!   其中 `SOUL.md` 与 `AI_NAME` 的正文读取已有既有实现
//!   （`agent_memory::MemoryEngine::read_identity`，`context.rs` 继续复用）；
//!   本模块只补**其余文件的正文与四个路径**。
//! - **产品配置**：`<home>/product.json`。仓内尚未创建该文件；本卡只定义读取口径
//!   与缺文件时的显式降级（缺文件 / 解析失败 / 字段缺失 ⇒ 空串或 `false`）。
//!   字段为 snake_case：`product_name` / `data_folder_name` / `is_windows`。
//!
//! ## `USER.md` 的两义（须知）
//!
//! `agent-memory` 把 `USER.md` 当**用户名文件**读（`agent-memory/src/identity.rs:33`
//! 的 `USER_NAME_FILE`，内容即称呼本身）；本模块把同一个文件当**用户画像正文**
//! 读（`PromptVars::user`）。两者不冲突于磁盘（同一路径），但语义不同：
//! 变量 `user_name` 是「称呼」、`user` 是「档案正文」。卡面 D1 明确 `user` ← `USER.md`，
//! 故本卡照此接线；若后续身份卡决定把画像正文另立文件（如 `USER_PROFILE.md`），
//! 只需改本模块的 `USER_FILE` 常量。
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
    /// 身份注入模式：`onboarding` / `normal`。
    ///
    /// 取 `onboarding` **当且仅当 `BOOTSTRAP.md` 存在**——与参考端
    /// `IdentityCollector` 逐条同形（`s = fileExists(BOOTSTRAP.md)`，
    /// `WorkspaceIdentityMode = s ? "onboarding" : "normal"`）。
    /// 参考端只在「专家回合」跳过 BOOTSTRAP 判定；本仓运行时无专家回合分支，
    /// 故恒走非专家判定（专家人格走 `plugin_agent_prompt` 单独一路）。
    pub identity_mode: String,
}

/// 身份文件正文的截断上限（字符数）。
///
/// 【出处：参考端 `IdentityCollector` 的 `truncateContent(content, xl)`，
/// 其中 `xl = 1e4`】。超限时截断并追加固定标记，避免单个身份文件把上下文吃满。
const IDENTITY_TRUNCATE_CHARS: usize = 10_000;

/// 超限截断标记（与参考端逐字一致）。
const TRUNCATED_SUFFIX: &str = "\n... (content truncated — exceeded character limit)";

/// 超限截断：不超限原样返回，超限取前 [`IDENTITY_TRUNCATE_CHARS`] 个字符并加标记。
///
/// 按 **char** 取前缀而非按字节，避免切坏多字节字符（参考端按 UTF-16 长度截断，
/// 与按 char 截断在 BMP 内等价；本仓正文为中文/BMP 为主）。
fn truncate_body(body: &str) -> String {
    if body.chars().count() <= IDENTITY_TRUNCATE_CHARS {
        return body.to_string();
    }
    let mut out: String = body.chars().take(IDENTITY_TRUNCATE_CHARS).collect();
    out.push_str(TRUNCATED_SUFFIX);
    out
}

impl Runtime {
    /// 取产品 / 身份文件类变量。缺文件与读取失败一律显式降级，不 panic。
    pub(crate) fn product_vars(&self, workspace_root: &str) -> ProductVars {
        let paths = MemoryPaths::new(self.workdir.join(".lengsuo"), workspace_root);
        let dir = paths.identity_dir();
        let product = read_product_config(&paths.home);
        // 身份注入模式由**文件存在性**决定，不由配置声明（参考端同形）。
        let identity_mode = if dir.join(BOOTSTRAP_FILE).is_file() {
            "onboarding"
        } else {
            "normal"
        };
        ProductVars {
            product_name: product.product_name,
            data_folder_name: product.data_folder_name,
            identity: truncate_body(&read_text(&dir.join(IDENTITY_FILE))),
            user: truncate_body(&read_text(&dir.join(USER_FILE))),
            bootstrap: truncate_body(&read_text(&dir.join(BOOTSTRAP_FILE))),
            soul_path: path_text(&dir.join(SOUL_FILE)),
            identity_path: path_text(&dir.join(IDENTITY_FILE)),
            user_path: path_text(&dir.join(USER_FILE)),
            bootstrap_path: path_text(&dir.join(BOOTSTRAP_FILE)),
            is_windows: product.is_windows,
            identity_mode: identity_mode.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录（自带随机后缀，测试间不互扰）。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("product-vars-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir
    }

    #[test]
    fn product_config_reads_declared_fields() {
        let dir = temp_dir("config");
        let file = dir.join(PRODUCT_FILE);
        std::fs::write(
            &file,
            r#"{"product_name":"澜枢平台","data_folder_name":"Lengsuo","is_windows":true}"#,
        )
        .expect("写 product.json");

        let config = read_product_config(&dir);
        assert_eq!(config.product_name, "澜枢平台");
        assert_eq!(config.data_folder_name, "Lengsuo");
        assert!(config.is_windows);
    }

    #[test]
    fn missing_or_malformed_product_config_degrades_to_defaults() {
        let dir = temp_dir("degrade");

        // 缺文件 ⇒ 缺省值（空串 / false），不 panic。
        let missing = read_product_config(&dir);
        assert_eq!(missing.product_name, "");
        assert_eq!(missing.data_folder_name, "");
        assert!(!missing.is_windows);

        // 非 JSON ⇒ 缺省值。
        let file = dir.join(PRODUCT_FILE);
        std::fs::write(&file, "not json at all").expect("写坏 JSON");
        let malformed = read_product_config(&dir);
        assert_eq!(malformed.product_name, "");
        assert!(!malformed.is_windows);

        // 字段类型不符 ⇒ 缺省值（字符串不当作 bool，整数不当作 string）。
        std::fs::write(&file, r#"{"product_name":42,"is_windows":"true"}"#).expect("写错类型");
        let wrong_types = read_product_config(&dir);
        assert_eq!(wrong_types.product_name, "");
        assert!(!wrong_types.is_windows);
    }

    #[test]
    fn identity_bodies_are_read_and_paths_point_at_the_files() {
        let dir = temp_dir("identity");
        let identity = dir.join("identity");
        std::fs::create_dir_all(&identity).expect("身份目录");
        for (name, body) in [
            (SOUL_FILE, "人格正文"),
            (IDENTITY_FILE, "身份正文"),
            (USER_FILE, "用户画像正文"),
            (BOOTSTRAP_FILE, "引导正文"),
        ] {
            std::fs::write(identity.join(name), body).expect("写身份文件");
        }

        assert_eq!(read_text(&identity.join(IDENTITY_FILE)), "身份正文");
        assert_eq!(read_text(&identity.join(USER_FILE)), "用户画像正文");
        assert_eq!(read_text(&identity.join(BOOTSTRAP_FILE)), "引导正文");
        assert_eq!(
            path_text(&identity.join(SOUL_FILE)),
            identity.join(SOUL_FILE).display().to_string()
        );
    }

    #[test]
    fn unreadable_identity_file_degrades_to_empty_without_panic() {
        let dir = temp_dir("unreadable");
        // 用目录占住文件位置 ⇒ 读文件必然失败（IsADirectory）。
        std::fs::create_dir_all(dir.join(IDENTITY_FILE)).expect("占位目录");

        assert_eq!(read_text(&dir.join(IDENTITY_FILE)), "");
        assert_eq!(read_text(&dir.join("NO_SUCH_FILE.md")), "");
    }

    /// 端到端：真实 `Runtime` 取值 ⇒ 正文与路径都落到 `ProductVars`。
    ///
    /// 集成测试只观测得到「渲染进 prompt 的部分」；`identity_mode` 未声明时
    /// 身份段整段不渲染，故身份正文/路径必须在这里断言（它们确实取到了值）。
    #[test]
    fn runtime_product_vars_reads_bodies_paths_and_product_config() {
        use std::sync::Arc;

        use crate::provider::EchoProvider;

        let workdir = temp_dir("runtime");
        let home = workdir.join(".lengsuo");
        let identity_dir = home.join("identity");
        std::fs::create_dir_all(&identity_dir).expect("身份目录");
        std::fs::write(identity_dir.join(SOUL_FILE), "人格正文").expect("SOUL");
        std::fs::write(identity_dir.join(IDENTITY_FILE), "身份正文").expect("IDENTITY");
        std::fs::write(identity_dir.join(USER_FILE), "用户画像正文").expect("USER");
        std::fs::write(identity_dir.join(BOOTSTRAP_FILE), "引导正文").expect("BOOTSTRAP");
        std::fs::write(
            home.join(PRODUCT_FILE),
            r#"{"product_name":"澜枢平台","data_folder_name":"Lengsuo","is_windows":true}"#,
        )
        .expect("product.json");

        let rt = crate::runtime::Runtime::builder()
            .workdir(&workdir)
            .provider(Arc::new(EchoProvider))
            .build()
            .expect("runtime");

        let vars = rt.product_vars(&workdir.display().to_string());

        assert_eq!(vars.product_name, "澜枢平台");
        assert_eq!(vars.data_folder_name, "Lengsuo");
        assert!(vars.is_windows);
        assert_eq!(vars.identity, "身份正文");
        assert_eq!(vars.user, "用户画像正文");
        assert_eq!(vars.bootstrap, "引导正文");
        // 四个路径都指向既有身份目录（`MemoryPaths::identity_dir()`）。
        let expected_dir = identity_dir.display().to_string();
        assert_eq!(vars.soul_path, format!("{expected_dir}/{SOUL_FILE}"));
        assert_eq!(
            vars.identity_path,
            format!("{expected_dir}/{IDENTITY_FILE}")
        );
        assert_eq!(vars.user_path, format!("{expected_dir}/{USER_FILE}"));
        assert_eq!(
            vars.bootstrap_path,
            format!("{expected_dir}/{BOOTSTRAP_FILE}")
        );
    }
}
