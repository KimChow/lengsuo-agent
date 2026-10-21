//! 网关 ↔ ACP host 的接线点（W2）。
//!
//! 本模块只做**持有并驱动**：网关按 [`GatewayConfig`] 起一个
//! [`agent_acp::host::Host`]（「被 spawn 的一端」由桌面壳/网关拉起），
//! 暴露 `register_method` 与 `notify` 两个入口（外加 control 回调与拆除）。
//!
//! **本卡补上的部分**：`AcpHostHandle::forward_action` —— 把**已冻结的 v0 动作名**
//! （[`crate::rpc`] 的 `域:动作` 面）**原样**当作 ACP method 投给 host 方法表，
//! 取回 [`RpcOutcome`] 并转成网关信封形状。**不做任何名字改写**——不发明
//! 「`域:动作` ↔ `session/*`」映射表（那套命名未冻结，见 [`crate::rpc`] 的旁路说明）。
//! 未在 host 注册过的动作走 ACP 的 `-32601 method not found` 语义，不 panic。
//! 一条连接上**一次只等一条**转发应答，**超时即撤销在途** ⇒ 一次超时不会把该连接的转发能力永久废掉。
//!
//! **host 基线**：生产入口 [`AcpHostHandle::from_gateway_config`] 默认注册
//! `initialize` / `logout` 与转发探针 `sys:ping`；[`AcpHostHandle::start`] 则按调用方
//! 传入的 [`HostConfig`] 原样起，**不做隐式注册**（见 [`agent_acp::host_baseline`]）。
//!
//! **仍未做的部分（诚实登记）**：
//! - 传输仍是宿主注入的 [`Transport`]（测试用 `mock_transport()`），网关自己不
//!   spawn 子进程、不自建管道；真实形态（谁 spawn 谁）需用户决策，见模块尾注。
//!
//! 形态对齐 [`crate::server::GatewayHandle`]：`Arc` 共享 + `Debug` 视图 +
//! 显式 `stop`（`Drop` 亦自动拆除，`Host` 自身幂等）。

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_acp::control::TopLevel;
use agent_acp::host::{Host, HostConfig, HostEvent, NextEvent};
use agent_acp::host_baseline::{register_baseline, register_ping, BaselineConfig};
use agent_acp::message::{
    JsonRpcErrorObject, JsonRpcRequest, Message, RequestId, JSONRPC_ERROR_INVALID_REQUEST,
    JSONRPC_ERROR_METHOD_NOT_FOUND,
};
use agent_acp::router::RpcOutcome;
use agent_acp::transport::{FrameSink, FrameSource, SourceEvent, SourcePull, Transport};
use agent_acp::ControlSubtype;
use serde_json::{json, Value};

use crate::server::GatewayConfig;

/// 单次转发的默认超时：ACP host 的应答由对端分派线程写出，正常在毫秒级；
/// 对端长时间不回时给网关一个上界，避免请求线程被无限期挂住。
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

/// 转发旁路的应答（已转成网关信封的可观测形态）。
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardOutcome {
    /// 对端返回的 JSON-RPC `result`（成功时）。
    pub result: Option<Value>,
    /// 对端返回的 JSON-RPC `error` 对象（`-32601` 等；失败时）。
    pub error: Option<JsonRpcErrorObject>,
    /// 转成网关信封形状的载荷：成功即 `result` 本体，失败即 `{"error":{…}}`，
    /// 用于直接塞进 `code=0` 的 `data` 字段（`rpc.rs` 的旁路取其本体）。
    pub envelope: Value,
}

impl ForwardOutcome {
    /// 是否是对端方法表未命中（ACP `-32601`）。
    #[must_use]
    pub fn is_method_not_found(&self) -> bool {
        self.error
            .as_ref()
            .is_some_and(|e| e.code == JSONRPC_ERROR_METHOD_NOT_FOUND)
    }

    /// 成功则取 `result` 本体；失败返回 `None`。
    #[must_use]
    pub fn ok(self) -> Option<Value> {
        if self.error.is_none() {
            self.result
        } else {
            None
        }
    }
}

/// 网关持有的 ACP host 句柄（`Clone`，内部共享同一 host）。
#[derive(Clone)]
pub struct AcpHostHandle {
    inner: Arc<Host>,
    /// 与接收线程共享的转发配对状态（发出请求 id ↔ 等应答）。
    responder: Arc<Mutex<Option<ResponsePair>>>,
    /// 转发请求的写侧：与 host 共用同一条帧汇（server 注入的 sink 自带写锁）。
    sink: Arc<Mutex<Box<dyn FrameSink>>>,
    /// 转发应答接收端（接收线程投递；`forward_action` 取用）。
    response_rx: Arc<Mutex<std::sync::mpsc::Receiver<RpcOutcome>>>,
    /// 转发请求 id 的单调计数器（与 `Host::next_id` 分开，避免两处编号混用）。
    next_forward_id: Arc<AtomicI64>,
}

/// 接收线程与 [`AcpHostHandle::forward_action`] 之间的配对状态：
/// 一个 pending 请求 id 与回执投递端，外加把回执送回 host 分派线程的通道端。
struct ResponsePair {
    /// 等待应答的转发请求 id（`None` = 无在途）。
    pending: Option<RequestId>,
    /// 应答投递端（转发动作取用）。
    response_tx: std::sync::mpsc::Sender<RpcOutcome>,
}

impl ResponsePair {
    fn new(response_tx: std::sync::mpsc::Sender<RpcOutcome>) -> Self {
        Self {
            pending: None,
            response_tx,
        }
    }
}

/// host 侧写侧的共享包装：与 [`AcpHostHandle`] 的转发写侧指向同一帧汇，
/// 写锁由帧汇自身的 `Mutex` 提供（锁覆盖编码+写，帧字节序即写序）。
struct HostSink {
    sink: Arc<Mutex<Box<dyn FrameSink>>>,
}

impl FrameSink for HostSink {
    fn send_frame(&mut self, frame: &[u8]) -> agent_acp::Result<()> {
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_frame(frame)
    }

    fn close(&mut self) {
        self.sink.lock().unwrap_or_else(|e| e.into_inner()).close();
    }
}

/// 接收线程：从原帧源拉帧，截住本端转发请求的响应，其余原样转投 host。
///
/// 之所以要有这条线程：host 侧**不发起请求**（无 pending 表），故转发请求的
/// 响应必须由本端自己认领；而 host 分派线程必须先拿到 `-32601` 才能替未注册
/// 方法应答，故非转发帧仍要走 host 方法表。
fn spawn_receiver(
    mut source: Box<dyn FrameSource>,
    events_tx: std::sync::mpsc::Sender<SourceEvent>,
    responder: Arc<Mutex<Option<ResponsePair>>>,
    inner: Arc<Host>,
) {
    let _ = std::thread::Builder::new()
        .name("acp-host-recv".into())
        .spawn(move || {
            while !inner.is_closed() {
                let ev = match source.next_event(FORWARD_TIMEOUT) {
                    SourcePull::Timeout => continue,
                    SourcePull::Event(ev) => ev,
                };
                // 只截本端发起的请求的响应；其余（请求/通知/control/畸形/终止）
                // 原样转投 host 方法表与事件队列。
                let SourceEvent::Frame(TopLevel::Rpc(Message::Response(resp))) = &ev else {
                    if events_tx.send(ev).is_err() {
                        break;
                    }
                    continue;
                };
                let outcome = match (&resp.error, &resp.result) {
                    (Some(e), _) => RpcOutcome::Err(e.clone()),
                    (None, Some(r)) => RpcOutcome::Ok(r.clone()),
                    (None, None) => RpcOutcome::Err(JsonRpcErrorObject {
                        code: JSONRPC_ERROR_INVALID_REQUEST,
                        message: "响应既无 result 也无 error".into(),
                        data: None,
                    }),
                };
                let claim = {
                    let mut guard = responder.lock().unwrap_or_else(|e| e.into_inner());
                    match guard.as_mut() {
                        Some(pair) if pair.pending.as_ref() == Some(&resp.id) => {
                            pair.pending = None;
                            Some(pair.response_tx.clone())
                        }
                        _ => None,
                    }
                };
                // 非本端转发请求的响应（对端错帧/迟到帧）：交回 host 计数并可观测（原语义不丢）。
                if let Some(tx) = claim {
                    let _ = tx.send(outcome);
                } else if events_tx.send(ev).is_err() {
                    break;
                }
            }
        });
}

impl fmt::Debug for AcpHostHandle {
    /// 只报可观测状态，不打印 handler 表（与 `GatewayHandle` 不收 token 同款）。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcpHostHandle")
            .field("closed", &self.inner.is_closed())
            .field("bad_frames", &self.inner.bad_frame_count())
            .finish()
    }
}

impl AcpHostHandle {
    /// 接管一条 ACP 传输，按配置起 host（分派线程即刻运行）。
    ///
    /// 原传输的**帧源**被换成一条内部事件通道：另有接收线程从原帧源拉帧并
    /// 转投该通道，途中把「本端转发请求的响应」截住配对应答（见
    /// [`AcpHostHandle::forward_action`]），其余帧原样透传给 host。
    /// 帧汇由 host 与本端转发**共用**（[`HostSink`] 包一层共享句柄 + 互斥锁），
    /// 故两处写不会交错半帧——锁覆盖编码+写，帧字节序即写序。
    #[must_use]
    pub fn start(transport: Transport, config: HostConfig) -> Self {
        let (source, events_tx) = agent_acp::transport::event_channel();
        let (response_tx, response_rx) = std::sync::mpsc::channel::<RpcOutcome>();
        // 帧汇由本端与 host 共享（server 注入的 sink 自带写锁）；本端持一份
        // 供转发写出，另一份随传输交给 host。
        let shared_sink: Arc<Mutex<Box<dyn FrameSink>>> = Arc::new(Mutex::new(transport.sink));
        let inner = Arc::new(Host::with_config(
            Transport::with_teardown(
                Box::new(HostSink {
                    sink: Arc::clone(&shared_sink),
                }),
                source,
                transport.teardown,
            ),
            config,
        ));
        let responder = Arc::new(Mutex::new(Some(ResponsePair::new(response_tx))));
        spawn_receiver(
            transport.source,
            events_tx,
            Arc::clone(&responder),
            Arc::clone(&inner),
        );
        Self {
            inner,
            responder,
            sink: shared_sink,
            response_rx: Arc::new(std::sync::Mutex::new(response_rx)),
            next_forward_id: Arc::new(AtomicI64::new(1)),
        }
    }

    /// 用 [`GatewayConfig`] 起 host：本卡的 `GatewayConfig` 不带 ACP 侧参数
    /// （路径/isolation 都是握手与运行时面），故取 [`HostConfig::default`]。
    /// 传输由调用方注入（网关不 spawn 子进程）。
    ///
    /// **与 [`AcpHostHandle::start`] 的默认值不同**：这里是网关启动 ACP host 的
    /// **生产入口**，要跟真实对端说话，故默认注册 host 基线（`initialize` /
    /// `logout`，协议版本取 [`BaselineConfig::default`]），并为白名单成员 `sys:ping`
    /// 注册应答实现（[`register_ping`]，回本进程 pid）——白名单开了这个动作，
    /// 网关自己的 host 就必须答得上，否则转发必然 `-32601`。`start` 接受调用方显式
    /// 传入的 [`HostConfig`]，因此**不加任何隐式注册**。理由见
    /// `docs/contracts/acp-host.md` §8.4。
    #[must_use]
    pub fn from_gateway_config(_config: &GatewayConfig, transport: Transport) -> Self {
        let handle = Self::start(transport, HostConfig::default());
        register_baseline(&handle.inner, &BaselineConfig::default());
        register_ping(&handle.inner);
        handle
    }

    /// 注册入站 ACP 方法 handler（同名重复注册 = 覆盖）。
    pub fn register_method(
        &self,
        name: impl Into<String>,
        handler: impl Fn(&JsonRpcRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        self.inner.register_method(name, handler);
    }

    /// 注册 control subtype handler（V.* 回调面）。
    pub fn register_control(
        &self,
        subtype: ControlSubtype,
        handler: impl Fn(&agent_acp::ControlRequest) -> RpcOutcome + Send + Sync + 'static,
    ) {
        self.inner.on_control(subtype, handler);
    }

    /// 主动向 ACP 对端推通知（如 `session/update`）。
    ///
    /// # Errors
    /// [`agent_acp::Error::Transport`]：连接已终止或写侧失败。
    pub fn notify(&self, method: impl Into<String>, params: Value) -> agent_acp::Result<()> {
        self.inner.notify(method, params)
    }

    /// 按**冻结动作面**转发：把 `action`（[`crate::rpc`] 的 `域:动作` 名，如
    /// `session:create`）**原样当作 ACP method** 投给 host 方法表，等对端应答
    /// 后返回 [`ForwardOutcome`]。
    ///
    /// 语义要点（见 `docs/contracts/acp-host.md` §6）：
    /// - **不发明映射表**：不做任何名字改写，v0 动作名即 ACP method 名；
    /// - **未注册动作**走 ACP 的 `-32601 method not found`（host 侧自动应答），
    ///   **不 panic、不静默丢弃**；调用方据 [`ForwardOutcome::is_method_not_found`] 判定；
    /// - 请求 id 由本端单调编号（转发用独立计数器），响应按 id 配对（host 侧不发请求）。
    ///
    /// # Errors
    /// - `pending`：同一条连接上已有转发在途（本端一次只等一条，避免响应错配）；
    /// - `transport`：连接已终止、写侧失败或对端在 [`FORWARD_TIMEOUT`] 内未回（超时即撤销在途）。
    ///
    /// [`ForwardOutcome::is_method_not_found`]: ForwardOutcome::is_method_not_found
    pub fn forward_action(
        &self,
        action: &str,
        payload: &Value,
    ) -> agent_acp::Result<ForwardOutcome> {
        let id = RequestId::Number(self.next_forward_id.fetch_add(1, Ordering::AcqRel));
        let request = JsonRpcRequest::new(id.clone(), action, payload.clone());
        // 先登记在途 id，再写出：否则对端足够快时响应先到、接收线程无处配对。
        {
            let mut guard = self.responder.lock().unwrap_or_else(|e| e.into_inner());
            let pair = guard.as_mut().ok_or_else(|| agent_acp::Error::Transport {
                reason: "ACP host 句柄已拆除，无法转发".into(),
            })?;
            if pair.pending.is_some() {
                return Err(agent_acp::Error::Transport {
                    reason: "已有转发在途，本端一次只等一条应答".into(),
                });
            }
            pair.pending = Some(id.clone());
        }
        let frame = agent_acp::codec::encode_frame(&Message::Request(request))?;
        let wrote = self.write_frame(&frame);
        if let Err(e) = wrote {
            // 写失败即撤销在途，避免后续转发被永久挡住。
            self.cancel_pending();
            return Err(e);
        }
        let outcome = self
            .response_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recv_timeout(FORWARD_TIMEOUT)
            .map_err(|_| {
                // 超时即撤销在途：否则本条转发永远占着槽位，后续转发会被「已有转发在途」一律拒掉。
                self.cancel_pending();
                agent_acp::Error::Transport {
                    reason: "ACP 对端未在超时内应答转发请求".into(),
                }
            })?;
        Ok(ForwardOutcome {
            envelope: match &outcome {
                RpcOutcome::Ok(v) => v.clone(),
                RpcOutcome::Err(e) => json!({ "error": e }),
            },
            result: match &outcome {
                RpcOutcome::Ok(v) => Some(v.clone()),
                RpcOutcome::Err(_) => None,
            },
            error: outcome.err(),
        })
    }

    /// 向帧汇写一帧已编码字节（转发路径用；与 host 侧共用同一 sink）。
    fn write_frame(&self, frame: &[u8]) -> agent_acp::Result<()> {
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send_frame(frame)
    }

    /// 撤销在途转发：清掉本请求占用的槽位（写帧失败 / 等应答超时都用它）。清槽后
    /// **迟到的应答配不上任何在途 id** ⇒ 交回 host 计数并入队（可观测、不静默吞掉）。
    fn cancel_pending(&self) {
        let mut guard = self.responder.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pair) = guard.as_mut() {
            pair.pending = None;
        }
    }

    /// 阻塞取下一个 host 事件（超时归 [`NextEvent::Idle`]）。
    #[must_use]
    pub fn next_event(&self, timeout: Duration) -> NextEvent {
        self.inner.next_event(timeout)
    }

    /// 非阻塞取下一个 host 事件。
    pub fn poll_event(&self) -> Option<HostEvent> {
        self.inner.poll_event()
    }

    /// 连接是否已终止。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    /// 主动拆除。`Host::stop` 幂等，重复调用（含 `Drop` 路径）安全。
    ///
    /// 注意：`Host::stop` 需 `&mut self`，而句柄是共享的——故拆除按
    /// 「丢弃句柄」执行（`Arc` 计数归零时 `Host::drop` → `stop`）。丢弃句柄后
    /// 转发配对状态一并清空：在途转发由 [`FORWARD_TIMEOUT`] 兜底返回传输错，且**超时即撤销在途**。
    pub fn stop(&mut self) {
        *self.responder.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

// 决策项（留给用户，不在本卡拍板）：
// 1. 谁 spawn 谁——网关作为 host 被桌面壳 spawn（`stdio` 读 stdin/stdout），
//    还是网关自己 `stdio::spawn_stdio` 拉起 CLI agent 作为**客户端**角色？
//    两者在 `agent-acp` 里都已成型，接线方式相反，需用户定网关的定位。
// 2. 「域:动作」↔「session/*」的**映射策略**仍未定（本卡只做机制：v0 动作名
//    原样当 ACP method 投；自创映射表需先有方法面台账）。
