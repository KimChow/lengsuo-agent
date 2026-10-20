//! 30 section 全量落地的断言入口（提示词线 A6）。
//!
//! 六条断言按「表 → 判定位 → 不渲染 → 锚点 → 段序 → agent loop」分组，
//! 每组一个子模块；共享夹具（30 行表）在 `sections/support.rs`。

mod common;

#[path = "sections/anchors.rs"]
mod anchors;
#[path = "sections/order.rs"]
mod order;
#[path = "sections/rows_11_20.rs"]
mod rows_11_20;
#[path = "sections/rows_1_10.rs"]
mod rows_1_10;
#[path = "sections/rows_21_30.rs"]
mod rows_21_30;
#[path = "sections/support.rs"]
mod support;
#[path = "sections/table.rs"]
mod table;
#[path = "sections/table_rows.rs"]
mod table_rows;
