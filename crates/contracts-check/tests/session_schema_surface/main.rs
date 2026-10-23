//! 会话/消息 schema 面（§3）：**枚举取值面**与**线上键名面**的契约断言。
//!
//! 本卡治的是**键名漂移**这半个病（另一半是上一张卡的响应码面）。
//! `AGENT-KICKOFF.md` §3 冻结的是**Rust 侧结构体的源字段名**（snake_case：
//! `workspace_root` / `permission_mode` / `created_at_ms` / `ts_ms` …），
//! 而**线上协议用的是 camelCase**（`serde(rename_all = "camelCase")`）。
//! 这个「源码字段名 ≠ 线上键名」的落差在事件 payload 层**没有任何断言守着**，
//! 已造成一处真实漂移：core 发 `argsRef`，壳侧读 `args_ref` ⇒ 恒为 `None`。
//!
//! # 断言口径：**只守关系，不锁具体取值/数量**
//!
//! - 四组枚举：**实现侧取值集合 == 契约登记的取值集合**（集合相等，不锁顺序、
//!   不锁「恰好几项」）；
//! - 线上键名面：**实现侧实际序列化出的键集合 == 契约登记的键集合**（同样集合相等）；
//! - 事件 payload 键名面：**实现源码里扫出的键集合 ⊆ 契约登记的键集合**
//!   （只守 ⊆，实现正当新增键时先补登记即可）；
//! - 壳侧读取：**camelCase 形态（`argsRef`）与旧形态（`args_ref`）都能读到**。
//!
//! ⛔ 刻意**不写** `assert_eq!(x.len(), N)` / `assert_eq!(x, &[...])` 这类锁规模
//! 或锁取值的断言——它们会在正当扩充时必红。
//!
//! 做法照抄 `tests/envelope_code_surface.rs`：**只读文件、手写字符扫描、
//! 不新增依赖、不用正则库**。
//!
//! # 本目录分组说明
//!
//! - `enum_values.rs`：四组枚举的线上取值面断言；
//! - `json_keys.rs`：`Session` / `Message` / `ToolInfo` / `Isolation` 的线上键名面；
//! - `runtime_payload.rs`：事件 payload 键名的 ⊆ 断言与壳侧读取守卫；
//! - `scanner.rs`：`json!` 键名扫描基元（含注释行排除的自证）。
//!
//! 登记常量转集合的 `registered` 辅助留在本文件（target 根），子模块用
//! `use super::registered` 取用。

use std::collections::BTreeSet;

/// 把登记常量转成集合。
fn registered(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| (*k).to_string()).collect()
}

mod enum_values;
mod json_keys;
mod runtime_payload;
mod scanner;
