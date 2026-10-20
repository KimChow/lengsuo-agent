//! 压缩产物与运行时接线的验收：`apply`、历史边界、落库与事件回放。
//!
//! 逐条对照 opencode `to-llm-message.ts:311-325`（checkpoint 渲染）与
//! `history.ts:28`（`latestCompaction` 历史边界）。

use agent_core::runtime::compaction::{render_checkpoint, Trigger};
use agent_core::schema::Role;

use super::support::{assistant, checkpoint, compaction, user, DEFAULT_KEEP};

/// 摘要 + 保留区拼成新历史，渲染含 `<conversation-checkpoint><summary>` 与
/// `<recent-context>`。
#[test]
fn apply_builds_checkpoint_history() {
    let history = vec![user("需求一"), assistant("回复一"), user("需求二")];
    let comp = compaction(Some(20_000), DEFAULT_KEEP);
    let plan = comp.plan(&history, Trigger::Auto).expect("可压缩");
    let new_history = comp.apply(&plan, "## Objective\n- 做事");

    assert_eq!(new_history.len(), 1, "新历史只含一条 checkpoint");
    let checkpoint = &new_history[0];
    assert!(
        checkpoint.content.contains("<conversation-checkpoint>"),
        "产物必须含开标签：{}",
        checkpoint.content
    );
    assert!(
        checkpoint
            .content
            .contains("<summary>\n## Objective\n- 做事\n</summary>"),
        "产物必须含摘要段：{}",
        checkpoint.content
    );
    assert!(
        checkpoint.content.contains("<recent-context>"),
        "产物必须含保留区段：{}",
        checkpoint.content
    );
    assert!(
        checkpoint.content.ends_with("</conversation-checkpoint>"),
        "产物必须以闭标签收尾：{}",
        checkpoint.content
    );
    assert_eq!(
        checkpoint.role,
        Role::User,
        "参考件把 completed 压缩渲染成 user 消息（to-llm-message.ts:311-325）"
    );
    // 保留区内容逐字保留。
    assert!(
        checkpoint.content.contains("需求二"),
        "保留区必须逐字保留最近内容：{}",
        checkpoint.content
    );
}

/// 历史边界：只从最近一次 completed 压缩开始读（参考 `history.ts:28`）。
#[test]
fn history_boundary_starts_at_latest_checkpoint() {
    use agent_core::runtime::compaction::history_from_latest_checkpoint;
    let history = vec![
        user("老消息"),
        checkpoint("摘要一", ""),
        user("新消息"),
        checkpoint("摘要二", ""),
        user("更新消息"),
    ];
    let bounded = history_from_latest_checkpoint(&history);
    assert_eq!(bounded.len(), 2, "边界 = 最近一次 checkpoint 起到末尾");
    assert!(bounded[0].content.contains("摘要二"));
    assert!(bounded[1].content.contains("更新消息"));
}

/// 压缩产物落进会话历史（`Runtime::apply_compaction`），且**逐字**保留：
/// 不被收窄、不被兜底裁剪丢掉。
#[test]
fn runtime_lands_compaction_in_history_verbatim() {
    use agent_core::runtime::{CreateSession, Runtime};
    use agent_core::schema::Isolation;

    let dir = std::env::temp_dir().join(format!("compaction-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(std::sync::Arc::new(agent_core::provider::EchoProvider))
        .build()
        .expect("runtime");
    let session = rt
        .create_session(&CreateSession::plain("压缩卡", Isolation::local_default()))
        .expect("create session");

    // 一条超长的摘要（远超 `HISTORY_MESSAGE_CHARS` = 2000 与总预算 24_000）。
    let summary = format!("## Objective\n- {}\n", "长".repeat(30_000));
    let checkpoint = render_checkpoint(&summary, "[User]: 最近输入");
    rt.apply_compaction(session.id, &checkpoint)
        .expect("apply compaction");

    let stored = rt.get_session(session.id).expect("session").messages;
    assert_eq!(stored.len(), 1, "压缩产物落一条消息");
    assert_eq!(stored[0].role, Role::User, "参考件渲染为 user 消息");
    assert_eq!(
        stored[0].content, checkpoint,
        "checkpoint 必须逐字保留（不被收窄、不被截断）"
    );
    assert!(
        stored[0].content.contains("<conversation-checkpoint>"),
        "内容是包裹段"
    );
}

/// 落库后重启：checkpoint 经事件回放**原样重建**（durable 事件是权威）。
#[test]
fn checkpoint_survives_restart_replay() {
    use agent_core::runtime::{CreateSession, Runtime};
    use agent_core::schema::Isolation;

    let dir = std::env::temp_dir().join(format!("compaction-restart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let checkpoint = render_checkpoint("## Objective\n- 摘要", "[User]: 最近");
    let session_id = {
        let rt = Runtime::builder()
            .workdir(&dir)
            .provider(std::sync::Arc::new(agent_core::provider::EchoProvider))
            .build()
            .expect("runtime");
        let session = rt
            .create_session(&CreateSession::plain("压缩卡", Isolation::local_default()))
            .expect("create session");
        rt.apply_compaction(session.id, &checkpoint)
            .expect("apply compaction");
        session.id
    };
    // 新 Runtime（同一 workdir）⇒ 启动回放重建历史。
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(std::sync::Arc::new(agent_core::provider::EchoProvider))
        .build()
        .expect("runtime");
    let rebuilt = rt.get_session(session_id).expect("session").messages;
    assert_eq!(rebuilt.len(), 1, "回放必须重建出 checkpoint 消息");
    assert_eq!(
        rebuilt[0].content, checkpoint,
        "回放后的 checkpoint 必须逐字相同"
    );
}
