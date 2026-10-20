//! 30 段规格表的数据本体：07 册 §3.1 的 30 个标签（原文顺序）+ 规格 §2 的逐行判定。
//!
//! 行类型（`Row` / `Verdict` / `Form`）、条件成立判定与模式夹具在 `support.rs`。

#![allow(dead_code)]

use super::rows_11_20::ROWS_11_TO_20;
use super::rows_1_10::ROWS_1_TO_10;
use super::rows_21_30::ROWS_21_TO_30;
use super::support::Row;

/// 「不做」段的识别标记：表格里 `not_done` 的判据是这些段**从不出现**。
/// 每个标记是参考册中该标签独有的正文关键词（本仓任何模式都不应出现）。
pub const NOT_DONE_MARKERS: [(&str, [&str; 1]); 9] = [
    // 1 <user_query>：用户输入由消息流承载，不写进系统提示词。
    ("user_query", ["本轮用户输入："]),
    // 11 <automations>：依赖 `automation_update` 工具，本仓无自动化域。
    ("automations", ["automation_update"]),
    // 13 <file_id>：CloudFile 域的会话文件引用，本仓工具面无此标识。
    ("file_id", ["file_id"]),
    // 14 <doc_url>：文档工具产出的 `?_fid=` 链接，本仓无文档域。
    ("doc_url", ["?_fid="]),
    // 15 <instructions_for_visualizer>：widget 工具族的调用说明。
    ("instructions_for_visualizer", ["show_widget"]),
    // 17 <visualizer_examples>：widget 工具族的示例。
    ("visualizer_examples", ["widget_guidelines"]),
    // 21 <user-prompt-submit-hook>：宿主层钩子机制，不在提示词模板层。
    ("user-prompt-submit-hook", ["user-prompt-submit-hook"]),
    // 24 <agent_mail>：依赖内置邮件 MCP 服务器。
    ("agent_mail", ["agent-mail"]),
    // 30 <binary_context>：多模态产物域（二进制上下文）。
    ("binary_context", ["二进制上下文"]),
];

/// 规格 §2 的 30 行判定表（编号 = 07 §3.1 原文顺序）：三份分段表按序拼成。
///
/// 行数据在 `rows_1_10` / `rows_11_20` / `rows_21_30`（每行 `source` 的册号节号
/// 可 grep 回参考册；`not_done` 只在 `Verdict::NotDone` 时非空，理由与重新考虑
/// 条件同步写进 `docs/prompt-sections-full-status.md` §4）。
///
/// 用 `Vec<Row>` 而不是常量 `[Row; 30]`：const 上下文不能调用 `copy_from_slice`。
/// 长度由 `table_has_thirty_rows_in_reference_order` 断言。
pub fn table() -> Vec<Row> {
    let mut rows = Vec::with_capacity(30);
    rows.extend_from_slice(&ROWS_1_TO_10);
    rows.extend_from_slice(&ROWS_11_TO_20);
    rows.extend_from_slice(&ROWS_21_TO_30);
    rows
}
