//! 取消信号：在途工具的可打断契约（tokio-free）。
//!
//! 本层是同步域逻辑，不依赖 tokio。`CancelSignal` 复用会话既有的取消令牌
//! （`HotSession.cancel` 的 `Arc<AtomicBool>`），另加一把进程级 `Condvar`
//! 唤醒总线：置取消时唤醒所有等待者，执行器无需轮询即可在取消后立即收尾。
//!
//! 两段时间口径（详见 `docs/cancel-intra-status.md`）：
//! - **执行区间可打断**：取消到达后，可取消执行器应自行收尾；
//! - **结算段不可打断**：执行器返回后的结算（含未配对调用的中断结算）不查
//!   取消标志，必须完成。
//!
//! 宽限与硬上限是**发布给执行器**的策略常量：[`CANCEL_GRACE_MS`] 是取消后
//! 留给执行器自行收尾的宽限窗口，[`CANCEL_HARD_LIMIT_MS`] 是绝对硬上限。
//! 真正的「TERM → 宽限 → KILL」升级由进程型执行器（`agent-tools-exec`）实现，
//! 不在本层（本层只持有信号与口径）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::types::CancellableToolExecutor;

/// 取消后的宽限窗口（毫秒）：执行器观察到取消后应在此窗口内自行收尾。
pub const CANCEL_GRACE_MS: u64 = 100;

/// 取消后的硬上限（毫秒）：超过此窗口仍未收尾 ⇒ 视为强制中断。
pub const CANCEL_HARD_LIMIT_MS: u64 = 3_000;

/// 进程级唤醒总线。
///
/// 置取消时 `notify_all`；等待者被唤醒后**重新检查自己的标志**（伪唤醒无害）。
/// 单个进程级总线足够：会话量级远小于唤醒频率，且唤醒本身只是顺带的一次
/// `notify_all`，不出现「谁没被唤醒」的定位问题。
struct WakeBus {
    lock: Mutex<()>,
    cv: Condvar,
}

static BUS: WakeBus = WakeBus {
    lock: Mutex::new(()),
    cv: Condvar::new(),
};

/// 取消信号（可跨线程共享、可克隆）。
///
/// 与本会话的取消令牌是**同一个** `Arc<AtomicBool>`：回合开始时 `turns.rs`
/// 复位它、`cancel_session` 置位它，信号因此无需第二份状态、也无需第二处复位。
#[derive(Clone, Debug)]
pub struct CancelSignal {
    flag: Arc<AtomicBool>,
}

impl CancelSignal {
    /// 独立信号（无会话绑定时使用，例如执行器单元自检）。
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 包装已有令牌（`Runtime::cancel_signal` 用它复用会话的取消令牌）。
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self { flag }
    }

    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// 置位并唤醒所有等待者（`cancel_session` 走这条路径）。
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        BUS.cv.notify_all();
    }

    /// 阻塞等待取消，至多 `timeout`；返回期间是否观察到取消。
    ///
    /// 等待者只在自己的标志上重试，因此其他会话的取消、伪唤醒都不影响结论。
    pub fn wait_cancelled(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut guard = BUS.lock.lock().expect("唤醒总线锁不得毒化");
        loop {
            if self.is_cancelled() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, _) = BUS
                .cv
                .wait_timeout(guard, deadline - now)
                .expect("唤醒总线锁不得毒化");
            guard = next;
        }
    }

    /// 观察到取消的那一刻起算的宽限截止时刻（[`CANCEL_GRACE_MS`]）。
    pub fn grace_deadline(&self, observed: Instant) -> Instant {
        observed + Duration::from_millis(CANCEL_GRACE_MS)
    }

    /// 观察到取消的那一刻起算的硬上限截止时刻（[`CANCEL_HARD_LIMIT_MS`]）。
    pub fn hard_deadline(&self, observed: Instant) -> Instant {
        observed + Duration::from_millis(CANCEL_HARD_LIMIT_MS)
    }
}

impl Default for CancelSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// 会话 → 可取消执行器（进程级登记表）。
///
/// 为什么是进程级而不是 `Runtime` 字段：`Runtime` / `HotSession` 都由本卡边界
/// 之外的 `builder.rs` / `sessions.rs` 以穷尽结构体字面量构造，新增字段会破坏
/// 那些构造点；既有执行器槽位是 `Arc<dyn ToolExecutor + …>`，而 trait 对象
/// 下转型需要 `ToolExecutor: Any`——那是本卡明确禁止修改的契约。故第二槽位
/// 按会话 id 登记在此。会话 id 是随机 UUID，跨 Runtime 不冲突；条目随会话
/// 生命周期存在（本仓无删会话 API），登记表不主动清理。
static CANCELLABLE: OnceLock<Mutex<HashMap<Uuid, Arc<dyn CancellableToolExecutor>>>> =
    OnceLock::new();

fn table() -> &'static Mutex<HashMap<Uuid, Arc<dyn CancellableToolExecutor>>> {
    CANCELLABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 给会话装可取消执行器（后写胜出）。
pub fn set_session_cancellable(id: Uuid, executor: Arc<dyn CancellableToolExecutor>) {
    table()
        .lock()
        .expect("可取消执行器登记表锁不得毒化")
        .insert(id, executor);
}

/// 取会话的可取消执行器。
pub fn session_cancellable(id: Uuid) -> Option<Arc<dyn CancellableToolExecutor>> {
    table()
        .lock()
        .expect("可取消执行器登记表锁不得毒化")
        .get(&id)
        .map(Arc::clone)
}

/// 卸下会话的可取消执行器（测试收尾用；生产无删会话 API）。
pub fn clear_session_cancellable(id: Uuid) {
    table()
        .lock()
        .expect("可取消执行器登记表锁不得毒化")
        .remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_wakes_on_cancel_from_another_thread() {
        let signal = CancelSignal::new();
        let other = signal.clone();
        let waiter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            other.cancel();
        });
        assert!(
            signal.wait_cancelled(Duration::from_secs(5)),
            "置取消后等待者必须被唤醒"
        );
        waiter.join().expect("join");
    }

    #[test]
    fn wait_times_out_without_cancel() {
        let signal = CancelSignal::new();
        assert!(!signal.wait_cancelled(Duration::from_millis(30)));
        assert!(!signal.is_cancelled());
    }

    #[test]
    fn deadlines_follow_grace_and_hard_limit() {
        let signal = CancelSignal::new();
        let now = Instant::now();
        assert_eq!(signal.grace_deadline(now), now + Duration::from_millis(100));
        assert_eq!(
            signal.hard_deadline(now),
            now + Duration::from_millis(3_000)
        );
    }
}
