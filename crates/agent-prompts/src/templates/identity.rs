//! 身份注入段：`<identity_context>` 的两分支渲染（onboarding / 常态）。
//!
//! 【出处：参考端 本地对照资料 18 §13「身份注入运行时」+ 04 §2.2 身份文件清单】。
//!
//! ## 两分支
//!
//! 18 §13 的 `WorkspaceIdentityMode` 两分支在本仓落成**一个字符串变量**
//! `identity_mode`（`"onboarding"` / `"normal"`），分支判定走**既有条件段机制**
//! （`Cond::Eq("identity_mode", …)`），不另造 if、不新增 `Segment` 变体。
//!
//! - **onboarding**：四文件（SOUL / IDENTITY / USER / BOOTSTRAP）全在 +
//!   「出生证明 / 只在对话有空间时遵循 / 学完删 BOOTSTRAP.md / 保持对话自然」纪律段；
//! - **常态**：只渲染 SOUL / IDENTITY / USER 三文件 +
//!   「扮演人格语气 / 与最近注入的档案一致 / 改了 SOUL.md 要告诉用户」纪律段。
//!
//! ## 空值语义（与记忆段相反）
//!
//! 身份段的空值**显式标缺失**，不省略（18 §13 原文 `(empty or missing)`）：
//! 文件缺失本身是 onboarding 的信号，缺整段等于把信号丢了。
//! 记忆段则是「三变量任一非空才渲染整段」——两者语义**相反**，不是同一套口径。
//!
//! 实现上不新增 `Segment` 变体：**渲染前把空值归一成 `(empty or missing)`**，
//! 再走既有的单遍 `{{key}}` 替换；纯空白同样算空。
//!
//! ## 纯数据面
//!
//! 本 crate **不做 IO**：四文件正文与路径都由调用方填进 `PromptVars`，
//! 本模块只负责把已填好的数据渲染成段。

use crate::model::{Cond, PromptMode, PromptTemplate, PromptVars, Segment};

/// 空文件 / 缺路径的显式标（18 §13 原文；**身份段专用**，记忆段不用）。
pub(crate) const EMPTY_MARK: &str = "(empty or missing)";

/// 身份段的开标签。
const OPEN_TAG: &str = "<identity_context>";

/// 身份段的闭标签。
const CLOSE_TAG: &str = "</identity_context>";

/// onboarding 分支纪律段（【出处：18 §13 onboarding 分支原文要点】）。
///
/// 逐条对应 18 §13：「BOOTSTRAP.md 是你的出生证明」/「只在对话有空间时
/// （闲聊/问候/闲置）遵循；用户在干正事就别提身份问题」/「边学边更新
/// SOUL/IDENTITY/USER.md，学完删 BOOTSTRAP.md」/「保持对话自然」。
pub(crate) const ONBOARDING_DISCIPLINE: &str = "## 身份纪律（onboarding）
- BOOTSTRAP.md 是你的出生证明。
- 只在对话有空间时（闲聊、问候、闲置）遵循；用户在干正事就别提身份问题。
- 边学边更新 SOUL.md / IDENTITY.md / USER.md，学完删 BOOTSTRAP.md。
- 保持对话自然。";

/// 常态分支纪律段（【出处：18 §13 常态分支原文要点】）。
///
/// 逐条对应 18 §13：「SOUL.md 存在则扮演其人格与语气」/「与最近注入的身份/用户
/// 档案保持一致」/「改了 SOUL.md 要告诉用户」。
pub(crate) const NORMAL_DISCIPLINE: &str = "## 身份纪律（常态）
- SOUL.md 存在则扮演其人格与语气。
- 与最近注入的身份/用户档案保持一致。
- 改了 SOUL.md 要告诉用户。";

/// 身份段（两分支）的段序列。
///
/// 分支判定用既有条件段机制：`Cond::Eq("identity_mode", "onboarding")` 与
/// `Cond::Eq("identity_mode", "normal")`（规格 §8 的建议形态）。
/// `identity_mode` 取这两个值之外的内容（缺省空串 = 调用方未接入身份注入）
/// ⇒ 两分支都不命中 ⇒ 整段不渲染。
pub(crate) fn identity_segments() -> Vec<Segment> {
    vec![
        Segment::If(
            Cond::Eq("identity_mode", "onboarding"),
            vec![Segment::Text(onboarding_text())],
        ),
        Segment::If(
            Cond::Eq("identity_mode", "normal"),
            vec![Segment::Text(normal_text())],
        ),
    ]
}

/// 渲染身份段（`<identity_context>` 整块）。
///
/// 分支不命中（`identity_mode` 未声明）⇒ 返回空串，不留空洞标记。
pub(crate) fn render_context(vars: &PromptVars) -> String {
    // 段序不依赖模式：身份段内部不含 `work_mode` 条件，故模板模式取常量。
    let template = PromptTemplate::new(PromptMode::Craft, identity_segments());
    let rendered = template.render(&normalized(vars));
    if rendered.trim().is_empty() {
        String::new()
    } else {
        rendered
    }
}

/// onboarding 分支的整块文本。
fn onboarding_text() -> String {
    format!(
        "{OPEN_TAG}\n{}\n{}\n{ONBOARDING_DISCIPLINE}\n{CLOSE_TAG}",
        common_files(),
        file_block("BOOTSTRAP.md", "bootstrap_path", "bootstrap"),
    )
}

/// 常态分支的整块文本。
fn normal_text() -> String {
    format!(
        "{OPEN_TAG}\n{}\n{NORMAL_DISCIPLINE}\n{CLOSE_TAG}",
        common_files(),
    )
}

/// 两分支共有的三个文件块（SOUL / IDENTITY / USER；顺序即 18 §13 的列出序）。
fn common_files() -> String {
    [
        file_block("SOUL.md", "soul_path", "soul"),
        file_block("IDENTITY.md", "identity_path", "identity"),
        file_block("USER.md", "user_path", "user"),
    ]
    .join("\n")
}

/// 单个身份文件块：`## <file>` → `Path: {path}` → 正文（空 ⇒ 显式标缺失）。
///
/// 路径与正文都用 `{{key}}` 占位：空值的「显式标缺失」由 [`normalized`] 在渲染前
/// 归一后走既有单遍替换完成，本函数因此不需要新的段类型。
fn file_block(file: &str, path_key: &str, content_key: &str) -> String {
    format!("## {file}\nPath: {{{{{path_key}}}}}\n{{{{{content_key}}}}}")
}

/// 归一：空（纯空白）内容与空路径一律改写成 `(empty or missing)`。
///
/// 身份段**不省略**空值（18 §13：文件缺失本身是信号），故这里只改写、不删字段。
fn normalized(vars: &PromptVars) -> PromptVars {
    let mut normalized = vars.clone();
    normalized.soul = marked(&normalized.soul);
    normalized.identity = marked(&normalized.identity);
    normalized.user = marked(&normalized.user);
    normalized.bootstrap = marked(&normalized.bootstrap);
    normalized.soul_path = marked(&normalized.soul_path);
    normalized.identity_path = marked(&normalized.identity_path);
    normalized.user_path = marked(&normalized.user_path);
    normalized.bootstrap_path = marked(&normalized.bootstrap_path);
    normalized
}

/// 值非空则取其去空白文本；空（纯空白）则取显式标。
fn marked(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        EMPTY_MARK.to_string()
    } else {
        trimmed.to_string()
    }
}
