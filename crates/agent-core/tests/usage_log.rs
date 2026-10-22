//! token 记账入口（x-token-usage 批 2）：`model.usage` 事件落点、payload 口径、
//! 复用 `TokenUsage::billed_tokens()`、usage 缺失不记账、可 replay。
//!
//! 直接面：`record_usage(&dyn EventLog, …)`；接线面：`Runtime::send_text` 在
//! provider 调用后经 `Provider::last_usage()` 取用量落事件。

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::error::CoreError;
use agent_core::event_log::{EventLog, JsonlEventLog};
use agent_core::provider::{Provider, ProviderStep, TokenUsage};
use agent_core::runtime::usage_log::{record_usage, MODEL_USAGE_EVENT};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::Isolation;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("usage-log-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

/// 卡面样本：208 输入（195 命中缓存）+ 11 输出 ⇒ 计费 13 + 11 = 24。
fn sample() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 208,
        completion_tokens: 11,
        total_tokens: 219,
        cached_tokens: 195,
    }
}

#[test]
fn record_usage_writes_exactly_one_model_usage_event() {
    let log = JsonlEventLog::open(workdir("one")).expect("open log");
    let session = uuid::Uuid::new_v4();

    let event = record_usage(&log, session, sample()).expect("record usage");

    assert_eq!(event.kind, MODEL_USAGE_EVENT);
    assert_eq!(event.kind, "model.usage");
    assert_eq!(event.session_id, session);
    assert_eq!(event.seq, 1, "首条事件 seq 从 1 起");
    let events = log.replay(session, 0, 100).expect("replay");
    assert_eq!(events.len(), 1, "一次调用只落一条事件");
    assert_eq!(events[0].kind, "model.usage");
}

#[test]
fn payload_carries_all_five_keys_matching_the_usage() {
    let log = JsonlEventLog::open(workdir("keys")).expect("open log");
    let session = uuid::Uuid::new_v4();
    let usage = sample();

    let event = record_usage(&log, session, usage).expect("record usage");
    let payload = &event.payload;

    for key in [
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "cached_tokens",
        "billed_tokens",
    ] {
        assert!(payload.get(key).is_some(), "payload 缺键 {key}: {payload}");
    }
    assert_eq!(payload["prompt_tokens"], usage.prompt_tokens);
    assert_eq!(payload["completion_tokens"], usage.completion_tokens);
    assert_eq!(payload["total_tokens"], usage.total_tokens);
    assert_eq!(payload["cached_tokens"], usage.cached_tokens);
}

#[test]
fn billed_tokens_reuses_the_token_usage_caliber() {
    let log = JsonlEventLog::open(workdir("billed")).expect("open log");
    let session = uuid::Uuid::new_v4();
    let usage = sample();

    let event = record_usage(&log, session, usage).expect("record usage");

    assert_eq!(event.payload["billed_tokens"], 24, "(208 − 195) + 11 = 24");
    assert_eq!(
        event.payload["billed_tokens"],
        usage.billed_tokens(),
        "payload 必须复用 TokenUsage::billed_tokens()，不得另算"
    );
}

#[test]
fn missing_usage_writes_no_event_and_no_zero_record() {
    // EchoProvider 不覆写 last_usage ⇒ trait 默认 None ⇒ 不记账。
    let dir = workdir("missing");
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(agent_core::provider::EchoProvider))
        .build()
        .expect("runtime");
    let session = rt
        .create_session(&CreateSession::plain(
            "missing-usage",
            Isolation::local_default(),
        ))
        .expect("session");

    rt.send_text(session.id, "没有 usage").expect("turn");
    let events = rt.event_log().replay(session.id, 0, 1000).expect("replay");

    assert!(
        events.iter().all(|event| event.kind != MODEL_USAGE_EVENT),
        "usage 缺失时不得写全 0 假记录: {:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );
}

#[test]
fn usage_event_survives_replay() {
    let dir = workdir("replay");
    let session = uuid::Uuid::new_v4();
    let usage = sample();
    let written = {
        let log = JsonlEventLog::open(dir.clone()).expect("open log");
        record_usage(&log, session, usage).expect("record usage")
    };

    let reloaded = JsonlEventLog::open(dir).expect("reopen replays");
    let events = reloaded.replay(session, 0, 100).expect("replay");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, written.seq, "seq 必须正常");
    assert_eq!(events[0].kind, "model.usage");
    assert_eq!(events[0].payload, written.payload, "payload 必须完整");
}

/// 带用量的 provider：每步报告同一个 `TokenUsage`（接线测试用）。
struct Metered {
    usage: TokenUsage,
}

impl Provider for Metered {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Ok(vec![
            ProviderStep::Text(user_text.to_string()),
            ProviderStep::Finished,
        ])
    }

    fn last_usage(&self) -> Option<TokenUsage> {
        Some(self.usage)
    }
}

#[test]
fn runtime_turn_records_usage_from_provider() {
    let dir = workdir("wire");
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(Metered { usage: sample() }))
        .build()
        .expect("runtime");
    let session = rt
        .create_session(&CreateSession::plain("wire", Isolation::local_default()))
        .expect("session");

    rt.send_text(session.id, "记一笔").expect("turn");

    let events = rt.event_log().replay(session.id, 0, 1000).expect("replay");
    let usage_events: Vec<_> = events
        .iter()
        .filter(|event| event.kind == MODEL_USAGE_EVENT)
        .collect();
    assert_eq!(
        usage_events.len(),
        1,
        "一次 provider 调用 = 一条 usage 事件"
    );
    assert_eq!(usage_events[0].payload["billed_tokens"], 24);
    assert!(
        usage_events[0].seq < events.iter().map(|e| e.seq).max().unwrap(),
        "usage 事件必须落在回合内、done 之前"
    );
}
