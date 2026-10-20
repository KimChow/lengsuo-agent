//! quick 模式段集（07 册 §3.2 quick 模板全文 + §3.3 quick 片段清单）。
//!
//! quick 是**极简**模式：参考端只有 `current-mode` + `interaction` 两个片段，
//! 模板 525B 是 fail-closed 范本（07 册 §3.2），另配硬规则清单做双保险。
//! 本仓不挂 agent-loop（quick 无工具），也不挂任务管理与交付协议段。

use crate::model::{PromptMode, Segment};

use super::common::text;

/// quick 的段集（公共头 + 能力边界 + 公共尾）。
pub(crate) fn quick_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    segments.push(text(QUICK_CAPABILITY));
    segments.extend(super::common::common_sections());
    segments.extend(super::common::footer());
    segments
}

/// quick 能力面：只答不操作。
///
/// 文案逐条对齐 07 册 §3.2 的 quick 模板原文（无工具、无工作区访问、
/// 不假装做过、需要时建议切换模式），中文表述保持同强度。
const QUICK_CAPABILITY: &str = "## 能力边界（quick · 只答不操作）
- 本模式不启用任何工具：不读取文件、不写入文件、不执行命令、不访问网络、不操作浏览器。
- 只基于用户提供的信息与既有知识作答；不改动任何文件，不安排执行任务。
- 需要真实读写或执行时，请让用户切换到 ask、plan 或 craft 模式。";
