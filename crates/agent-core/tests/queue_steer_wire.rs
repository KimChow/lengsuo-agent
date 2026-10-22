//! 两层投递（steer 插话 + queue 排队 + ownerRequestId 过期判定）的线缆契约测试。
//!
//! 覆盖卡面七条：① 插话不阻塞且在**下一个步边界**被消费；② 排队不阻塞且在**回合
//! 收口后**开新回合被消费；③ steer 早于 queue；④ owner 变更后旧项过期且有可计数
//! 证据；⑤ 过期项不执行；⑥ 入队文本脱敏；⑦ 并发投递无数据竞争。

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Event, Isolation};
use serde_json::json;
use uuid::Uuid;

/// 卡面要求「异步测试必须有超时」：所有等待都经本上限。
const WAIT: Duration = Duration::from_secs(10);
/// 投递必须「立即返回」的判定上限（远小于 WAIT，避免把阻塞误判成慢）。
const NON_BLOCKING: Duration = Duration::from_secs(1);

// ---------------- provider ----------------

/// 一步以工具调用终结（无文本、无 Finished）⇒ 回合继续下一步。未注入执行器 ⇒ 工具步
/// 落「待宿主执行」占位（既有语义），回合照常推进。
fn tool_step(name: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args: json!({ "path": "a.txt" }),
    }]
}

/// 一步收敛（文本 + Finished）。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

/// 闸门：第 0 次调用卡住，直到测试放行（模拟「回合进行中」）。
struct Gate {
    started: Option<Sender<()>>,
    release: Option<Receiver<()>>,
}

/// 脚本化 provider：按脚本出队并记录每次调用的上下文；带闸门时第 0 次调用先卡住
/// （此刻外部可以并行投递）。
struct ScriptProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<TurnContext>>,
    gate: Mutex<Option<Gate>>,
}

impl ScriptProvider {
    /// 不带闸门（每步直接出队）。
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Self::build(steps, None)
    }

    /// 带闸门：返回 (provider, 「已进入第一步」信号, 放行信号)。
    fn gated(steps: Vec<Vec<ProviderStep>>) -> (Arc<Self>, Receiver<()>, Sender<()>) {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = Gate {
            started: Some(started_tx),
            release: Some(release_rx),
        };
        (Self::build(steps, Some(gate)), started_rx, release_tx)
    }

    fn build(steps: Vec<Vec<ProviderStep>>, gate: Option<Gate>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
            gate: Mutex::new(gate),
        })
    }

    /// 已记录的全部上下文（断言「模型看到了什么」用）。
    fn seen(&self) -> Vec<TurnContext> {
        self.seen.lock().expect("锁").clone()
    }
}

impl Provider for ScriptProvider {
    fn next_steps(
        &self,
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本测试只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let mut seen = self.seen.lock().expect("锁");
        seen.push(context.clone());
        let first = seen.len() == 1;
        drop(seen);
        if first {
            // 第 0 次调用卡住：此刻回合「正在进行中」，外部可以并行投递。
            let mut gate = self.gate.lock().expect("锁");
            if let Some(gate) = gate.as_mut() {
                if let Some(tx) = gate.started.take() {
                    let _ = tx.send(());
                }
                if let Some(rx) = gate.release.as_ref() {
                    rx.recv_timeout(WAIT).expect("放行");
                }
            }
        }
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported("脚本已耗尽：出现计划外的调用"))
    }
}

// ---------------- harness ----------------

fn runtime(provider: Arc<dyn Provider>) -> Runtime {
    let dir = std::env::temp_dir().join(format!("queue-steer-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .build()
        .expect("runtime")
}

fn session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession::plain(
        "两层投递",
        Isolation::local_default(),
    ))
    .expect("create session")
    .id
}

/// 后台线程跑 `send_text`。
fn spawn_turn(rt: &Runtime, id: Uuid, text: &'static str) -> JoinHandle<TurnReceipt> {
    let rt = rt.clone();
    std::thread::spawn(move || rt.send_text(id, text).expect("回合不得失败"))
}

fn join(handle: JoinHandle<TurnReceipt>) -> TurnReceipt {
    handle.join().expect("回合线程不得 panic")
}

/// 某类事件（可计数证据）。
fn events_of(rt: &Runtime, id: Uuid, kind: &str) -> Vec<Event> {
    rt.replay(id, 0, 1000)
        .expect("回放")
        .into_iter()
        .filter(|event| event.kind == kind)
        .collect()
}

/// 上下文里是否出现过某段文本（输入或历史任一处）。
fn context_has(context: &TurnContext, needle: &str) -> bool {
    context.user_text.contains(needle)
        || context
            .history
            .iter()
            .any(|message| message.content.contains(needle))
}

// ---------------- 1. 插话不阻塞 + 下一步边界消费 ----------------

/// 回合进行中投递插话 ⇒ 立即返回（不排在投递锁上）；插话在**下一个步边界**注入
/// 进历史 ⇒ 第二步的上下文能看到它（第一步看不到）。
#[test]
fn steer_does_not_block_and_is_consumed_at_the_next_step_boundary() {
    let (provider, started_rx, release_tx) =
        ScriptProvider::gated(vec![tool_step("fs.read"), text_step("第二步")]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    let handle = spawn_turn(&rt, id, "开工");
    started_rx.recv_timeout(WAIT).expect("回合已进入第一步");

    let began = Instant::now();
    rt.send_steer(id, "owner-1", "插话内容").expect("插话投递");
    let elapsed = began.elapsed();
    assert!(elapsed < NON_BLOCKING, "投递不得阻塞：{elapsed:?}");

    release_tx.send(()).expect("放行");
    join(handle);

    let seen = provider.seen();
    assert_eq!(seen.len(), 2, "回合两步（工具步 + 收敛步）");
    assert!(!context_has(&seen[0], "插话内容"), "第一步看不到插话");
    assert!(context_has(&seen[1], "插话内容"), "第二步看到插话");
    let consumed = events_of(&rt, id, "turn.steer.consumed").len();
    let enqueued = events_of(&rt, id, "turn.steer.enqueued").len();
    assert_eq!(consumed, 1, "插话消费证据");
    assert_eq!(enqueued, 1, "插话入队证据");
}

// ---------------- 2. 排队不阻塞 + 回合收口后开新回合 ----------------

/// 回合进行中投递排队消息 ⇒ 立即返回；该消息在**本回合收口后**由**新回合**消费
/// （新回合首步的输入就是它）。
#[test]
fn queued_does_not_block_and_is_consumed_by_a_new_turn_after_the_turn_closes() {
    let (provider, started_rx, release_tx) = ScriptProvider::gated(vec![
        tool_step("fs.read"),
        text_step("第一回合收敛"),
        text_step("排队回合收敛"),
    ]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    let handle = spawn_turn(&rt, id, "开工");
    started_rx.recv_timeout(WAIT).expect("回合已进入第一步");

    let began = Instant::now();
    rt.send_queued(id, "owner-1", "排队内容").expect("排队投递");
    let elapsed = began.elapsed();
    assert!(elapsed < NON_BLOCKING, "投递不得阻塞：{elapsed:?}");

    release_tx.send(()).expect("放行");
    let receipt = join(handle);

    let seen = provider.seen();
    assert_eq!(seen.len(), 3, "第一回合两步 + 排队回合一步");
    assert!(!context_has(&seen[1], "排队内容"), "排队不插进进行中的回合");
    assert!(seen[2].user_text.contains("排队内容"), "排队在新回合消费");
    let consumed = events_of(&rt, id, "turn.queued.consumed").len();
    assert_eq!(consumed, 1, "排队消费证据");
    let dones = receipt.events.iter().filter(|e| e.kind == "done").count();
    assert_eq!(dones, 2, "两个回合（原本 + 排队）各一条 done 收口");
}

// ---------------- 3. 取用时机不同 ----------------

/// 同一回合进行中先插话、后排队 ⇒ 插话在步边界被消费（第二步可见），排队消息在回合
/// 收口后的新回合被消费 ⇒ **steer 早于 queue**。
#[test]
fn steer_is_consumed_before_queued() {
    let (provider, started_rx, release_tx) = ScriptProvider::gated(vec![
        tool_step("fs.read"),
        text_step("第一回合收敛"),
        text_step("排队回合收敛"),
    ]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    let handle = spawn_turn(&rt, id, "开工");
    started_rx.recv_timeout(WAIT).expect("回合已进入第一步");
    rt.send_steer(id, "owner-1", "插话内容").expect("插话投递");
    rt.send_queued(id, "owner-1", "排队内容").expect("排队投递");
    release_tx.send(()).expect("放行");
    join(handle);

    let seen = provider.seen();
    let steer_at = seen.iter().position(|c| context_has(c, "插话内容"));
    let queued_at = seen.iter().position(|c| context_has(c, "排队内容"));
    assert!(steer_at.is_some() && queued_at.is_some(), "两者都被消费");
    assert!(steer_at < queued_at, "插话早于排队");
}

// ---------------- 4. 过期判定 ----------------

/// owner 变更后，旧的队列项被判**过期** ⇒ 丢弃且有 `turn.queue.expired` 证据。
#[test]
fn stale_owner_queue_item_is_expired_with_countable_evidence() {
    let provider = ScriptProvider::new(vec![text_step("收到")]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    rt.send_queued(id, "owner-A", "过期文本").expect("排队投递");
    // owner 变更：最近一次带 owner 的投递换了人 ⇒ 旧项过期。
    rt.send_text_as(id, "owner-B", "新输入").expect("投递");

    let expired = events_of(&rt, id, "turn.queue.expired");
    assert_eq!(expired.len(), 1, "过期可计数（恰好一条）");
    assert_eq!(expired[0].payload["ownerRequestId"], "owner-A");
    assert_eq!(expired[0].payload["kind"], "queued");
    assert_eq!(
        events_of(&rt, id, "turn.queued.consumed").len(),
        0,
        "过期项未消费"
    );
}

// ---------------- 5. 过期项不执行 ----------------

/// 过期项**不执行**：模型既没在本轮输入里、也没在历史里看到过期文本。
#[test]
fn expired_item_is_never_delivered_to_the_model() {
    let provider = ScriptProvider::new(vec![text_step("收到")]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    rt.send_queued(id, "owner-A", "过期文本").expect("排队投递");
    rt.send_text_as(id, "owner-B", "新输入").expect("投递");

    let seen = provider.seen();
    assert!(!seen.is_empty(), "至少跑了一个回合");
    for context in &seen {
        assert!(!context_has(context, "过期文本"), "不入模型上下文");
    }
    let messages = rt.get_session(id).expect("session").messages;
    let leaked = messages.iter().any(|m| m.content.contains("过期文本"));
    assert!(!leaked, "不入历史");
}

// ---------------- 6. 脱敏 ----------------

/// 入队文本含凭据样式串 ⇒ 队列里存的是**脱敏后**文本（事件与历史都不带明文）。
#[test]
fn queued_text_is_redacted_before_entering_the_queue() {
    let provider = ScriptProvider::new(vec![text_step("收到"), text_step("收到")]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    rt.send_queued(id, "owner-A", "password: sup3rs3cretvalue")
        .expect("排队投递");

    let enqueued = events_of(&rt, id, "turn.queued.enqueued");
    assert_eq!(enqueued.len(), 1, "入队事件一条");
    let text = enqueued[0].payload["text"].as_str().expect("text");
    assert!(
        !text.contains("sup3rs3cretvalue"),
        "凭据明文不得入队：{text}"
    );
    assert!(text.contains("***"), "脱敏占位符必须就位：{text}");

    // 消费侧同样只拿到脱敏文本。
    rt.send_text_as(id, "owner-A", "下一轮").expect("投递");
    let messages = rt.get_session(id).expect("session").messages;
    let leaked = messages
        .iter()
        .any(|m| m.content.contains("sup3rs3cretvalue"));
    assert!(!leaked, "凭据明文不得落进会话历史");
}

// ---------------- 7. 并发安全 ----------------

/// 多线程同时投递 ⇒ 不 panic、无数据竞争；每条投递都留下一条入队证据。
#[test]
fn concurrent_deliveries_are_safe() {
    let provider = ScriptProvider::new(vec![text_step("收到")]);
    let rt = runtime(provider.clone());
    let id = session(&rt);

    let mut handles: Vec<JoinHandle<()>> = Vec::new();
    for i in 0..8 {
        let rt = rt.clone();
        handles.push(std::thread::spawn(move || {
            rt.send_steer(id, &format!("owner-{i}"), &format!("插话-{i}"))
                .expect("插话投递");
            rt.send_queued(id, &format!("owner-{i}"), &format!("排队-{i}"))
                .expect("排队投递");
        }));
    }
    for handle in handles {
        handle.join().expect("投递线程不得 panic");
    }

    assert_eq!(
        events_of(&rt, id, "turn.steer.enqueued").len(),
        8,
        "八条插话入队证据"
    );
    assert_eq!(
        events_of(&rt, id, "turn.queued.enqueued").len(),
        8,
        "八条排队入队证据"
    );
}
