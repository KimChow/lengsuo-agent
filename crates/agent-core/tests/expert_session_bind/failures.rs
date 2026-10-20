//! 失败语义（§11.2）：I3 召唤失败不写字段、可重试；I4 会话间互不影响；组装期降级。

use std::fs;

use serde_json::Value;
use uuid::Uuid;

use agent_core::error::CoreError;
use agent_core::runtime::Runtime;
use agent_core::schema::{Mode, SessionStatus};

use super::support::{
    count, projects_root, runtime, session, turn, workdir, write_broken_expert, write_expert,
    Capture,
};

#[test]
fn unrelated_broken_package_does_not_block_summon_or_hide_not_found() {
    let ws = workdir("unrelated");
    let root = projects_root(&ws);
    write_expert(&root, "alpha-expert", "ALPHA-PERSONA");
    // 同根下另一个包坏掉（无关）：不得让 alpha 召唤失败，也不得把「不存在」变成坏包错误。
    write_broken_expert(&root, "aaa-broken");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);

    let ok = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("alpha-expert")),
            None,
        )
        .expect("无关坏包不影响召唤");
    assert_eq!(ok.expert_id.as_deref(), Some("alpha-expert"));

    let err = rt
        .update_session(s.id, None, None, None, None, Some(Some("no-such")), None)
        .expect_err("不存在的专家仍报不存在");
    assert_eq!(err.code(), Some("EXPERT_NOT_FOUND"));

    // 召唤坏包本身 ⇒ 包错误码，且不改会话（I3）。
    let err = rt
        .update_session(s.id, None, None, None, None, Some(Some("aaa-broken")), None)
        .expect_err("坏包召唤失败");
    assert_eq!(err.code(), Some("MANIFEST_PARSE_FAILED"));
    assert_eq!(
        rt.get_session(s.id).expect("会话").expert_id.as_deref(),
        Some("alpha-expert")
    );
}

#[test]
fn r11_1_missing_expert_leaves_session_untouched_and_retryable() {
    let ws = workdir("i3");
    write_expert(&projects_root(&ws), "alpha-expert", "ALPHA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);
    rt.send_text(s.id, "此前已有一轮").expect("turn");
    let before = rt.get_session(s.id).expect("会话");
    let events_before = rt.event_log().last_seq(s.id).expect("seq");

    let err = rt
        .update_session(s.id, None, None, None, None, Some(Some("no-such")), None)
        .expect_err("不存在的专家必须失败");
    match &err {
        CoreError::ExpertUnavailable {
            code, expert_id, ..
        } => {
            assert_eq!(*code, "EXPERT_NOT_FOUND");
            assert_eq!(expert_id, "no-such");
        }
        other => panic!("错误码应区分「专家不存在」: {other:?}"),
    }
    let after = rt.get_session(s.id).expect("会话");
    assert_eq!(after.expert_id, None, "I3：不落半个专家");
    assert_eq!(after.mode, before.mode, "I3：mode 不变");
    assert_eq!(after.messages, before.messages, "I3：messages 不变");
    assert_eq!(after.title, before.title, "I3：title 不变");
    assert_eq!(
        after.updated_at_ms, before.updated_at_ms,
        "I3：失败不触碰会话"
    );
    assert_eq!(
        rt.event_log().last_seq(s.id).expect("seq"),
        events_before,
        "I3：失败不写切换事件"
    );

    // 可重试：修正 id 后同一会话召唤成功。
    let ok = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("alpha-expert")),
            None,
        )
        .expect("重试成功");
    assert_eq!(ok.expert_id.as_deref(), Some("alpha-expert"));
}

#[test]
fn i3_broken_expert_reports_validation_failure_and_keeps_session() {
    let ws = workdir("broken");
    let dir = write_expert(&projects_root(&ws), "broken-expert", "PERSONA");
    // tags 只剩 2 个 ⇒ §17 #10 TAG_COUNT（Error 级）⇒ 校验不过。
    let manifest: Value =
        serde_json::from_str(&super::support::plugin_json_for_test("broken-expert"))
            .expect("manifest 是 JSON");
    let mut manifest = manifest;
    manifest["tags"] = serde_json::json!([{ "zh": "唯一", "en": "Only" }]);
    fs::write(
        dir.join(".lengsuo-plugin/plugin.json"),
        serde_json::to_string(&manifest).expect("序列化"),
    )
    .expect("写坏包");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let s = session(&rt);

    let err = rt
        .update_session(
            s.id,
            None,
            None,
            None,
            None,
            Some(Some("broken-expert")),
            None,
        )
        .expect_err("损坏包必须召唤失败");
    match &err {
        CoreError::ExpertUnavailable { code, .. } => assert_eq!(*code, "TAG_COUNT"),
        other => panic!("错误码应区分「包校验失败」: {other:?}"),
    }
    let after = rt.get_session(s.id).expect("会话");
    assert_eq!(after.expert_id, None);
    assert_eq!(after.mode, Mode::Craft);
}

fn stop_and_fail(rt: &Runtime, id: Uuid) {
    rt.stop_session(id).expect("stop");
    rt.fail_session(id).expect("fail");
}

fn finish(rt: &Runtime, id: Uuid) {
    rt.finish_session(id).expect("finish");
}

#[test]
fn i4_lifecycle_of_one_session_never_touches_another() {
    let ws = workdir("i4");
    let root = projects_root(&ws);
    write_expert(&root, "alpha-expert", "ALPHA-PERSONA");
    write_expert(&root, "beta-expert", "BETA-PERSONA");
    let rt = runtime(&ws, Capture::new().dyn_capture());
    let a = session(&rt);
    let b = session(&rt);
    rt.update_session(
        a.id,
        None,
        None,
        None,
        None,
        Some(Some("alpha-expert")),
        None,
    )
    .expect("召唤 A");
    rt.update_session(
        b.id,
        None,
        None,
        None,
        None,
        Some(Some("beta-expert")),
        None,
    )
    .expect("召唤 B");
    rt.send_text(b.id, "B 的历史").expect("turn B");
    let b_before = rt.get_session(b.id).expect("B");

    // 结束（闭合并拒绝新输入）A 会话：只作用于 A。
    finish(&rt, a.id);
    stop_and_fail(&rt, a.id);

    let b_after = rt.get_session(b.id).expect("B");
    assert_eq!(b_after.expert_id.as_deref(), Some("beta-expert"), "I4");
    assert_eq!(b_after.mode, Mode::Expert, "I4");
    assert_eq!(b_after.messages, b_before.messages, "I4");
    assert_eq!(b_after.status, SessionStatus::Idle, "I4：B 状态不受 A 影响");
    assert_eq!(
        rt.get_session(a.id).expect("A").expert_id.as_deref(),
        Some("alpha-expert"),
        "A 自己的绑定也不因结束而丢"
    );
}

#[test]
fn legacy_injection_slot_carries_persona_and_missing_package_degrades() {
    let ws = workdir("legacy");
    let dir = write_expert(&projects_root(&ws), "alpha-expert", "ALPHA-PERSONA 正文");
    let cap = Capture::new();
    let rt = runtime(&ws, cap.dyn_capture());
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
    let prompt = turn(&rt, &cap, s.id, "一轮");
    assert!(
        prompt.contains("## 专家人格\nALPHA-PERSONA 正文"),
        "既有注入点须承载人格正文:\n{prompt}"
    );

    // 人格**只注入一处**：既有 `plugin_agent_prompt` 槽（`## 专家人格`）。专用槽
    // `expert_persona`（`## 专家角色设定（专家包）`）保持空 —— 两处同填会重复渲染。
    assert_eq!(
        count(&prompt, "ALPHA-PERSONA 正文"),
        1,
        "人格正文只出现一次"
    );
    assert!(!prompt.contains("## 专家角色设定"), "专用槽不填");

    // 组装期包被删 ⇒ 降级为不注入人格段，expert_id 保持召唤时校验过的状态。
    fs::remove_dir_all(&dir).expect("删包");
    let seq_before = rt.event_log().last_seq(s.id).expect("seq");
    let degraded = turn(&rt, &cap, s.id, "再来一轮");
    assert!(!degraded.contains("ALPHA-PERSONA"), "缺包不注入人格");
    assert!(!degraded.contains("## 专家人格"), "空值不留标题");
    // 降级可观测：一条 warning（不阻断回合、不改会话）。
    let warnings: Vec<_> = rt
        .event_log()
        .replay(s.id, seq_before, usize::MAX)
        .expect("读事件")
        .into_iter()
        .filter(|e| e.kind == "warning")
        .collect();
    assert_eq!(warnings.len(), 1, "降级留一条 warning");
    assert_eq!(warnings[0].payload["kind"], "expert_persona_unavailable");
    assert_eq!(warnings[0].payload["expertId"], "alpha-expert");
    let kept = rt.get_session(s.id).expect("会话");
    assert_eq!(kept.expert_id.as_deref(), Some("alpha-expert"));
    assert_eq!(kept.mode, Mode::Expert);
}
