//! 模型调用失败的指数退避重试（x-retry）。
//!
//! **只包 provider 调用**：provider 调用是只读语义（产出步骤序列，不改状态）⇒
//! 重试安全；工具执行**不在**重试范围内（工具已写进会话历史，重放会重复副作用）。
//!
//! 参数（卡面定死）：最多重试 3 次（首发 + 3 次重试 = 4 次尝试）；退避
//! `delay = base * 2^attempt`，`base = 200ms` ⇒ 实际等待 200 / 400 / 800ms；抖动
//! ±25%；三次退避合计 ≤ 2s。
//!
//! 可重试错误是**白名单**：只 [`CoreError::Io`]（网络/IO 瞬时故障）；其余一律不重试
//! （新增错误变体默认落在「不可重试」一侧）。
//!
//! 可观测性：每次重试落一条 `model.retry` 事件（`attempt` / `delayMs` /
//! `errorKind` / `willRetry`）；失败且不再重试时**不**落该事件，原错误按既有语义
//! 向上抛。
//!
//! 退避时长可注入（[`Sleeper`]）⇒ 测试不必真等；抖动源可注入（[`Jitter`]）⇒
//! 退避时长可复现。生产路径不设置覆盖 ⇒ 走真实线程休眠与时钟播种的伪随机。

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use uuid::Uuid;

use crate::error::CoreError;
use crate::provider::{ProviderStep, StepOptions, TurnContext};
use crate::schema::Event;

use super::Runtime;

/// `model.retry` 事件的 kind（定死；沿用仓内点分命名）。
pub const MODEL_RETRY_EVENT: &str = "model.retry";

/// 退避基准间隔（卡面定死：200ms）。
pub const BASE_DELAY_MS: u64 = 200;

/// 最大重试次数（卡面定死：3 次 ⇒ 最多 4 次尝试：首发 + 3 次重试）。
pub const MAX_RETRIES: u32 = 3;

/// 退避总耗时上限（卡面定死：2 秒）。
pub const MAX_TOTAL_DELAY_MS: u64 = 2_000;

/// 抖动幅度（卡面定死：±25%）。
pub const JITTER_RATIO: f64 = 0.25;

/// 退避等待器（可注入 ⇒ 测试不必真等）。
pub trait Sleeper: Send + Sync {
    /// 等待给定时长。
    fn sleep(&self, delay: Duration);
}

/// 抖动源（可注入 ⇒ 测试用固定种子复现）。
pub trait Jitter: Send + Sync {
    /// 返回 [0, 1) 区间的伪随机数。
    fn unit(&self) -> f64;
}

/// 一次重试的通知（供上层落 `model.retry` 事件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryNotice {
    /// 第几次重试（从 1 计）。
    pub attempt: u32,
    /// 本次退避的实际时长（毫秒）。
    pub delay_ms: u64,
    /// 错误分类标签（如 `"io"`）。
    pub error_kind: &'static str,
    /// 本次重试之后是否还会有下一次重试。
    pub will_retry: bool,
}

/// 指数退避重试地调用一次 provider（策略层）。
///
/// `call` 是「一次 provider 调用」；失败且错误可重试时按 `base * 2^n` 退避后重试，
/// 最多 [`MAX_RETRIES`] 次；每次重试前经 `on_retry` 通知上层（上层落 `model.retry`
/// 事件）。
///
/// **退避只包 provider 调用**：工具执行不在本函数范围内（工具已写进会话历史，
/// 重放会重复副作用）。
pub fn call_with_retry<F, N>(
    mut call: F,
    sleeper: &dyn Sleeper,
    jitter: &dyn Jitter,
    on_retry: &mut N,
) -> Result<Vec<ProviderStep>, CoreError>
where
    F: FnMut() -> Result<Vec<ProviderStep>, CoreError>,
    N: FnMut(RetryNotice) -> Result<(), CoreError>,
{
    // 已完成的重试次数。
    let mut retried = 0u32;
    // 已累计的退避时长（总耗时上限按「退避合计」计）。
    let mut waited = Duration::ZERO;
    loop {
        let batch = match call() {
            Ok(batch) => return Ok(batch),
            Err(err) => err,
        };
        // 白名单：只 `Io` 是瞬时错误；其余（含新增变体）一律不重试。
        let Some(error_kind) = retryable_kind(&batch) else {
            return Err(batch);
        };
        if retried >= MAX_RETRIES {
            return Err(batch);
        }
        let delay = backoff_delay(retried, jitter);
        // 退避合计超上限 ⇒ 不再重试，直接返回最后一次错误。
        if waited + delay > Duration::from_millis(MAX_TOTAL_DELAY_MS) {
            return Err(batch);
        }
        retried += 1;
        on_retry(RetryNotice {
            attempt: retried,
            delay_ms: delay.as_millis() as u64,
            error_kind,
            will_retry: retried < MAX_RETRIES,
        })?;
        sleeper.sleep(delay);
        waited += delay;
    }
}

/// 可重试错误的分类标签（**白名单**：只 `Io` 是瞬时错误；其余一律不重试，
/// 新增错误变体默认落在「不重试」一侧）。
fn retryable_kind(err: &CoreError) -> Option<&'static str> {
    match err {
        CoreError::Io { .. } => Some("io"),
        _ => None,
    }
}

/// 第 `index` 次退避（`index` 从 0 计）的时长：指数退避 + ±25% 抖动。
fn backoff_delay(index: u32, jitter: &dyn Jitter) -> Duration {
    let factor = 1.0 - JITTER_RATIO + jitter.unit() * (2.0 * JITTER_RATIO);
    let ms = (BASE_DELAY_MS as f64 * 2f64.powi(index as i32) * factor).round();
    Duration::from_millis(ms.max(0.0) as u64)
}

thread_local! {
    /// 本线程的退避等待器覆盖（测试/宿主自检用；`None` ⇒ 走真实线程休眠）。
    static SLEEPER_OVERRIDE: RefCell<Option<Arc<dyn Sleeper>>> = const { RefCell::new(None) };
    /// 本线程的抖动源覆盖（测试/宿主自检用；`None` ⇒ 走默认伪随机）。
    static JITTER_OVERRIDE: RefCell<Option<Arc<dyn Jitter>>> = const { RefCell::new(None) };
}

/// 覆盖本线程的退避等待器（**测试/宿主自检专用**：把退避压到 0 ⇒ 测试不必真等）。
/// 传 `None` 复原默认（真实线程休眠）。
pub fn override_sleeper(sleeper: Option<Arc<dyn Sleeper>>) {
    SLEEPER_OVERRIDE.with(|slot| *slot.borrow_mut() = sleeper);
}

/// 覆盖本线程的抖动源（**测试/宿主自检专用**：固定种子 ⇒ 退避时长可复现）。
/// 传 `None` 复原默认（时钟 + 会话 id 播种的伪随机）。
pub fn override_jitter(jitter: Option<Arc<dyn Jitter>>) {
    JITTER_OVERRIDE.with(|slot| *slot.borrow_mut() = jitter);
}

/// 生效的退避等待器：覆盖优先，否则真实线程休眠。
fn active_sleeper() -> Arc<dyn Sleeper> {
    SLEEPER_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| Arc::new(ThreadSleeper))
}

/// 生效的抖动源：覆盖优先，否则按「时钟纳秒 + 会话 id」播种的伪随机。
fn active_jitter(session: Uuid) -> Arc<dyn Jitter> {
    JITTER_OVERRIDE
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| Arc::new(SeededJitter::new(session)))
}

/// 固定种子的抖动源（**测试/宿主自检专用**：退避时长可复现）。
pub fn seeded_jitter(seed: u64) -> Arc<dyn Jitter> {
    Arc::new(SeededJitter::with_seed(seed))
}

/// 真实等待器：线程休眠。
struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

/// 默认抖动源：xorshift64*（不引依赖）；种子取时钟纳秒 + 会话 id ⇒ 多会话不同相。
struct SeededJitter {
    state: Mutex<u64>,
}

impl SeededJitter {
    /// 按会话播种（时钟纳秒 + 会话 id ⇒ 多会话不同相）。
    fn new(session: Uuid) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or_default();
        Self::with_seed(nanos ^ (session.as_u128() as u64))
    }

    /// 固定种子。
    fn with_seed(seed: u64) -> Self {
        // 0 是 xorshift 的不动点 ⇒ 强制非 0。
        Self {
            state: Mutex::new(seed | 1),
        }
    }

    fn next_unit(&self) -> f64 {
        let mut guard = self.state.lock().expect("抖动源锁不得毒化");
        let mut x = *guard;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *guard = x;
        // 取高 53 位 ⇒ [0, 1)
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

impl Jitter for SeededJitter {
    fn unit(&self) -> f64 {
        self.next_unit()
    }
}

impl Runtime {
    /// 带指数退避重试的 provider 调用（**只包 provider 调用**：工具执行不在范围内）。
    ///
    /// `options` 决定调用哪个 provider 方法（`Some` ⇒ 带选项，`None` ⇒ 不带选项），
    /// 与既有两处调用点的语义逐字一致。
    ///
    /// `events` 是回合事件累加器：`model.retry` 事件既落持久日志也进累加器
    /// （仓内既有契约「receipt 即持久日志，不得旁路」要求回合事件两处齐全）。
    pub(crate) fn call_provider_with_retry(
        &self,
        ctx: &TurnContext,
        options: Option<&StepOptions>,
        events: &mut Vec<Event>,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let call = || match options {
            Some(options) => self.provider.next_steps_with_options(ctx, options),
            None => self.provider.next_steps_with_context(ctx),
        };
        let sleeper = active_sleeper();
        let jitter = active_jitter(ctx.session_id);
        let mut notice = |notice: RetryNotice| {
            let session = self.get_session(ctx.session_id)?;
            events.push(self.emit(
                &session,
                MODEL_RETRY_EVENT,
                json!({
                    "attempt": notice.attempt,
                    "delayMs": notice.delay_ms,
                    "errorKind": notice.error_kind,
                    "willRetry": notice.will_retry,
                }),
            )?);
            Ok(())
        };
        call_with_retry(call, sleeper.as_ref(), jitter.as_ref(), &mut notice)
    }
}
