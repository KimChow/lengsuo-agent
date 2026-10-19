//! host 侧分派状态机：入站帧的归类语义与计数（`host.rs` 的私有内核）。
//!
//! 与 [`crate::router`] 的 `RouteState` **逐条同语义**，差异只在角色：
//! host 不发起请求，故没有 pending 表；入站响应计 `unexpected_responses`。
//! 帧归类表见 [`crate::host`] 模块头（两处必须一致）。
//!
//! 之所以独立成文件：这是「谁被 spawn 的一端」里最容易读错的一半——写侧与
//! 生命周期留在 [`crate::host`]，读侧与计数集中在此。语义仍是单文件可读的
//! 一张表，不额外分层。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde::Serialize;
use serde_json::{json, Value};

use crate::codec::encode_frame;
use crate::control::{
    ControlNotification, ControlRequest, ControlResponse, ControlResponseTag, TopLevel,
};
use crate::error::Error;
use crate::message::JsonRpcNotification;
use crate::message::{
    JsonRpcErrorObject, JsonRpcRequest, JsonRpcResponse, Message, RequestId,
};
use crate::router::{ControlHandler, RpcHandler, RpcOutcome, ACP_ERROR_CONNECTION_CLOSED};
use crate::transport::{FrameSink, FrameSource, SourceEvent, SourcePull};

/// 分派线程轮询间隔（同时决定 close 的响应延迟上界）。
pub(crate) const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// 默认事件队列上限（与 [`crate::client::DEFAULT_MAX_EVENTS`] 同值）。
pub const DEFAULT_MAX_EVENTS: usize = 1024;

/// host 侧的帧顶层归属：分派线程算出来、上层（如网关）可观测的归类结果。
///
/// 只报**非应答**的入站帧——`Rpc::Request` 与已认领的 `ControlRequest` 在分派
/// 线程内就地应答，观测量是应答本身（对端可见），不重复入队。
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    /// 入站通知（如 `session/cancel`，回 [`crate::client::ClientEvent::Notification`]）。
    Notification(JsonRpcNotification),
    /// control 通知信封。
    ControlNotification(ControlNotification),
    /// 未注册 subtype 的 control 请求，待 [`crate::host::Host::respond_control`] 延后应答。
    ControlRequest(ControlRequest),
    /// 入站响应：host 不发起请求，故为对端错帧/迟到帧（已计数，仅作观测）。
    UnexpectedResponse(JsonRpcResponse),
}

/// [`crate::host::Host::next_event`] 的结果（与 [`crate::client::NextEvent`] 同形）。
#[derive(Debug, Clone, PartialEq)]
pub enum NextEvent {
    /// 取到一个事件。
    Event(HostEvent),
    /// 超时内无事件（连接仍可用）。
    Idle,
    /// 连接已终止（事件队列排空后才报此态）。
    Closed,
}

/// host 分派线程共享状态：写侧 sink、handler 表、事件队列与计数器。
pub(crate) struct HostRouteState {
    pub sink: Mutex<Box<dyn FrameSink>>,
    pub events: Mutex<VecDeque<HostEvent>>,
    pub event_cv: Condvar,
    pub max_events: usize,
    pub handlers: Mutex<HashMap<String, Arc<RpcHandler>>>,
    pub control_handlers: Mutex<HashMap<&'static str, Arc<ControlHandler>>>,
    pub stop: AtomicBool,
    pub closed: AtomicBool,
    pub closed_reason: Mutex<Option<String>>,
    pub dropped_events: AtomicU64,
    pub bad_frames: AtomicU64,
    pub frames_seen: AtomicU64,
    pub unexpected_responses: AtomicU64,
}

impl std::fmt::Debug for HostRouteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostRouteState")
            .field("closed", &self.closed.load(Ordering::Acquire))
            .field("max_events", &self.max_events)
            .field(
                "dropped_events",
                &self.dropped_events.load(Ordering::Acquire),
            )
            .field("bad_frames", &self.bad_frames.load(Ordering::Acquire))
            .field(
                "unexpected_responses",
                &self.unexpected_responses.load(Ordering::Acquire),
            )
            .finish()
    }
}

impl HostRouteState {
    pub fn new(sink: Box<dyn FrameSink>, max_events: usize) -> Self {
        Self {
            sink: Mutex::new(sink),
            events: Mutex::new(VecDeque::new()),
            event_cv: Condvar::new(),
            max_events: max_events.max(1),
            handlers: Mutex::new(HashMap::new()),
            control_handlers: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            closed_reason: Mutex::new(None),
            dropped_events: AtomicU64::new(0),
            bad_frames: AtomicU64::new(0),
            frames_seen: AtomicU64::new(0),
            unexpected_responses: AtomicU64::new(0),
        }
    }

    /// 连接可用（未终止）才可写。
    pub fn usable(&self) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) {
            let reason = self
                .closed_reason
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_else(|| "连接已终止".into());
            return Err(Error::Transport { reason });
        }
        Ok(())
    }

    /// 编码一帧并写入 sink（写锁覆盖编码+写，保证帧字节序与写序一致）。
    pub fn write_frame<T: Serialize>(&self, value: &T) -> Result<(), Error> {
        self.usable()?;
        let frame = encode_frame(value)?;
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_frame(&frame)
    }

    /// 入队事件：满时丢最旧并计数（fail-closed：保护内存，不静默吞计数）。
    pub fn push_event(&self, ev: HostEvent) {
        let mut q = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= self.max_events {
            let _ = q.pop_front();
            self.dropped_events.fetch_add(1, Ordering::AcqRel);
        }
        q.push_back(ev);
        drop(q);
        self.event_cv.notify_all();
    }

    /// 连接终止：置 closed 与原因，唤醒等待者。幂等。
    pub fn terminate(&self, reason: Option<String>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        *self.closed_reason.lock().unwrap_or_else(|e| e.into_inner()) = reason;
        self.event_cv.notify_all();
    }

    /// 归类处理一帧（分派语义表见模块头）。
    pub fn handle_top(self: &Arc<Self>, top: TopLevel) {
        match top {
            TopLevel::Rpc(Message::Request(req)) => self.handle_inbound_request(req),
            TopLevel::Rpc(Message::Notification(n)) => {
                self.push_event(HostEvent::Notification(n));
            }
            TopLevel::Rpc(Message::Response(resp)) => {
                self.unexpected_responses.fetch_add(1, Ordering::AcqRel);
                self.push_event(HostEvent::UnexpectedResponse(resp));
            }
            TopLevel::ControlRequest(req) => {
                let handler = self
                    .control_handlers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(req.request.subtype.as_str())
                    .cloned();
                match handler {
                    Some(h) => {
                        let outcome = h(&req);
                        let _ = self.respond_control(&req.request_id, outcome);
                    }
                    None => self.push_event(HostEvent::ControlRequest(req)),
                }
            }
            TopLevel::ControlNotification(n) => {
                self.push_event(HostEvent::ControlNotification(n));
            }
            // 本端不发起 control 请求：对端的 control_response 无 pending 可完成。
            TopLevel::ControlResponse(_) => {
                self.unexpected_responses.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    /// 入站 JSON-RPC 请求：命中 handler 则同步执行回响应；未命中自动回
    /// `-32601`，绝不应答失败后悬挂（也不静默丢弃）。
    fn handle_inbound_request(self: &Arc<Self>, req: JsonRpcRequest) {
        let handler = self
            .handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&req.method)
            .cloned();
        let outcome = match handler {
            Some(h) => h(&req),
            None => RpcOutcome::Err(JsonRpcErrorObject {
                code: crate::message::JSONRPC_ERROR_METHOD_NOT_FOUND,
                message: "host does not handle this method".into(),
                data: Some(Value::String(req.method.clone())),
            }),
        };
        let resp = match outcome {
            Ok(v) => Message::Response(JsonRpcResponse::ok(req.id.clone(), v)),
            Err(e) => Message::Response(JsonRpcResponse::err(req.id.clone(), e)),
        };
        let _ = self.write_frame(&resp);
    }

    /// 回一条 control 应答信封：Ok → 载荷本体；Err → `{error:{…}}` 载荷。
    pub fn respond_control(
        &self,
        request_id: &RequestId,
        outcome: RpcOutcome,
    ) -> Result<(), Error> {
        let response = match outcome {
            Ok(v) => v,
            Err(e) => json!({ "error": e }),
        };
        let env = ControlResponse {
            kind: ControlResponseTag::ControlResponse,
            request_id: request_id.clone(),
            response,
        };
        self.write_frame(&env)
    }

    /// 直接处理一个源事件（[`dispatch_loop`] 与 [`Host::from_bytes`] 共用的
    /// 归类入口：帧归类 / 畸形帧计数 / 终止）。
    pub fn handle_source_event(self: &Arc<Self>, ev: SourceEvent) {
        self.frames_seen.fetch_add(1, Ordering::AcqRel);
        match ev {
            SourceEvent::Frame(top) => self.handle_top(top),
            SourceEvent::BadFrame { .. } => {
                self.bad_frames.fetch_add(1, Ordering::AcqRel);
            }
            SourceEvent::Closed { reason } => self.terminate(reason),
        }
    }
}

/// 分派线程主循环：拉帧事件直到 Closed 或 stop 标志。
///
/// 帧源在 host 侧是 [`SourceEvent`] 流（与 [`crate::client`] 同一套
/// [`crate::transport`] 抽象）；**畸形帧按事件计数，不触发 `LineDecoder` 的
/// fail-closed**——`LineDecoder` 只在真实字节流（[`crate::stdio`]）里做行切分，
/// 流内超长行在那里已按协议终止连接，语义未被放宽。
pub(crate) fn dispatch_loop(mut source: Box<dyn FrameSource>, state: Arc<HostRouteState>) {
    while !state.stop.load(Ordering::Acquire) {
        match source.next_event(POLL_INTERVAL) {
            SourcePull::Timeout => continue,
            // 帧归类 / 畸形帧计数 / 终止三条路径与 from_bytes 共用一处实现。
            SourcePull::Event(ev) => {
                let closed = matches!(ev, SourceEvent::Closed { .. });
                state.handle_source_event(ev);
                if closed {
                    return;
                }
            }
        }
    }
    // stop 主动拆除：也算终止，唤醒等待者。
    state.terminate(None);
}

/// 连接终止时写侧拒绝所用的私有错误码（与 [`crate::router`] 同值，供上层把
/// 「连接已终止」与业务错误区分开）。
pub const ACP_ERROR_HOST_CONNECTION_CLOSED: i64 = ACP_ERROR_CONNECTION_CLOSED;
