//! 产品身份段（`<product_identity>` 正文）。

/// 产品身份段正文：标题 + 一行身份声明。
///
/// 【出处：参考端 本地对照资料 18 §13「<product_identity>」】。
///
/// 规格原文的变量名是 `{{productName}}`，本仓变量名是 [`PromptVars::product_name`]
/// （见 `model.rs`），故此处用 `{{product_name}}`。正文只此一句，未扩写；
/// `product_name` 为空（纯空白）时整段省略（`if_filled` 语义）。
///
/// [`PromptVars::product_name`]: crate::model::PromptVars::product_name
pub(in crate::templates) const PRODUCT_IDENTITY: &str = "## 产品身份
You are {{product_name}}, a powerful AI assistant.";
