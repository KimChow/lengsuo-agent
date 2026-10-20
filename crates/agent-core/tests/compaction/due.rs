//! 压缩决策（`due()`）的验收：三条否定条件与触发条件。
//!
//! 逐条对照 opencode `compaction.ts:242-251`：刚压完不压、native checkpoint
//! 比 measured prompt 新则不压、窗口未知/自动压缩关掉不压、估算达到上限才压。

use agent_core::runtime::compaction::{Compaction, CompactionSettings, Trigger};
use agent_core::schema::{Message, Role};

use super::support::{assistant, checkpoint, compaction, user, CharEstimator, DEFAULT_KEEP};

/// 最后一条是 completed 压缩 ⇒ 不压（参考 `compaction.ts:246`）。
#[test]
fn due_is_none_when_last_message_is_a_completed_compaction() {
    let history = vec![
        user("原始需求"),
        assistant("回复"),
        checkpoint("## Objective\n- 摘要", ""),
    ];
    let comp = compaction(Some(1), DEFAULT_KEEP);
    assert_eq!(
        comp.due(&history),
        None,
        "刚压完 ⇒ 不再压（让运行方先按 checkpoint 重建请求）"
    );
}

/// 有 native checkpoint 且无 measured prompt ⇒ 不压（参考 `compaction.ts:248-249`）。
#[test]
fn due_is_none_with_native_checkpoint_and_no_measured_prompt() {
    // native checkpoint = 包裹段里**没有** `<summary>`（provider 原生窗口）。
    let native = Message {
        role: Role::User,
        content: "<conversation-checkpoint>\nprovider window\n</conversation-checkpoint>"
            .to_string(),
        tool: None,
        ts_ms: 0,
    };
    // native checkpoint 在最后一条 assistant（measured 锚）之后 ⇒ 测不准 ⇒ 不压。
    let history = vec![assistant("回复"), native];
    let comp = compaction(Some(1), DEFAULT_KEEP);
    assert_eq!(
        comp.due(&history),
        None,
        "native checkpoint 比 measured prompt 新 ⇒ 不压"
    );
}

/// 估算小于上限 ⇒ 不压；达到上限 ⇒ 压（参考 `compaction.ts:250`）。
#[test]
fn due_triggers_only_when_estimate_reaches_ceiling() {
    // 窗口 20 ⇒ 上限 18（20 - 10%）。可见文本含 `[User]: ` 标签 ⇒ 2 字符输入 = 10 token。
    let under = vec![user("12")]; // 10 token < 18
    assert_eq!(compaction(Some(20), DEFAULT_KEEP).due(&under), None);

    let over = vec![user(&"x".repeat(30))]; // 38 token >= 18
    assert_eq!(
        compaction(Some(20), DEFAULT_KEEP).due(&over),
        Some(Trigger::Auto),
        "估算达到上限 ⇒ 该压"
    );
}

/// 窗口未知 ⇒ 自动压缩永不触发（参考 `compaction.ts:222-225`）。
#[test]
fn due_is_none_when_window_is_unknown() {
    let history = vec![user(&"x".repeat(10_000))];
    assert_eq!(
        compaction(None, DEFAULT_KEEP).due(&history),
        None,
        "窗口未知 ⇒ 上限无穷 ⇒ 不压"
    );
}

/// 自动压缩关掉 ⇒ 不压（参考 `compaction.ts:210`）。
#[test]
fn due_is_none_when_auto_is_disabled() {
    let mut settings = CompactionSettings {
        auto: false,
        buffer: None,
        keep: DEFAULT_KEEP,
        window: Some(20),
    };
    let comp = Compaction::new(settings.clone(), Box::new(CharEstimator));
    let history = vec![user(&"x".repeat(100))];
    assert_eq!(comp.due(&history), None, "自动压缩关掉 ⇒ 不压");
    settings.auto = true;
    let comp = Compaction::new(settings, Box::new(CharEstimator));
    assert_eq!(comp.due(&history), Some(Trigger::Auto));
}
