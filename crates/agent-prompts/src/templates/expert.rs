//! expert 模式段集（07 册 §3.3 expert 六片段 + 21 册 §5「人格是纯变量注入点」）。
//!
//! expert 复用 craft 的全部段（21 册 §3「craft/plan/expert 共用同一份」），
//! 差异是**专家人格注入**（`plugin_agent_prompt` 变量，21 册 §5「current-mode
//! 片段是空壳」）与专家协作纪律。本仓把人格做成条件段：变量非空才渲染。

use crate::model::{PromptMode, Segment};

use super::common::{if_filled, text};

/// expert 的段集（craft 段 + 专家人格段 + 专家协作段）。
///
/// 专家人格段与专家协作段都排在能力段之后、硬规则之前。
pub(crate) fn expert_segments(mode: PromptMode) -> Vec<Segment> {
    let mut segments = super::craft::craft_segments(mode);
    let tail = segments.split_off(segments.len() - 2);
    // 21 §5：专家人格是纯变量注入点，current-mode 片段是空壳 ⇒ 人格段按变量
    // 非空才渲染（条件段）。
    segments.push(if_filled(
        &["plugin_agent_prompt"],
        vec![text(super::common::EXPERT_PERSONA)],
    ));
    segments.push(text(super::common::EXPERT_COLLABORATION));
    segments.extend(tail);
    segments
}
