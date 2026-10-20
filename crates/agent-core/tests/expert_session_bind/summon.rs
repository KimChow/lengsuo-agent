//! 召唤（§10.2 步骤 1 + 步骤 5）：设 `expert_id` ⇒ 强制 Expert，并写 `session_update`。

use serde_json::Value;

use agent_core::schema::Mode;

use super::support::{projects_root, runtime, session, workdir, write_expert, Capture};

#[test]
fn summon_sets_expert_and_forces_expert_mode_and_emits_event() {
    let ws = workdir("summon");
    write_expert(
        &projects_root(&ws),
        "alpha-expert",
        "ALPHA-PERSONA 人格正文",
    );
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);

    let after = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("alpha-expert")),
            None,
        )
        .expect("召唤成功");

    assert_eq!(after.expert_id.as_deref(), Some("alpha-expert"));
    assert_eq!(after.mode, Mode::Expert, "设 expert_id ⇒ 强制 Expert");
    // 事件：session_update，payload 带 expertId。
    let log = rt.event_log();
    let events = log.replay(s.id, 0, usize::MAX).expect("读事件");
    let last = events.last().expect("切换必须写一条 session_update");
    assert_eq!(last.kind, "session_update");
    assert_eq!(last.payload["expertId"], Value::from("alpha-expert"));
    assert_eq!(last.payload["mode"], Value::from("expert"));
}

#[test]
fn i2_session_identity_and_other_fields_survive_binding() {
    let ws = workdir("i2");
    write_expert(&projects_root(&ws), "alpha-expert", "ALPHA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);
    let after = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("alpha-expert")),
            None,
        )
        .expect("召唤");
    assert_eq!(after.id, s.id, "I2：会话标识稳定");
    assert_eq!(after.created_at_ms, s.created_at_ms);
    assert_eq!(after.permission_mode, s.permission_mode);
}
