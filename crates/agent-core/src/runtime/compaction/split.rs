//! 历史切分：老段进摘要、新段逐字保留（照抄 opencode `compaction.ts:708-744`）。
//!
//! 逐字对照（`packages/core/src/session/compaction.ts`）：
//! - [`split_conversation`] ← `:708-720`（`splitConversation`）；
//! - [`recent_start`] ← `:722-744`（`recentStart`：起点对齐 **user 边界**，
//!   绝不切在工具调用与工具结果之间）；
//! - [`oldest_to_drop`] ← `:746-756`（`oldestToDrop`：从最新端向旧端累加）；
//! - [`flatten_and_drop_oldest`] ← `:471-510`（`flattenAndDropOldest`：仍装不下
//!   ⇒ 从最旧**整段**丢 exchange，并留 `[N older exchanges omitted]` 注记）。
//!
//! **一处运行时差异（偏离清单）**：参考件「一条消息的文本形态」由
//! `messageToText`（`:759-811`）与 `flattenMessage`（`:829-849`）决定，依赖
//! opencode 的 `SessionMessage.Info`（含 parts / 媒体 / 工具状态）。本仓的
//! [`Message`]（`schema.rs:130`）是**纯文本**（`role` + `content` + `tool`），
//! 故此处按本仓角色映射为参考件的同名标签（`[User]:` / `[Assistant]:` /
//! `[Assistant tool call]:` / `[Tool result]:`），标签文字与参考件**逐字一致**。

use crate::schema::{Message, Role};

use super::is_checkpoint;

/// 工具输出在摘要里的最大字符数（照抄 `compaction.ts:100`）。
pub const TOOL_OUTPUT_MAX_CHARS: usize = 1_250;

/// 切分结果：`older` 进摘要，`recent` 逐字保留。
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationSplit {
    /// 进摘要的老段（消息原文）。
    pub older: Vec<Message>,
    /// 逐字保留的新段（渲染进 `<recent-context>`）。
    pub recent: String,
    /// 保留区起点（`older` = `messages[..split_at]`）。
    pub split_at: usize,
}

/// 压平结果：老段装不下时的文本形态 + 被丢的起点。
#[derive(Debug, Clone, PartialEq)]
pub struct Flattened {
    /// 压平后的文本（整段 exchange 拼接 + `[N older exchanges omitted]` 注记）。
    pub text: String,
    /// 第一个被丢弃的消息下标（相对传入的区间；未丢时 = 区间长度）。
    pub dropped_from: usize,
}

/// 一条消息在「保留区」里的文本形态（照抄 `messageToText`，`compaction.ts:759-811`）。
///
/// 压缩件不进保留区（参考件同：`case "compaction": return ""`）——摘要由摘要自身
/// 承载，不重复进 `<recent-context>`。
pub fn message_to_text(message: &Message) -> String {
    if is_checkpoint(message) {
        return String::new();
    }
    match message.role {
        Role::User => format!("[User]: {}", message.content),
        Role::Assistant => format!("[Assistant]: {}", message.content),
        Role::ToolCall => format!(
            "[Assistant tool call]: {}{}",
            message.content,
            tool_input(message)
        ),
        Role::ToolResult => format!("[Tool result]: {}", truncate_tool_output(&message.content)),
    }
}

/// 一条消息在「压平文本」里的形态（照抄 `flattenMessage`，`compaction.ts:829-849`）。
///
/// 与 [`message_to_text`] 的差别只在本仓没有媒体/推理 part；标签文字一致。
pub fn flatten_message(message: &Message) -> String {
    message_to_text(message)
}

/// 工具调用的入参片段（参考件 `flattenMessage` 的 `JSON.stringify(part.input)`）。
///
/// 本仓工具消息只留 `args_ref`（blob 引用），无内联入参 ⇒ 引用即全部可得信息。
fn tool_input(message: &Message) -> String {
    message
        .tool
        .as_ref()
        .and_then(|t| t.args_ref.as_deref())
        .map(|reference| format!("({reference})"))
        .unwrap_or_default()
}

/// 截断工具输出（照抄 `truncateToolOutput`，`compaction.ts:813-826`）。
///
/// 超 [`TOOL_OUTPUT_MAX_CHARS`] ⇒ 按**字符**（不是字节）截断并追加
/// `\n[truncated]`；按字符切避免把多字节字符切成乱码。
pub fn truncate_tool_output(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= TOOL_OUTPUT_MAX_CHARS {
        return value.to_string();
    }
    let kept: String = chars[..TOOL_OUTPUT_MAX_CHARS].iter().collect();
    format!("{kept}\n[truncated]")
}

/// 切分历史（照抄 `splitConversation`，`compaction.ts:708-720`）。
///
/// `keep` 是**保留区的 token 预算**（参考件默认 15000）。返回 `None` = 无可压缩
/// 内容（对应参考件的 `NOTHING_TO_COMPACT`）。
pub fn split_conversation(
    messages: &[Message],
    keep: usize,
    estimator: &dyn TokenEstimator,
) -> Option<ConversationSplit> {
    let entries: Vec<(usize, String)> = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let text = message_to_text(message);
            if text.is_empty() {
                None
            } else {
                Some((index, text))
            }
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    let start = recent_start(messages, &entries, keep, estimator);
    let split_at = entries
        .get(start)
        .map(|(index, _)| *index)
        .unwrap_or(messages.len());
    let recent = entries[start..]
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(ConversationSplit {
        older: messages[..split_at].to_vec(),
        recent,
        split_at,
    })
}

/// 保留区起点（照抄 `recentStart`，`compaction.ts:722-744`）。
///
/// 三步，对应参考件的三条分支：
/// 1. 从最旧起丢到剩余部分装进 `keep` 预算（但**永远保留最新一条**）；
/// 2. 起点回退到 **user 边界**（助手的消息与它的工具调用/结果成组，不拆散）；
/// 3. 全部装得下 ⇒ 只留最新一次交互，老段全部进摘要。
fn recent_start(
    messages: &[Message],
    entries: &[(usize, String)],
    keep: usize,
    estimator: &dyn TokenEstimator,
) -> usize {
    let sizes: Vec<usize> = entries
        .iter()
        .map(|(_, text)| estimator.estimate(text))
        .collect();
    // 丢到装得下为止，但永远保留最新一条（参考件 `min(oldestToDrop(…), len-1)`）。
    let dropped = oldest_to_drop(&sizes, keep).min(entries.len() - 1);

    // 起点落在 user 边界上：助手的消息与它的工具调用/结果同进退。
    let user_boundary = entries
        .iter()
        .enumerate()
        .take_while(|(index, _)| *index <= dropped)
        .filter(|(_, (message_index, _))| messages[*message_index].role == Role::User)
        .map(|(index, _)| index)
        .last();
    if let Some(boundary) = user_boundary {
        if boundary > 0 {
            return boundary;
        }
    }

    // 全装得下：只留最新一次交互，老段全部进摘要。
    // 注意：取「最后一条 user」⇒ 用 `filter` + `next_back`（`rfilter` 只在
    // `DoubleEndedIterator` 上可用，`Enumerate` 不满足该约束；`next_back` 是
    // `DoubleEndedIterator` 的 O(1) 末项取法，clippy 不会报 `last` 的整链遍历）。
    let latest_user = entries
        .iter()
        .enumerate()
        .filter(|(_, (message_index, _))| messages[*message_index].role == Role::User)
        .map(|(index, _)| index)
        .next_back();
    if let Some(latest) = latest_user {
        if latest > 0 {
            return latest;
        }
    }

    // 只有一次交互、没有更老的内容：全进摘要，保留区为空。
    entries.len()
}

/// 从最旧端丢弃到装进预算为止，返回需丢弃的条数（照抄 `oldestToDrop`，`compaction.ts:746-756`）。
///
/// 恰好等于预算仍保留，严格大于才停。
pub fn oldest_to_drop(sizes: &[usize], budget: usize) -> usize {
    let mut total = 0usize;
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

/// 压平后仍装不下 ⇒ 从最旧**整段**丢 exchange，并留注记（照抄 `flattenAndDropOldest`，
/// `compaction.ts:471-510`）。
///
/// 返回 `None` = 连最新一段都装不下（参考件同：`kept.length === 0 ⇒ undefined`）。
///
/// - `messages`：要压平的消息区间（一般是进摘要的老段）；
/// - `lead`：前一段 checkpoint 文本（若有），占预算但不可丢；
/// - `target`：剩余预算（token）。
pub fn flatten_and_drop_oldest(
    messages: &[Message],
    lead: &str,
    target: usize,
    estimator: &dyn TokenEstimator,
) -> Option<Flattened> {
    // 一个 exchange = 一条 user 消息及其后到下一条 user 之前的全部消息（整体留或整体丢）。
    let mut exchanges: Vec<String> = Vec::new();
    let mut exchange_starts: Vec<usize> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let text = flatten_message(message);
        if text.is_empty() {
            continue;
        }
        if message.role == Role::User || exchanges.is_empty() {
            exchanges.push(text);
            exchange_starts.push(index);
        } else {
            let last = exchanges.len() - 1;
            exchanges[last].push('\n');
            exchanges[last].push_str(&text);
        }
    }
    if exchanges.is_empty() {
        return None;
    }

    let note = |omitted: usize| {
        if omitted == 0 {
            String::new()
        } else {
            format!(
                "[{omitted} older {} omitted]\n\n",
                if omitted == 1 {
                    "exchange"
                } else {
                    "exchanges"
                }
            )
        }
    };

    // 选保留内容前先给最长的注记留位（参考件 `room` 的算法，`compaction.ts:501-504`）。
    let room = target
        .saturating_sub(estimator.estimate(lead))
        .saturating_sub(estimator.estimate(&note(exchanges.len())));
    let sizes: Vec<usize> = exchanges
        .iter()
        .map(|text| estimator.estimate(text) + 1)
        .collect();
    let dropped = oldest_to_drop(&sizes, room);
    let kept = &exchanges[dropped..];
    if kept.is_empty() {
        return None;
    }
    let dropped_from = if dropped < exchange_starts.len() {
        exchange_starts[dropped]
    } else {
        messages.len()
    };
    Some(Flattened {
        text: format!(
            "{}{}",
            note(exchanges.len() - kept.len()),
            kept.join("\n\n")
        ),
        dropped_from,
    })
}

/// token 估算口（照抄参考件 `Token.estimate` 的位置，本仓抽出成 trait）。
///
/// 参考件的 `Token.estimate`（`util/token.ts:5`）是 `round(len / 4)`；本仓默认实现
/// [`RoughEstimator`] 同口径（字符数 / 4）。
pub trait TokenEstimator {
    /// 估算一段文本的 token 数。
    fn estimate(&self, text: &str) -> usize;
}

/// 粗糙估算：字符数 / 4（与 opencode `util/token.ts:3-5` 同口径）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoughEstimator;

impl TokenEstimator for RoughEstimator {
    fn estimate(&self, text: &str) -> usize {
        // 按**字符**计（不是字节）：与参考件 `String.length` 的口径一致（UTF-16 码元，
        // 对 BMP 字符等于字符数）。
        let chars = text.chars().count();
        chars.div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rough_estimator_matches_reference_ceiling() {
        assert_eq!(RoughEstimator.estimate(""), 0);
        assert_eq!(RoughEstimator.estimate("a"), 1);
        assert_eq!(RoughEstimator.estimate("abcd"), 1);
        assert_eq!(RoughEstimator.estimate("abcde"), 2);
    }

    #[test]
    fn truncate_tool_output_counts_characters_not_bytes() {
        let long = "中".repeat(TOOL_OUTPUT_MAX_CHARS + 10);
        let cut = truncate_tool_output(&long);
        assert!(cut.ends_with("\n[truncated]"));
        assert_eq!(
            cut.chars().count(),
            TOOL_OUTPUT_MAX_CHARS + "\n[truncated]".chars().count()
        );
        let short = "x".repeat(10);
        assert_eq!(truncate_tool_output(&short), short);
    }
}
