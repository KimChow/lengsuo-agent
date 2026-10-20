//! 回合上下文组装（provider-context 卡）：Composer × 记忆 × 历史 × 本轮输入。
//!
//! Runtime 发起回合时不再只把用户输入送给 Provider，而是构造 [`TurnContext`]：
//! - `system_prompt`：`agent_prompts::Composer::build(模式, 变量)`，变量含身份层
//!   （AI 名/用户称呼/人格，agent-memory 身份层不受跨会话记忆开关限制）、工作区、
//!   确定性日期文本、以及**已启用**时的记忆摘要；
//! - `history`：会话已有消息（时间序），本轮输入在其后单独作为 `user_text`；
//!   进上下文过两道闸：先逐条收窄（`HISTORY_MESSAGE_CHARS`），再按总预算裁剪
//!   （`fit_history`：超 `HISTORY_TOTAL_CHARS` 从最旧整组丢弃，工具调用与结果
//!   同组同进退，最新一条永不丢；对齐 opencode 上下文重建/裁剪的确定性口径）；
//! - `memory_digest`：`memory.enabled` 关闭时为 `None`，Composer 不留空段。
//!
//! 隐私（AGENTS §6）：进入上下文的任何文本先过脱敏器；凭据不进模型输入。
//!
//! 记忆新鲜度：开启记忆的每个回合都重新组装摘要，不信任建会话时的 `session`
//! 快照（见 [`Runtime::turn_memory_digest`]），否则本会话内的显式记忆写入永不生效。

use std::ops::Range;

use super::*;
use crate::schema::Role;
use agent_prompts::{Composer, PromptMode, PromptVars};

/// 历史消息进上下文前的字符上限（防单条大输出顶爆模型输入；大结果走 result_ref）。
const HISTORY_MESSAGE_CHARS: usize = 2000;

/// 历史总预算（字符口径）：逐条收窄后仍超 ⇒ 从最旧整组丢弃（见 [`fit_history`]）。
const HISTORY_TOTAL_CHARS: usize = 24_000;

impl Runtime {
    /// 构造本轮上下文。`history` 是本轮输入推入之前的历史快照。
    pub(crate) fn build_turn_context(
        &self,
        session: &Session,
        history: &[Message],
        user_text: &str,
    ) -> TurnContext {
        let digest = self.turn_memory_digest(session);
        let vars = self.prompt_vars(session, digest.as_deref());
        let system_prompt = Composer::build(prompt_mode(session.mode), &vars);
        TurnContext {
            session_id: session.id,
            mode: session.mode,
            system_prompt,
            memory_digest: digest,
            history: fit_history(&narrow_history(history)),
            user_text: self.redact(user_text),
        }
    }

    /// 本轮记忆摘要：仅在 `memory.enabled` 时给出（关闭 ⇒ `None`，Composer 不留空段）。
    ///
    /// 新鲜度（memory-freshness 卡）：**每次开启的回合都按现存 MemoryEngine 重组**，
    /// 不采用建会话时留在 `session.memory_digest` 上的快照——否则本会话内通过既有
    /// 记忆写工具（`memory.set_user` / `memory.set_project` / `memory.append_daily`）
    /// 做的显式更新永远进不了模型上下文。会话快照只作为建会话时刻的热态展示用。
    ///
    /// 重组失败 ⇒ 本回合**不带任何摘要**（并落一条 `warning` 事件可告警），回合照常
    /// 跑完；尤其**不回退**到旧会话快照——失败时旧快照可能已过时且含敏感层内容，
    /// 宁可少注入也不注入陈旧记忆（AGENTS §6 最小上下文）。
    fn turn_memory_digest(&self, session: &Session) -> Option<String> {
        let enabled = self
            .config
            .lock()
            .map(|c| c.memory_enabled)
            .unwrap_or_default();
        if !enabled {
            return None;
        }
        match self.compose_memory_digest(&session.workspace_root) {
            Ok(digest) => Some(self.redact(&digest)),
            Err(e) => {
                // 告警本身失败不得改变回合结论（摘要已确定缺席）。
                let _ = self.emit(
                    session,
                    "warning",
                    serde_json::json!({
                        "kind": "memory_compose_failed",
                        "reason": e.to_string(),
                    }),
                );
                None
            }
        }
    }

    /// 模板变量：身份层 + 工作区 + 日期 + 用户规则（v0 空）+ 已启用记忆摘要。
    ///
    /// **临时兼容（h1-prompt-vars）**：`PromptVars` 已扩容为 22 字段、记忆拆为四段，
    /// 但本卡只做数据面。此处把现有单段摘要整体塞进 `memory_project`，其余记忆段留空；
    /// 产品/身份/风格/专家人格/语言/平台/功能开关/工具呈现/产物目录等新变量暂给默认值
    /// （字符串空、布尔 `false`）。**正式拆分与接线由 M3 / A7 卡完成**。
    fn prompt_vars(&self, session: &Session, digest: Option<&str>) -> PromptVars {
        let identity = self.read_identity(&session.workspace_root);
        PromptVars {
            ai_name: self.redact(&identity.ai_name),
            user_name: self.redact(&identity.user_name),
            soul: self.redact(&identity.soul),
            workspace: self.redact(&session.workspace_root),
            date: date_text(),
            user_rules: String::new(),
            memory_project: digest.unwrap_or_default().to_string(),
            ..PromptVars::default()
        }
    }

    /// 身份层读取（失败回退默认值：身份属产品人格，不阻断回合）。
    fn read_identity(&self, workspace_root: &str) -> agent_memory::Identity {
        use agent_memory::{MemoryEngine, MemoryPaths};
        let paths = MemoryPaths::new(self.workdir.join(".lengsuo"), workspace_root);
        MemoryEngine::new(paths, false)
            .read_identity()
            .unwrap_or_default()
    }
}

/// 会话模式 → 提示词模式（两侧枚举一一对应，不引入跨 crate 依赖方向变化）。
fn prompt_mode(mode: Mode) -> PromptMode {
    match mode {
        Mode::Quick => PromptMode::Quick,
        Mode::Ask => PromptMode::Ask,
        Mode::Plan => PromptMode::Plan,
        Mode::Craft => PromptMode::Craft,
        Mode::Expert => PromptMode::Expert,
    }
}

/// 历史进上下文：逐条收窄并脱敏，保持时间序与角色不变。
fn narrow_history(messages: &[Message]) -> Vec<Message> {
    let mut history = messages.to_vec();
    for message in &mut history {
        message.content = narrow(&redact_static(&message.content), HISTORY_MESSAGE_CHARS);
    }
    history
}

/// 第二道闸：总预算裁剪（对齐 opencode `compaction.ts` 的确定性丢弃核心）。
///
/// 输入应为已过第一道闸 [`narrow_history`] 的历史。超 [`HISTORY_TOTAL_CHARS`] ⇒
/// **从最旧整组丢弃**，直到保留后缀不超预算：
/// - 组 = 一段不可拆分的完整对话：连续 `ToolCall`/`ToolResult` 消息为一组
///   （调用与其结果同进退，绝不拆散），其余消息各自成组；
/// - 最新一组永不丢（对齐参考件 `recentStart` 的 `min(oldestToDrop(…), len-1)` 帽）：
///   即使它单独超预算也只保留第一道闸的逐条收窄形态；
/// - 允许裁到空（仅空输入），不伪造占位消息、不新增 Role。
///
/// 纯函数：不读时钟/环境变量、不依赖 Runtime 状态，同输入同输出。
fn fit_history(messages: &[Message]) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }
    let groups = tool_groups(messages);
    let sizes: Vec<usize> = groups.iter().map(|g| group_chars(messages, g)).collect();
    let dropped = oldest_to_drop(&sizes, HISTORY_TOTAL_CHARS).min(groups.len() - 1);
    messages[groups[dropped].start..].to_vec()
}

/// 历史分组：连续 `ToolCall`/`ToolResult` 消息为一个不可拆分组，其余每条自成一组。
///
/// 参考件以「user 边界」保证调用与结果同进退（`recentStart` 的 userBoundary /
/// `flattenAndDropOldest` 的 exchange）；本仓 ToolCall/ToolResult 是顶层消息，
/// 对应的不可拆分单位即相邻工具消息段（判定依据见卡面）。
fn tool_groups(messages: &[Message]) -> Vec<Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    for i in 1..=messages.len() {
        let splits = i == messages.len()
            || !(is_tool_message(&messages[i - 1]) && is_tool_message(&messages[i]));
        if splits {
            groups.push(start..i);
            start = i;
        }
    }
    groups
}

fn is_tool_message(message: &Message) -> bool {
    matches!(message.role, Role::ToolCall | Role::ToolResult)
}

/// 组的字符量（口径与逐条收窄一致：`content` 字符数求和）。
fn group_chars(messages: &[Message], group: &Range<usize>) -> usize {
    messages[group.start..group.end]
        .iter()
        .map(|m| m.content.chars().count())
        .sum()
}

/// 逐字对齐 opencode `compaction.ts` 的 `oldestToDrop`：从最新端向旧端累加尺寸，
/// 返回需从最旧端丢弃的组数，使保留后缀的总字符 ≤ 预算（恰好等于预算仍保留，
/// 严格大于才停）。组内顺序、组间时间序均不变。
fn oldest_to_drop(sizes: &[usize], budget: usize) -> usize {
    let mut total = 0;
    let mut start = sizes.len();
    while start > 0 {
        let next = total + sizes[start - 1];
        if next > budget {
            break;
        }
        total = next;
        start -= 1;
    }
    start
}

/// 无 Runtime 句柄处的脱敏（历史收窄用同一 v0 规则集）。
fn redact_static(text: &str) -> String {
    Redactor::default_v0().redact_str(text)
}

/// 确定性日期文本（本地日期；不引入时刻，保证同一天内同输入同输出）。
fn date_text() -> String {
    chrono::Local::now().date_naive().to_string()
}
