//! 事件重放协议类型（19 册 §4 重放机制 + 27 册 §1 回放族 `_meta` 键）。
//!
//! 断线恢复的完整机制（19 册 §4，SessionManager 实证语义，类型层承载其
//! 结构约束、不做水合动作）：
//! - 重放顺序**先 history 后 live**（`hydrateSessionReplayPayload(s, e,
//!   'history'|'live')` 的调用顺序），由 [`ReplayPhase`] 逐事件标注；
//! - 历史存储有代际（`historySessionGenerations` / `historyStoreGeneration`），
//!   存储失效触发 `history_invalidated` 强制重新水合——代际属 daemon 内部
//!   状态，无 wire 字段实证，未建型；
//! - 重放后若任务态「回退到终态」按陈旧覆盖防护处理——同为内部行为边界；
//! - 事件流携带回放安全标记（27 册 §1 规则 A27-1）：`promptReplaySafe`
//!   标记哪些事件可安全重放、`isCompactInternal` 标记压缩内部产物，
//!   见 [`ReplaySafetyMeta`]。
//!
//! 27/19/18 册未实证重放的 wire 方法名与逐字段报文；本模块按卡片要求建
//! 「锚点 + 上限」的请求/结果类型，缺口见任务报告偏离清单。

use serde::{Deserialize, Serialize};

use crate::methods::{SessionId, SessionUpdate};

/// 重放请求参数：按锚点游标拉取会话事件历史。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEventsParams {
    /// 目标会话（19 册 §4：重放以会话为单位，replaceSessionReplay(session,…)）。
    pub session_id: SessionId,
    /// 重放锚点（不透明 cursor；缺省从头重放）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// 单次回放事件条数上限。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// 重放响应结果：事件按「先 history 后 live」原序回传。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEventsResult {
    /// 重放事件序列（顺序约束见 [`ReplayPhase`]）。
    pub events: Vec<ReplayEvent>,
    /// 后续锚点（无则已重放到底）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// 重放事件条目：`session/update` 同形载荷 + 水合阶段 + 回放安全标记。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEvent {
    /// 所属会话。
    pub session_id: SessionId,
    /// 事件本体（判别式 `sessionUpdate`，19 册 §2 事件目录）。
    pub update: SessionUpdate,
    /// 水合阶段（19 册 §4 第三参取值域；history 恒先于 live）。
    pub phase: ReplayPhase,
    /// 回放安全标记（27 册 §1 回放族键；wire 键 `_meta`）。
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<ReplaySafetyMeta>,
}

/// 水合阶段（19 册 §4 `hydrateSessionReplayPayload` 的 `'history' | 'live'`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPhase {
    /// 历史事件（先水合）。
    History,
    /// 实时事件（后水合）。
    Live,
}

/// 事件回放安全标记（27 册 §1 回放族全三键，wire 键带 `codebuddy.ai/`
/// 前缀**原样命名**；规则 A27-1）。
///
/// 挂在事件元数据 `_meta` 上：`promptReplaySafe` 标记哪些事件可安全重放、
/// `isCompactInternal` 标记哪些是压缩内部产物；三键均可选缺省。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReplaySafetyMeta {
    /// `codebuddy.ai/historyReplay`：历史重放产物标记（回放族第三键）。
    #[serde(
        default,
        rename = "codebuddy.ai/historyReplay",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_replay: Option<bool>,
    /// `codebuddy.ai/promptReplaySafe`：可安全重放标记。
    #[serde(
        default,
        rename = "codebuddy.ai/promptReplaySafe",
        skip_serializing_if = "Option::is_none"
    )]
    pub prompt_replay_safe: Option<bool>,
    /// `codebuddy.ai/isCompactInternal`：压缩内部产物标记。
    #[serde(
        default,
        rename = "codebuddy.ai/isCompactInternal",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_compact_internal: Option<bool>,
}
