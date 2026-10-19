//! host 侧写侧与生命周期：公开角色 [`Host`] 与拆解（`host.rs` 的私有内核之一）。
//!
//! 读侧归类在 [`crate::host_router`]，本文件只管：接管传输、方法表 / control 回调
//! 注册、主动通知、事件出队、计数器读数、拆除。**公开文档在 [`crate::host`]**，
//! 这里只保留逐方法的契约注释。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::codec::LineDecoder;
use crate::control::ControlRequest;
use crate::host::{HostConfig, HostEvent, NextEvent};
use crate::host_router::{dispatch_loop, HostRouteState};
use crate::message::{JsonRpcNotification, JsonRpcRequest, Message, RequestId};
use crate::router::{ControlHandler, RpcHandler, RpcOutcome};
use crate::transport::{SourceEvent, Teardown, Transport};

impl Host {
    /// 接管一条传输：启动分派线程，返回 host。
    #[must_use]
    pub fn new(transport: Transport) -> Self {
        Self::with_config(transport, HostConfig::default())
    }

    /// 带配置接管传输。
    pub fn with_config(transport: Transport, config: HostConfig) -> Self {
        let Transport {
            sink,
            source,
            teardown,
        } = transport;
        let state = Arc::new(HostRouteState::new(sink, config.max_events));
        let dispatcher = std::thread::Builder::new()
            .name("acp-host".into())
            .spawn({
                let state = Arc::clone(&state);
                move || dispatch_loop(source, state)
            })
            .expect("分派线程总能启动");
        Self {
            state,
            next_number: AtomicI64::new(1),
            max_line_bytes: config.max_line_bytes,
            dispatcher: Mutex::new(Some(dispatcher)),
            teardown: Mutex::new(Some(teardown)),
        }
    }

    /// 从一行帧字节建 host：字节经 [`LineDecoder`] 切行、[`crate::codec::decode_frame`]
    /// 归类，畸形帧计数不断连——即「被 spawn 的一端从 stdin 读帧」的最短形态。
    ///
    /// # Errors
    /// [`Error::FrameTooLarge`] / [`Error::Broken`]：行超 [`HostConfig::max_line_bytes`]
    /// 上限（与 [`crate::stdio`] 同款 fail-closed）。
    pub fn from_bytes(&self, bytes: &[u8]) -> crate::Result<u64> {
        let mut decoder = LineDecoder::new(self.max_line_bytes());
        let frames = decoder.feed(bytes)?;
        for body in frames {
            let ev = match crate::codec::decode_frame(&body) {
                Ok(top) => SourceEvent::Frame(top),
                Err(e) => SourceEvent::BadFrame {
                    reason: e.to_string(),
                },
            };
            self.state.handle_source_event(ev);
        }
        Ok(self.frames_seen())
    }

    /// 单行帧上限。
    #[must_use]
    pub fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// 注册入站 JSON-RPC 方法 handler（同名重复注册 = 覆盖，对齐
    /// [`crate::client::Client::on_request`]）。
    pub fn register_method(
        &self,
        name: impl Into<String>,
        handler: impl Fn(&JsonRpcRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        let arc: Arc<RpcHandler> = Arc::new(handler);
        self.state
            .handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.into(), arc);
    }

    /// 注册 control subtype handler；subtype 用其 wire 拼写（`Unknown` 拒绝注册，
    /// 未认领请求走事件队列）。
    pub fn on_control(
        &self,
        subtype: crate::control::ControlSubtype,
        handler: impl Fn(&ControlRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        use crate::control::ControlSubtype;
        if subtype == ControlSubtype::Unknown {
            return; // Unknown 无 wire 拼写可还原，注册它等于死代码。
        }
        let arc: Arc<ControlHandler> = Arc::new(handler);
        self.state
            .control_handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subtype.as_str(), arc);
    }

    /// 对未认领的 control 请求延后应答。
    ///
    /// # Errors
    /// [`Error::Transport`]：连接已终止或写侧失败。
    pub fn respond_control(
        &self,
        request_id: &RequestId,
        outcome: RpcOutcome,
    ) -> crate::Result<()> {
        self.state.respond_control(request_id, outcome)
    }

    /// 主动推一条 JSON-RPC 通知（host 侧与 client 侧最本质的差别：能推
    /// `session/update` 一类增量）。
    ///
    /// # Errors
    /// [`Error::Transport`]：序列化失败或连接已终止/写侧失败。
    pub fn notify(&self, method: impl Into<String>, params: Value) -> crate::Result<()> {
        self.state
            .write_frame(&Message::Notification(JsonRpcNotification::new(
                method, params,
            )))
    }

    /// 自动编号请求 id（host 用于自报用；ACP host 面通常不主动发请求）。
    #[must_use]
    pub fn next_id(&self) -> RequestId {
        RequestId::Number(self.next_number.fetch_add(1, Ordering::AcqRel))
    }

    /// 阻塞取下一个事件，最多 `timeout`。
    #[must_use]
    pub fn next_event(&self, timeout: Duration) -> NextEvent {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let mut q = self.state.events.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(ev) = q.pop_front() {
                    return NextEvent::Event(ev);
                }
                if self.state.closed.load(Ordering::Acquire) {
                    return NextEvent::Closed;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return NextEvent::Idle;
                }
                let (guard, _wait) = self
                    .state
                    .event_cv
                    .wait_timeout(q, remaining)
                    .unwrap_or_else(|e| e.into_inner());
                drop(guard);
            }
            // 唤醒或超时后重查队列（wait_timeout 可能虚假唤醒）。
        }
    }

    /// 非阻塞取下一个事件（队列空 → `None`）。
    pub fn poll_event(&self) -> Option<HostEvent> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    /// 连接是否已终止。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.state.closed.load(Ordering::Acquire)
    }

    /// 终止原因（若有）。
    #[must_use]
    pub fn closed_reason(&self) -> Option<String> {
        self.state
            .closed_reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 畸形帧（字节流仍对齐）计数。
    #[must_use]
    pub fn bad_frame_count(&self) -> u64 {
        self.state.bad_frames.load(Ordering::Acquire)
    }

    /// 已处理的源事件计数（帧 + 畸形帧 + 终止）。
    #[must_use]
    pub fn frames_seen(&self) -> u64 {
        self.state.frames_seen.load(Ordering::Acquire)
    }

    /// 因队列满被丢弃的最旧事件数。
    #[must_use]
    pub fn dropped_event_count(&self) -> u64 {
        self.state.dropped_events.load(Ordering::Acquire)
    }

    /// 入站响应计数（host 不发起请求，故为对端错帧/迟到帧）。
    #[must_use]
    pub fn unexpected_response_count(&self) -> u64 {
        self.state.unexpected_responses.load(Ordering::Acquire)
    }
}

/// 被 spawn 的一端：ACP host/server 角色（同步线程模型）。
///
/// 公开语义见 [`crate::host`] 模块头；本类型的逐方法契约见下方 `impl` 注释。
#[derive(Debug)]
pub struct Host {
    state: Arc<HostRouteState>,
    next_number: AtomicI64,
    max_line_bytes: usize,
    dispatcher: Mutex<Option<std::thread::JoinHandle<()>>>,
    teardown: Mutex<Option<Teardown>>,
}

impl Drop for Host {
    /// 主动拆除：停分派线程 → 关写侧 → 执行传输拆除钩子（一次）。幂等。
    fn drop(&mut self) {
        self.stop();
    }
}

impl Host {
    /// 主动拆除（幂等）：停分派线程 → 关写侧 → 执行传输拆除钩子。
    pub fn stop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        {
            let mut sink = self.state.sink.lock().unwrap_or_else(|e| e.into_inner());
            sink.close();
        }
        let handle = self
            .dispatcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(h) = handle {
            // 分派线程每 poll_interval 醒一次查 stop，故内置帧源（stdio/mock）下
            // 该 join 有界——与 Client::close 同款取舍。
            let _ = h.join();
        }
        let hook = self
            .teardown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mut hook) = hook {
            hook.run();
        }
    }
}
