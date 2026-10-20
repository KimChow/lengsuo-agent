//! 切换 / 清空（§10.2 步骤 1–4）：Expert 保持、清空回落 Craft、I1 历史、人格段重写。

use serde_json::Value;

use agent_core::schema::Mode;

use super::support::{projects_root, runtime, session, turn, workdir, write_expert, Capture};

#[test]
fn switch_to_another_expert_keeps_expert_mode() {
    let ws = workdir("switch");
    let root = projects_root(&ws);
    write_expert(&root, "alpha-expert", "ALPHA-PERSONA");
    write_expert(&root, "beta-expert", "BETA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);

    let a = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("alpha-expert")),
            None,
        )
        .expect("召唤 A");
    let b = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("beta-expert")),
            None,
        )
        .expect("切到 B");
    assert_eq!(a.expert_id.as_deref(), Some("alpha-expert"));
    assert_eq!(b.expert_id.as_deref(), Some("beta-expert"));
    assert_eq!(b.mode, Mode::Expert, "切换专家仍处 Expert");
}

#[test]
fn clearing_expert_falls_back_to_craft_mode() {
    let ws = workdir("clear");
    write_expert(&projects_root(&ws), "alpha-expert", "ALPHA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);
    rt.update_session(
        s.id,
        None,
        None,
        None,
        None,
        Some(Some("alpha-expert")),
        None,
    )
    .expect("召唤");

    let back = rt
        .update_session(s.id, None, None, None, None, Some(None), None)
        .expect("切回 craft");
    assert_eq!(back.expert_id, None);
    assert_eq!(back.mode, Mode::Craft, "清 expert_id ⇒ 回落 Craft");

    let log = rt.event_log();
    let last = log.replay(s.id, 0, usize::MAX).expect("读事件").pop();
    assert_eq!(last.expect("事件").payload["expertId"], Value::Null);
}

#[test]
fn i1_three_switches_keep_every_message_intact() {
    let ws = workdir("i1");
    let root = projects_root(&ws);
    write_expert(&root, "alpha-expert", "ALPHA-PERSONA");
    write_expert(&root, "beta-expert", "BETA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);
    rt.send_text(s.id, "第一句话").expect("turn 1");
    rt.send_text(s.id, "第二句话").expect("turn 2");
    let before = rt.get_session(s.id).expect("会话").messages;
    assert_eq!(before.len(), 4, "两轮 user/assistant");

    for id in ["alpha-expert", "beta-expert", ""] {
        let bound = if id.is_empty() { None } else { Some(id) };
        rt.update_session(s.id, None, None, None, None, Some(bound), None)
            .expect("切换");
    }

    let after = rt.get_session(s.id).expect("会话").messages;
    assert_eq!(after.len(), before.len(), "I1：切换不丢历史（条数）");
    for (new, old) in after.iter().zip(before.iter()) {
        assert_eq!(new, old, "I1：切换不丢历史（逐条内容/时间戳）");
    }
}

#[test]
fn switch_rewrites_persona_segment_only_after_the_next_turn() {
    let ws = workdir("persona");
    let root = projects_root(&ws);
    write_expert(&root, "alpha-expert", "ALPHA-PERSONA 负责 alpha");
    write_expert(&root, "beta-expert", "BETA-PERSONA 负责 beta");
    let cap = Capture::new();
    let rt = runtime(&ws, cap.dyn_capture());
    let s = session(&rt);
    let plain = turn(&rt, &cap, s.id, "普通一轮");
    assert!(!plain.contains("PERSONA"), "未召唤时不得有人格段面");

    rt.update_session(
        s.id,
        None,
        None,
        None,
        None,
        Some(Some("alpha-expert")),
        None,
    )
    .expect("召唤 A");
    let first = turn(&rt, &cap, s.id, "A 的一轮");
    assert!(
        first.contains("ALPHA-PERSONA"),
        "system 须含 A 的人格:\n{first}"
    );
    assert!(first.contains("# 系统提示词 · 模式：expert"));

    rt.update_session(
        s.id,
        None,
        None,
        None,
        None,
        Some(Some("beta-expert")),
        None,
    )
    .expect("切到 B");
    let second = turn(&rt, &cap, s.id, "B 的一轮");
    assert!(second.contains("BETA-PERSONA"), "system 须含 B 的人格");
    assert!(!second.contains("ALPHA-PERSONA"), "不得残留 A 的人格");
}
