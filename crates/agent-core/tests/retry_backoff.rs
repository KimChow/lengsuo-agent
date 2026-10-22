//! 模型调用失败的指数退避重试（x-retry）契约测试。
//!
//! 覆盖卡面七条：Io 重试后成功且只记一条 usage / Io 连续失败返回**最后一次**错误 /
//! 非 Io 不重试 / 退避序列 200-400-800ms / 抖动在 ±25% 内 / 每次重试落一条
//! `model.retry` 且 `attempt` 递增 / 工具执行失败不触发重试。
//!
//! 退避时长与抖动都经注入的假 sleeper / 假抖动源 ⇒ **测试不必真等**。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TokenUsage};
use agent_core::runtime::retry_policy::{
    override_jitter, override_sleeper, seeded_jitter, Jitter, Sleeper, BASE_DELAY_MS, MAX_RETRIES,
    MODEL_RETRY_EVENT,
};
use agent_core::runtime::usage_log::MODEL_USAGE_EVENT;
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Event, Isolation, Mode};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};
use agent_core::{PermissionGate, ToolExecutor};
use serde_json::{json, Value};

/// 假 sleeper：记录每次等待的时长，不真等。
#[derive(Default)]
struct RecordingSleeper {
    delays: Mutex<Vec<Duration>>,
}

impl Sleeper for RecordingSleeper {
    fn sleep(&self, delay: Duration) {
        self.delays.lock().expect("锁不得毒化").push(delay);
    }
}

/// 恒定抖动源（无抖动 ⇒ 退避时长可精确断言）。
struct FixedJitter(f64);

impl Jitter for FixedJitter {
    fn unit(&self) -> f64 {
        self.0
    }
}

/// 装上假等待器 + 恒定抖动源，返回等待器以便断言。
fn fake_sleeper() -> Arc<RecordingSleeper> {
    let sleeper = Arc::new(RecordingSleeper::default());
    override_sleeper(Some(sleeper.clone() as Arc<dyn Sleeper>));
    override_jitter(Some(Arc::new(FixedJitter(0.5))));
    sleeper
}

/// 剧本的一步：模拟失败或返回给定步骤。
enum Step {
    /// 模拟 `Io` 瞬时故障（路径带调用序号 ⇒ 能分辨第一次 / 最后一次）。
    Io,
    /// 模拟确定性配置错误（不可重试）。
    InvalidConfig,
    /// 成功。
    Ok(Vec<ProviderStep>),
}

/// 按调用序给出结果的 provider。
struct ScriptedProvider {
    script: Mutex<Vec<Step>>,
    calls: AtomicU32,
    usage: Option<TokenUsage>,
}

impl ScriptedProvider {
    fn new(script: Vec<Step>, usage: Option<TokenUsage>) -> Self {
        Self {
            script: Mutex::new(script),
            calls: AtomicU32::new(0),
            usage,
        }
    }

    fn call_count(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
        let script = self.script.lock().expect("剧本锁不得毒化");
        match script.get(index) {
            Some(Step::Io) => Err(CoreError::Io {
                path: PathBuf::from(format!("/tmp/retry-backoff-net-{index}")),
                source: std::io::Error::other("模拟网络抖动"),
            }),
            Some(Step::InvalidConfig) => Err(CoreError::InvalidConfig("模拟确定性失败".into())),
            Some(Step::Ok(steps)) => Ok(steps.clone()),
            None => Ok(vec![ProviderStep::Finished]),
        }
    }

    fn last_usage(&self) -> Option<TokenUsage> {
        self.usage
    }
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("retry-backoff-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    dir
}

fn runtime(tag: &str, provider: Arc<ScriptedProvider>) -> Runtime {
    Runtime::builder()
        .workdir(workdir(tag))
        .provider(provider)
        .build()
        .expect("runtime")
}

fn session(rt: &Runtime) -> uuid::Uuid {
    rt.create_session(&CreateSession::plain("retry", Isolation::local_default()))
        .expect("session")
        .id
}

/// 连续 `times` 次 `Io` 失败的剧本。
fn failing_io(times: usize) -> Vec<Step> {
    (0..times).map(|_| Step::Io).collect()
}

/// 一个成功步（文本 + 结束）。
fn ok_step() -> Step {
    Step::Ok(vec![
        ProviderStep::Text("好了".into()),
        ProviderStep::Finished,
    ])
}

fn replay(rt: &Runtime, id: uuid::Uuid) -> Vec<Event> {
    rt.event_log().replay(id, 0, 1000).expect("replay")
}

fn of_kind<'a>(events: &'a [Event], kind: &str) -> Vec<&'a Event> {
    events.iter().filter(|e| e.kind == kind).collect()
}

/// 取某类事件里某键的数值序列。
fn nums(events: &[Event], kind: &str, key: &str) -> Vec<u64> {
    of_kind(events, kind)
        .iter()
        .map(|e| e.payload[key].as_u64().expect("数值键"))
        .collect()
}

/// 假等待器记录的各次 delay（毫秒）。
fn delays_ms(sleeper: &RecordingSleeper) -> Vec<u64> {
    sleeper
        .delays
        .lock()
        .expect("锁")
        .iter()
        .map(|d| d.as_millis() as u64)
        .collect()
}

/// 跑一个「连续 Io 失败」的回合：返回（事件面、各次退避毫秒、provider 调用次数）。
fn run_failing(tag: &str, times: usize, jitter: Arc<dyn Jitter>) -> (Vec<Event>, Vec<u64>, u32) {
    let sleeper = Arc::new(RecordingSleeper::default());
    override_sleeper(Some(sleeper.clone() as Arc<dyn Sleeper>));
    override_jitter(Some(jitter));
    let provider = Arc::new(ScriptedProvider::new(failing_io(times), None));
    let rt = runtime(tag, provider.clone());
    let id = session(&rt);
    let _ = rt.send_text(id, "开工");
    (replay(&rt, id), delays_ms(&sleeper), provider.call_count())
}

/// 1. `Io` 错误重试 ⇒ 第 2 次成功 ⇒ 返回成功结果，**且只产生 1 条**最终 usage 事件。
///    附带核对仓内既有契约「receipt 即持久日志，不得旁路」：重试事件既落盘也进 receipt。
#[test]
fn io_error_is_retried_then_succeeds_with_one_usage_event() {
    let sleeper = fake_sleeper();
    let provider = Arc::new(ScriptedProvider::new(
        vec![Step::Io, ok_step()],
        Some(TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 10,
            total_tokens: 110,
            cached_tokens: 0,
        }),
    ));
    let rt = runtime("retry-ok", provider.clone());
    let id = session(&rt);

    // 建会话本身先落一条 session_update；契约只比对回合区段。
    let pre_turn_seq = rt.get_session(id).expect("session").last_seq;
    let receipt = rt.send_text(id, "开工").expect("回合必须成功");

    let events = rt
        .event_log()
        .replay(id, pre_turn_seq, 1000)
        .expect("replay");
    let usage = of_kind(&events, MODEL_USAGE_EVENT);
    assert_eq!(usage.len(), 1, "只记最终成功那一次的 usage");
    assert_eq!(usage[0].payload["billed_tokens"], 110);
    let retry = of_kind(&events, MODEL_RETRY_EVENT);
    assert_eq!(retry.len(), 1, "一次失败 ⇒ 一条重试事件");
    assert_eq!(retry[0].payload["attempt"], 1);
    assert_eq!(retry[0].payload["errorKind"], "io");
    assert_eq!(retry[0].payload["delayMs"], 200);
    assert_eq!(provider.call_count(), 2, "首发 + 1 次重试");
    assert_eq!(delays_ms(&sleeper), vec![200]);
    // receipt 即持久日志，不得旁路。
    let receipt_seq: Vec<u64> = receipt.events.iter().map(|e| e.seq).collect();
    let replay_seq: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(receipt_seq, replay_seq, "receipt 与持久日志必须逐项一致");
    assert!(
        receipt.events.iter().any(|e| e.kind == MODEL_RETRY_EVENT),
        "重试事件必须进 receipt"
    );
}

/// 2. `Io` 错误连续失败到上限 ⇒ 返回**最后一次**错误（不是第一次），尝试 4 次。
#[test]
fn io_errors_exhausting_retries_return_the_last_error() {
    let sleeper = fake_sleeper();
    let provider = Arc::new(ScriptedProvider::new(failing_io(5), None));
    let rt = runtime("retry-exhaust", provider.clone());
    let id = session(&rt);

    let err = rt.send_text(id, "开工").expect_err("连续失败必须报错");

    let CoreError::Io { path, .. } = &err else {
        panic!("必须返回 Io 错误，实际：{err:?}");
    };
    assert_eq!(
        path,
        &PathBuf::from("/tmp/retry-backoff-net-3"),
        "必须返回最后一次（第 4 次调用）的错误，不是第一次"
    );
    assert_eq!(provider.call_count(), 4, "首发 + 3 次重试 = 4 次尝试");
    assert_eq!(delays_ms(&sleeper), vec![200, 400, 800], "三次退避");
}

/// 3. 非 `Io` 错误（如 `InvalidConfig`）⇒ **不重试**（sleeper 调用次数 = 0）。
#[test]
fn non_io_error_is_not_retried() {
    let sleeper = fake_sleeper();
    let provider = Arc::new(ScriptedProvider::new(
        vec![Step::InvalidConfig, Step::Ok(vec![ProviderStep::Finished])],
        None,
    ));
    let rt = runtime("retry-nonretryable", provider.clone());
    let id = session(&rt);

    let err = rt.send_text(id, "开工").expect_err("确定性失败必须报错");

    assert!(
        matches!(err, CoreError::InvalidConfig(_)),
        "原错误按既有语义向上抛：{err:?}"
    );
    assert_eq!(provider.call_count(), 1, "确定性错误不重试");
    assert!(delays_ms(&sleeper).is_empty(), "确定性错误不得退避等待");
    let events = replay(&rt, id);
    assert!(
        of_kind(&events, MODEL_RETRY_EVENT).is_empty(),
        "非重试错误不落 model.retry 事件"
    );
}

/// 4. 退避序列 = 200 / 400 / 800ms（注入假 sleeper 记录各次 delay）。
#[test]
fn backoff_sequence_is_200_400_800_ms() {
    let (_events, delays, calls) = run_failing("retry-seq", 4, Arc::new(FixedJitter(0.5)));

    assert_eq!(delays, vec![200, 400, 800], "指数退避：base * 2^n");
    assert_eq!(
        delays.len(),
        MAX_RETRIES as usize,
        "退避次数 = 最大重试次数"
    );
    assert_eq!(calls, 4, "首发 + 3 次重试 = 4 次尝试");
}

/// 5. 抖动存在且在 ±25% 内（注入固定种子的假随机，断言 delay ∈ 区间）。
#[test]
fn jitter_stays_within_25_percent() {
    let (_events, delays, _calls) = run_failing("retry-jitter", 4, seeded_jitter(0x5eed));

    assert_eq!(delays.len(), 3);
    for (index, delay) in delays.iter().enumerate() {
        let base = BASE_DELAY_MS * 2u64.pow(index as u32);
        let low = base * 3 / 4;
        let high = base * 5 / 4;
        assert!(
            (low..=high).contains(delay),
            "第 {index} 次退避 {delay}ms 超出 ±25% 区间 [{low}, {high}]"
        );
    }
    let pure: Vec<u64> = (0..3).map(|i| BASE_DELAY_MS * 2u64.pow(i)).collect();
    assert_ne!(delays, pure, "抖动必须真的扰动退避时长");
}

/// 6. 每次重试落一条 `model.retry` 事件，`attempt` 从 1 递增。
#[test]
fn each_retry_emits_a_model_retry_event_with_increasing_attempt() {
    let (events, _delays, _calls) = run_failing("retry-events", 4, Arc::new(FixedJitter(0.5)));

    let retry = of_kind(&events, MODEL_RETRY_EVENT);
    assert_eq!(retry.len(), 3, "三次退避 ⇒ 三条事件");
    assert_eq!(nums(&events, MODEL_RETRY_EVENT, "attempt"), vec![1, 2, 3]);
    assert_eq!(
        nums(&events, MODEL_RETRY_EVENT, "delayMs"),
        vec![200, 400, 800]
    );
    let will_retry: Vec<bool> = retry
        .iter()
        .map(|e| e.payload["willRetry"].as_bool().expect("布尔键"))
        .collect();
    assert_eq!(
        will_retry,
        vec![true, true, false],
        "最后一次重试之后没有下一次"
    );
    for event in &retry {
        assert_eq!(event.payload["errorKind"], "io");
    }
    // 失败且不再重试时不落 model.retry ⇒ 没有第四条。
    assert_eq!(retry.len(), MAX_RETRIES as usize);
}

/// 7. 工具执行失败**不触发**重试（断言工具路径未进入重试计数）。
#[test]
fn tool_execution_failure_does_not_trigger_retry() {
    /// 恒失败的工具执行器。
    struct FailingExecutor;

    impl ToolExecutor for FailingExecutor {
        fn execute(&self, _name: &str, _args: &Value) -> Result<Value, String> {
            Err("模拟工具执行失败".to_string())
        }
    }

    let sleeper = fake_sleeper();
    let provider = Arc::new(ScriptedProvider::new(
        vec![
            Step::Ok(vec![ProviderStep::ToolCall {
                name: "demo.tool".into(),
                args: json!({}),
            }]),
            Step::Ok(vec![
                ProviderStep::Text("收工".into()),
                ProviderStep::Finished,
            ]),
        ],
        None,
    ));
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: "demo.tool".into(),
        description: "演示工具".into(),
        kind: ToolKind::Read,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    let gate = PermissionGate::new(registry)
        .with_whitelist(Mode::Ask, BTreeSet::from(["demo.tool".to_string()]))
        .with_auto_allowed("demo.tool");
    let rt = Runtime::builder()
        .workdir(workdir("retry-tool"))
        .provider(provider.clone())
        .gate(gate)
        .build()
        .expect("runtime");
    rt.set_tool_executor(Arc::new(FailingExecutor));
    let id = session(&rt);

    rt.send_text(id, "开工").expect("工具失败不中断回合");

    let events = replay(&rt, id);
    assert!(
        of_kind(&events, MODEL_RETRY_EVENT).is_empty(),
        "工具执行失败不触发重试"
    );
    assert!(delays_ms(&sleeper).is_empty(), "工具执行失败不触发退避等待");
    assert_eq!(
        provider.call_count(),
        2,
        "两次 provider 调用分别属于「工具步」与「续步」，都与重试无关"
    );
    let tool_results = of_kind(&events, "tool_result");
    assert_eq!(tool_results.len(), 1, "工具失败结算成一条结果");
    assert_eq!(tool_results[0].payload["isError"], true);
}
