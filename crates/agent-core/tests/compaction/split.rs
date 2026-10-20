//! 切分与压平的验收：切点边界、保留区预算、整段丢弃与工具输出截断。
//!
//! 逐条对照 opencode `compaction.ts:733-735`（`recentStart` 回退到 user 边界）、
//! `:498-499`（单复数注记）、`:813-826`（工具输出截断）。

use agent_core::runtime::compaction::split::{
    flatten_and_drop_oldest, message_to_text, oldest_to_drop, RoughEstimator, TokenEstimator,
};
use agent_core::runtime::compaction::Trigger;
use agent_core::schema::Role;

use super::support::{
    assistant, compaction, tool_call, tool_result, user, CharEstimator, DEFAULT_KEEP,
    TOOL_OUTPUT_MAX_CHARS,
};

/// 切点必须落在 **user 边界**上：不能切在工具调用与工具结果之间。
#[test]
fn split_lands_on_user_boundary_not_between_tool_call_and_result() {
    // 布局：user / assistant / tool_call / tool_result / user / assistant。
    let history = vec![
        user("需求一"),
        assistant("回复一"),
        tool_call("read", "读文件"),
        tool_result("文件内容"),
        user("需求二"),
        assistant("回复二"),
    ];
    let comp = compaction(Some(20_000), DEFAULT_KEEP);
    let plan = comp.plan(&history, Trigger::Auto).expect("可压缩");

    // 切点 index 处的消息必须是 user（助手消息与其工具调用/结果同进退）。
    let boundary = &history[plan.split_at];
    assert_eq!(
        boundary.role,
        Role::User,
        "切点必须落在 user 边界上（实际 role={:?}）",
        boundary.role
    );
    // 老段里不得出现「有调用无结果」的孤儿。
    let calls = plan
        .older
        .iter()
        .filter(|m| m.role == Role::ToolCall)
        .count();
    let results = plan
        .older
        .iter()
        .filter(|m| m.role == Role::ToolResult)
        .count();
    assert_eq!(calls, results, "老段内工具调用与结果必须成对");
}

/// 只有一次交互 ⇒ 全部进摘要，保留区为空（参考 `recentStart` 末条分支）。
#[test]
fn split_with_single_exchange_summarizes_everything() {
    let history = vec![user("需求"), assistant("回复")];
    let comp = compaction(Some(20_000), DEFAULT_KEEP);
    let plan = comp.plan(&history, Trigger::Auto).expect("可压缩");
    assert_eq!(plan.split_at, history.len(), "只有一次交互 ⇒ 全进摘要");
    assert!(plan.recent.is_empty(), "保留区应为空");
}

/// 保留区按 `keep` token 预算切：老段进摘要，新段逐字保留。
///
/// 注意参考件语义：预算按「条目数」选完后，起点**回退到 user 边界**
/// （`compaction.ts:733-735`），保留区因此可能略超 `keep`（边界内的一条 user
/// 连同其回复整体保留）。断言按此口径：不超 `keep + 一次交互`。
#[test]
fn split_keeps_recent_within_keep_budget() {
    // 每条 20 字符 = 20 token；keep = 60 ⇒ 预算保留 3 条，user 边界回退后 ≤ 4 条。
    let mut history = Vec::new();
    for k in 0..5 {
        history.push(user(&format!("u{k:02}{}", "x".repeat(18))));
        history.push(assistant(&format!("a{k:02}{}", "y".repeat(18))));
    }
    let comp = compaction(Some(20_000), 60);
    let plan = comp.plan(&history, Trigger::Auto).expect("可压缩");
    assert!(
        plan.split_at > 0,
        "老段必须非空（要进摘要），实际 split_at={}",
        plan.split_at
    );
    assert!(
        plan.split_at < history.len(),
        "保留区必须非空，实际 split_at={} len={}",
        plan.split_at,
        history.len()
    );
    // 切点必须落在 user 边界上（对齐参考件 `recentStart`）。
    assert_eq!(
        history[plan.split_at].role,
        Role::User,
        "切点必须落在 user 边界"
    );
    // 保留区 ≤ keep + 一次交互（user 边界回退至多多保一条 user + 其回复）。
    let recent_tokens = CharEstimator.estimate(&plan.recent);
    assert!(
        recent_tokens <= 60 + 40,
        "保留区必须 ≤ keep + 一次交互（实际 {recent_tokens}）"
    );
}

/// 仍装不下时从最旧丢**整个 exchange**，并留下 `[N older exchanges omitted]` 注记。
#[test]
fn flatten_drops_whole_exchanges_and_leaves_omission_note() {
    // 三个 exchange，每个 = user + assistant。
    let history = vec![
        user("e1-u"),
        assistant("e1-a"),
        user("e2-u"),
        assistant("e2-a"),
        user("e3-u"),
        assistant("e3-a"),
    ];
    // 预算：注记（27 字符）+ 最后一个 exchange（27 字符）⇒ 只够留最新一段。
    let flat = flatten_and_drop_oldest(&history, "", 60, &CharEstimator).expect("仍能保留最新一段");
    assert!(
        flat.text.starts_with("[2 older exchanges omitted]"),
        "必须留注记：{}",
        flat.text
    );
    assert!(flat.text.contains("e3-u"), "最新一段必须保留");
    assert!(!flat.text.contains("e1-u"), "最旧一段必须整段丢掉");
    assert!(!flat.text.contains("e2-u"), "次旧一段必须整段丢掉");
    assert_eq!(
        flat.dropped_from, 4,
        "丢弃起点 = 第 3 个 exchange 的消息下标"
    );
}

/// 丢一个 exchange 时注记用单数（参考 `compaction.ts:498-499`）。
#[test]
fn flatten_note_is_singular_for_one_dropped_exchange() {
    let history = vec![user("e1-u"), assistant("e1-a"), user("e2-u")];
    let flat = flatten_and_drop_oldest(&history, "", 50, &CharEstimator).expect("保留最新");
    assert!(
        flat.text.starts_with("[1 older exchange omitted]"),
        "单个 exchange 用单数：{}",
        flat.text
    );
}

/// 连最新一段都装不下 ⇒ `None`（参考 `kept.length === 0 ⇒ undefined`）。
#[test]
fn flatten_returns_none_when_nothing_fits() {
    let history = vec![user(&"很长的用户输入".repeat(10)), assistant("回复")];
    assert!(
        flatten_and_drop_oldest(&history, "", 1, &CharEstimator).is_none(),
        "预算装不下最新一段 ⇒ None"
    );
}

/// 工具输出超长时按 [`TOOL_OUTPUT_MAX_CHARS`] 截断（照抄 `compaction.ts:813-826`）。
#[test]
fn tool_output_is_truncated_in_recent_text() {
    let long = "x".repeat(TOOL_OUTPUT_MAX_CHARS + 100);
    let history = vec![user("需求"), tool_result(&long)];
    let comp = compaction(Some(20_000), DEFAULT_KEEP);
    let plan = comp.plan(&history, Trigger::Auto).expect("可压缩");
    // 工具结果若落在保留区，其文本必须带截断标记。
    let tool_text = message_to_text(&history[1]);
    assert!(
        tool_text.contains("\n[truncated]"),
        "工具输出必须截断并留标记：{}",
        tool_text
    );
    // 截断后长度 = 上限 + 标记。
    assert_eq!(
        tool_text.chars().count(),
        "[Tool result]: ".len() + TOOL_OUTPUT_MAX_CHARS + "\n[truncated]".len()
    );
    // 只有一次交互 ⇒ 全进摘要，保留区为空（参考 `recentStart` 末条分支）。
    assert!(plan.recent.is_empty(), "保留区应为空（只有一次交互）");
}

/// `oldestToDrop` 语义：恰好等于预算仍保留，严格大于才停。
#[test]
fn oldest_to_drop_keeps_exact_budget_and_stops_when_over() {
    assert_eq!(oldest_to_drop(&[10, 10], 20), 0, "恰好装下 ⇒ 不丢");
    assert_eq!(oldest_to_drop(&[10, 10], 19), 1, "装不下 ⇒ 丢最旧");
    assert_eq!(oldest_to_drop(&[10, 20], 100), 0, "全装得下 ⇒ 不丢");
}

/// 粗糙估算：字符数 / 4（与 opencode `util/token.ts:5` 同口径）。
#[test]
fn rough_estimator_is_chars_over_four() {
    assert_eq!(RoughEstimator.estimate("abcd"), 1);
    assert_eq!(RoughEstimator.estimate("abcde"), 2);
    assert_eq!(RoughEstimator.estimate(""), 0);
}
