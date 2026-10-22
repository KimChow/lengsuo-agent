//! 回合投递队列（两层投递：steer 插话 + queue 排队 + ownerRequestId 过期判定）。
//!
//! 语义（本卡只做**投递侧的入队与取用**，不碰「立刻中断当前步」——那属
//! `x-cancel-intra`）：
//! - **steer（插话）**：回合**进行中**投递 ⇒ 在**下一步边界**注入进会话历史，
//!   正在跑的回合下一一步就能看到这条新消息并继续；
//! - **queue（排队）**：回合**进行中或已结束**投递 ⇒ 进会话队列，在**本回合
//!   收口后**自动开新回合消费（每条排队消息一个回合）。
//!
//! 两者的区别只有**取用时机**：steer 在步边界取，queue 在回合边界取。
//!
//! **过期判定（D2）**：队列项记入队时的 `owner_request_id`；消费时若该项的
//! owner 已不是该会话当前的**活跃 owner** ⇒ 判过期，**丢弃**并记一条可查证据
//! （`turn.queue.expired`），fail-closed：不静默丢弃、也不静默执行。
//!
//! 活跃 owner 的口径：本仓 `send_text` 的公开签名不带 owner（公开 API 冻结，
//! 改它要牵动网关等跨 crate 调用点），故「调用方标识」只由**带 owner 的投递**
//! 给出——`send_text_as` / `send_steer` / `send_queued` 各带一个
//! `owner_request_id`，**最近一次带 owner 的投递**即为该会话的活跃 owner；
//! 不带 owner 的 `send_text` 不改活跃 owner（没有身份可改）。
//!
//! **队列的挂载点（D3 的偏离）**：D3 要求在 `Runtime` 结构体上加
//! `queue` 字段，但 `Runtime` 由 `builder.rs` 里的结构体字面量构造，加字段
//! 必然要改那个文件，而本卡边界禁改 `builder.rs`（落点清单只有四个文件）。
//! 故队列落为**进程级单例**（`OnceLock`），键是会话 id（UUID ⇒ 跨 Runtime
//! 实例不串扰）；`Runtime::queue()` 取用。若后续允许改 `builder.rs`，改成
//! `Runtime` 字段即可，接口不变。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use serde_json::json;
use uuid::Uuid;

use crate::error::CoreError;
use crate::schema::{Event, Role};

use super::{Runtime, TurnReceipt};

/// 队列项种类：steer（步边界取）/ queued（回合边界取）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueKind {
    /// 插话：下一步边界注入。
    Steer,
    /// 排队：回合收口后开新回合消费。
    Queued,
}

impl QueueKind {
    /// 事件 kind 的段名（`turn.steer.enqueued` / `turn.queued.enqueued`）。
    fn segment(self) -> &'static str {
        match self {
            QueueKind::Steer => "steer",
            QueueKind::Queued => "queued",
        }
    }
}

/// 一条待消费项（文本已脱敏）。
pub(crate) struct QueuedItem {
    /// 入队时的调用方标识（消费时与活跃 owner 比对）。
    pub owner_request_id: String,
    /// 取用时机（步边界 / 回合边界）。
    pub kind: QueueKind,
    /// 脱敏后的文本。
    pub text: String,
    /// 入队时刻（毫秒）。
    pub enqueued_at_ms: u64,
}

impl QueuedItem {
    /// 事件 payload（D5 键名定死）：`ownerRequestId` / `kind` / `text` /
    /// `enqueuedAtMs`。
    fn payload(&self) -> serde_json::Value {
        json!({
            "ownerRequestId": self.owner_request_id,
            "kind": self.kind.segment(),
            "text": self.text,
            "enqueuedAtMs": self.enqueued_at_ms,
        })
    }
}

/// 会话投递队列：待消费项（按入队序）+ 当前活跃 owner。
#[derive(Default)]
pub(crate) struct TurnQueue {
    /// 会话 → 待消费队列项（按入队序）。
    pending: Mutex<HashMap<Uuid, VecDeque<QueuedItem>>>,
    /// 会话 → 当前活跃 owner_request_id。
    owner: Mutex<HashMap<Uuid, String>>,
}

/// 进程级队列单例（见模块头「队列的挂载点」的偏离说明）。
static QUEUE: OnceLock<TurnQueue> = OnceLock::new();

impl TurnQueue {
    /// 进程级队列单例。
    fn global() -> &'static TurnQueue {
        QUEUE.get_or_init(TurnQueue::default)
    }

    /// 记活跃 owner（最近一次带 owner 的投递）。
    fn set_owner(&self, id: Uuid, owner_request_id: &str) {
        self.owner
            .lock()
            .expect("owner 锁不得毒化")
            .insert(id, owner_request_id.to_string());
    }

    /// 当前活跃 owner；从未有过带 owner 的投递 ⇒ `None`。
    fn active_owner(&self, id: Uuid) -> Option<String> {
        self.owner
            .lock()
            .expect("owner 锁不得毒化")
            .get(&id)
            .cloned()
    }

    /// 入队一项。
    fn enqueue(&self, id: Uuid, item: QueuedItem) {
        self.pending
            .lock()
            .expect("队列锁不得毒化")
            .entry(id)
            .or_default()
            .push_back(item);
    }

    /// 取走某类的全部待消费项（保持入队序）；另一类的项原样留在队列里。
    fn take_kind(&self, id: Uuid, kind: QueueKind) -> Vec<QueuedItem> {
        let mut guard = self.pending.lock().expect("队列锁不得毒化");
        let Some(items) = guard.get_mut(&id) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        let mut kept = VecDeque::new();
        for item in items.drain(..) {
            if item.kind == kind {
                taken.push(item);
            } else {
                kept.push_back(item);
            }
        }
        *items = kept;
        taken
    }

    /// 过期判定：该项的 owner 与当前活跃 owner 不一致 ⇒ 过期。
    ///
    /// fail-closed 的一侧：判过期就丢弃（调用方记证据），绝不「不一致也照跑」。
    fn is_expired(&self, id: Uuid, owner_request_id: &str) -> bool {
        self.active_owner(id).as_deref() != Some(owner_request_id)
    }
}

impl Runtime {
    /// 投递队列句柄（进程级单例）。
    fn queue(&self) -> &'static TurnQueue {
        TurnQueue::global()
    }

    /// **插话投递**（steer）：回合进行中投递 ⇒ 在**下一步边界**注入。
    ///
    /// 非阻塞：不取会话投递锁（回合正在跑时也不等），入队即返回。
    pub fn send_steer(
        &self,
        id: Uuid,
        owner_request_id: &str,
        text: &str,
    ) -> Result<(), CoreError> {
        self.enqueue_delivery(id, owner_request_id, text, QueueKind::Steer)
    }

    /// **排队投递**（queue）：回合进行中或已结束投递 ⇒ 在**回合收口后**开新回合消费。
    ///
    /// 非阻塞：不取会话投递锁，入队即返回。
    pub fn send_queued(
        &self,
        id: Uuid,
        owner_request_id: &str,
        text: &str,
    ) -> Result<(), CoreError> {
        self.enqueue_delivery(id, owner_request_id, text, QueueKind::Queued)
    }

    /// 带 owner 的文本投递：`send_text` 的 owner 版本。
    ///
    /// 先记活跃 owner（此后本会话的排队项按新 owner 判过期），再跑回合。
    pub fn send_text_as(
        &self,
        id: Uuid,
        owner_request_id: &str,
        text: &str,
    ) -> Result<TurnReceipt, CoreError> {
        self.queue().set_owner(id, owner_request_id);
        self.send_text(id, text)
    }

    /// 入队一条投递：脱敏 → 记活跃 owner → 入队 → 落 `turn.<kind>.enqueued`。
    fn enqueue_delivery(
        &self,
        id: Uuid,
        owner_request_id: &str,
        text: &str,
        kind: QueueKind,
    ) -> Result<(), CoreError> {
        let session = self.get_session(id)?;
        // 脱敏只做一次：入队文本落库前先过既有脱敏器（与 `send_text` 同口径）。
        let redacted = self.redact(text);
        let item = QueuedItem {
            owner_request_id: owner_request_id.to_string(),
            kind,
            text: redacted,
            enqueued_at_ms: self.turn_ts(),
        };
        // 活跃 owner = 最近一次带 owner 的投递。
        self.queue().set_owner(id, owner_request_id);
        let payload = item.payload();
        self.queue().enqueue(id, item);
        self.emit(
            &session,
            &format!("turn.{}.enqueued", kind.segment()),
            payload,
        )?;
        Ok(())
    }

    /// 回合边界取队列项（**queue 的消费点**）：过期项丢弃并记证据，活项返回。
    ///
    /// 活项**不在此处**记消费证据：证据由 [`Runtime::run_delivered_turn`] 在
    /// 消息**确实落盘**后才发（没落盘 ⇒ 该项不算被消费）。
    fn take_queued(&self, id: Uuid, events: &mut Vec<Event>) -> Result<Vec<QueuedItem>, CoreError> {
        let items = self.queue().take_kind(id, QueueKind::Queued);
        let mut live = Vec::new();
        for item in items {
            if self.queue().is_expired(id, &item.owner_request_id) {
                // 过期：丢弃 + 记可查证据（不静默丢弃、不执行）。
                let session = self.get_session(id)?;
                events.push(self.emit(&session, "turn.queue.expired", item.payload())?);
                continue;
            }
            live.push(item);
        }
        Ok(live)
    }

    /// 步边界取插话项（**steer 的消费点**）：注入进会话历史 ⇒ 下一步可见。
    ///
    /// 注入方式与 `send_text` 同款（落 user 消息 + 发 `user` 事件）：消息进
    /// 历史后，下一步重读历史时就能看到这条新消息（首步用的是回合前快照，
    /// 故插话最早在**第二步**可见——这正是「下一步边界」的语义）。
    pub(super) fn consume_steer(&self, id: Uuid, events: &mut Vec<Event>) -> Result<(), CoreError> {
        let items = self.queue().take_kind(id, QueueKind::Steer);
        for item in items {
            if self.queue().is_expired(id, &item.owner_request_id) {
                // 过期：丢弃 + 记可查证据（不静默丢弃、不执行）。
                let session = self.get_session(id)?;
                events.push(self.emit(&session, "turn.queue.expired", item.payload())?);
                continue;
            }
            // 消息先落、事件随后（与 `send_text` 同序）：历史与事件面对齐。
            let ts = self.turn_ts();
            let session = self.push_message(id, Role::User, item.text.clone(), None, ts)?;
            events.push(self.emit(&session, "user", json!({ "text": item.text, "tsMs": ts }))?);
            events.push(self.emit(&session, "turn.steer.consumed", item.payload())?);
        }
        Ok(())
    }

    /// 投递面：落 user 消息 + 发 user 事件 + 跑回合。
    ///
    /// 回合边界循环：每轮先取**回合边界队列项**（queue 的消费点），再跑本轮
    /// 输入；跑完若又来了排队项，下一轮继续开新回合消费——这正是「回合收口后
    /// 自动开新回合」。调用方输入只跑一次（第一轮），其后各轮跑的都是排队项。
    pub(super) fn deliver_and_run(&self, id: Uuid, text: &str) -> Result<TurnReceipt, CoreError> {
        let mut events: Vec<Event> = Vec::new();
        let mut last_seq = 0u64;
        let mut caller_input = Some(text.to_string());
        loop {
            // 排队项 ⇒ (文本, 消费证据 payload)；调用方输入没有排队证据。
            let mut batch: Vec<(String, Option<serde_json::Value>)> = self
                .take_queued(id, &mut events)?
                .into_iter()
                .map(|item| {
                    let payload = item.payload();
                    (item.text, Some(payload))
                })
                .collect();
            if let Some(input) = caller_input.take() {
                batch.push((input, None));
            }
            if batch.is_empty() {
                break;
            }
            for (input, consumed) in batch {
                let receipt = self.run_delivered_turn(id, &input, consumed)?;
                events.extend(receipt.events);
                last_seq = receipt.last_seq;
            }
        }
        Ok(TurnReceipt {
            session_id: id,
            events,
            last_seq,
        })
    }

    /// 跑一个回合：落 user 消息 → 发 `user` 事件 → 跑回合政策层。
    ///
    /// 首步 history 取本轮输入推入之前的快照（本轮输入只作为 user_text 出现）。
    /// 脱敏只做一次：`emit` 内部还会再过一次 `redact_json`，若此处先脱敏，
    /// 事件 text 与历史消息 content 才逐字同一（否则敏感片段被改两次）。
    /// 一次时钟读数同时用于消息与事件 payload：消息时间因此随事件落盘
    /// （`replay_message` 优先取 `payload.tsMs`），重启前后逐条相等。
    ///
    /// `consumed` 是排队项的消费证据：**消息确实落盘之后**才发（没落盘 ⇒ 该项
    /// 不算被消费，不发证据），且排在 `user` 事件之后、回合事件之前。
    fn run_delivered_turn(
        &self,
        id: Uuid,
        text: &str,
        consumed: Option<serde_json::Value>,
    ) -> Result<TurnReceipt, CoreError> {
        let history = self.get_session(id)?.messages;
        let redacted = self.redact(text);
        let ts = self.turn_ts();
        let session = self.push_message(id, Role::User, redacted.clone(), None, ts)?;
        // 用户输入本身也是一条 durable 事件（金标 `session-event` 的 user 消息投影：
        // `InboxDelivered` 是会话事件面上的首条，投影出 user 消息）。顺序：消息先落、
        // 事件随后，且事件必须是回合事件面的第一条，否则 receipt ≡ 持久日志会破。
        let event = self.emit(&session, "user", json!({ "text": redacted, "tsMs": ts }))?;
        let consumed = consumed
            .map(|payload| self.emit(&session, "turn.queued.consumed", payload))
            .transpose()?;
        let mut receipt = self.run_turn(id, text, &history, event)?;
        if let Some(consumed) = consumed {
            // 证据插在 `user` 事件之后（与落盘顺序一致）。
            receipt.events.insert(1, consumed);
        }
        Ok(receipt)
    }
}
