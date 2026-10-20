//! quick 模式段集（07 册 §3.2 quick 模板全文 + §3.3 quick 片段清单）。
//!
//! quick 是**极简**模式：参考端只有 `current-mode` + `interaction` 两个片段，
//! 模板是 fail-closed 极简范本（07 册 §3.2），另配硬规则清单做双保险
//! （05 册 §2「quick 无工具但仍有硬规则」）。
//!
//! 本仓不挂 agent-loop、任务管理、文件分享与技能 / MCP / 专家管理段
//! （quick 无工具、不产出产物）；第 8 项 `<result_presentation>` 只保留
//! 「工具不阻塞正常回复」一条（见 `common::RESULT_PRESENTATION_QUICK`）。

use crate::model::{PromptMode, Segment};

use super::common::{contains, text, when};

/// quick 的段集（公共头 + 能力边界 + 公共层 + 回复语言 + 公共尾）。
pub(crate) fn quick_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::common::header(mode);
    // 05 §2 模式矩阵 quick 行：`tools: []`，定位纯对话。
    segments.push(text(super::common::QUICK_CAPABILITY));
    segments.extend(super::common::public_layer());
    // 07 §3.1 第 8 项 <result_presentation>（规格 §2 标必做，五模式都在）：
    // quick 无工具、不产出文件，只保留 18 §6 六条里「工具不阻塞回复」一条。
    segments.push(text(super::common::RESULT_PRESENTATION_QUICK));
    // 07 §3.2 quick 模板末条「Match the user's language」；
    // 条件段形态沿用 07 §5（'中文' in ResponseLanguage）。
    segments.push(when(
        contains("response_language", "中文"),
        vec![text(super::common::RESPONSE_LANGUAGE)],
    ));
    segments.extend(super::common::footer());
    segments
}
