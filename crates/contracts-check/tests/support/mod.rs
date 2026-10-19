//! 测试夹具：起真实网关（RPC 裸 TCP + 真实事件总线）+ 一个只推帧的 WS mock。
//!
//! RPC 侧写法照抄 `crates/agent-gateway/tests/rpc.rs` 的 `request()` / `runtime()`
//! （契约锚点，双端各保有一份）。事件侧两条路：
//! - [`start_gateway`] + `lengsuo-shell` 的真实客户端：整条 `/events` 回环；
//! - [`ws_mock`]：只做 WS 服务端握手并按序推给定帧，用来喂**未登记 kind / 非法帧**
//!   这类真实网关不会主动产生的输入。解码器仍是 `lengsuo-shell` 的真身，
//!   mock 只负责线格式。写法照抄 `crates/lengsuo-shell/tests/support/mod.rs` 的
//!   `MockGateway::events` / `events_conn`（双端各保有一份）。
//!
//! **故意不加 `#![cfg(unix)]`**：握手文件的 Windows ACL 已落地
//! （`crates/agent-gateway/src/handshake_windows.rs`），本 crate 的断言必须在
//! Windows 真机上同样可跑。
#![allow(dead_code)]

use agent_core::{provider::EchoProvider, runtime::Runtime, schema::Isolation};
use agent_gateway::{GatewayConfig, GatewayHandle};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// 起一个临时工作区 + echo provider 的运行时（照抄 gateway 测试夹具）。
pub fn runtime(dir: &std::path::Path) -> Runtime {
    Runtime::builder()
        .workdir(dir)
        .provider(Arc::new(EchoProvider))
        .build()
        .unwrap()
}

/// 在临时目录里起一个真实网关，返回句柄与目录（目录需活到测试结束）。
pub async fn start_gateway() -> (GatewayHandle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let handle = GatewayHandle::start(
        runtime(dir.path()),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();
    (handle, dir)
}

/// 起一个真实网关，同时把 `Runtime` 一并交出（事件面需要直接推总线）。
pub async fn start_gateway_with_runtime() -> (GatewayHandle, Runtime, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.json");
    let rt = runtime(dir.path());
    let handle = GatewayHandle::start(
        rt.clone(),
        GatewayConfig::new(&path, Isolation::local_default()),
    )
    .await
    .unwrap();
    (handle, rt, dir)
}

/// 带 token 的 `POST /rpc`：返回 `(http_status, envelope_json)`。
pub async fn request(port: u16, token: &str, action: &str, payload: Value) -> (u16, Value) {
    request_with_headers(port, Some(token), action, payload).await
}

/// 同上，但 `token=None` 时**完全不发** `X-Gateway-Token` 头（鉴权用例需要）。
pub async fn request_with_headers(
    port: u16,
    token: Option<&str>,
    action: &str,
    payload: Value,
) -> (u16, Value) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let body = json!({"action":action,"payload":payload}).to_string();
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let auth = token.map_or(String::new(), |t| format!("X-Gateway-Token: {t}\r\n"));
        let wire = format!(
            "POST /rpc HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(wire.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            serde_json::from_str(body).unwrap(),
        )
    })
    .await
    .expect("bounded RPC")
}

/// 从握手文件读出真实 `{port, pid, token}`（验证文件面而不是只看内存值）。
pub fn read_handshake(dir: &std::path::Path) -> Value {
    let bytes = std::fs::read(dir.join("gateway.json")).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// 一帧事件文本：`{seq, session_id, kind, payload, ts_ms}`（`AGENT-KICKOFF.md` §3）。
///
/// `payload` 传**已序列化**的 JSON 文本（`"null"` / `{"text":"hi"}`），
/// 便于构造刻意的非法帧（如缺 `kind`、半截 JSON）。
pub fn event_frame(seq: u64, session: &str, kind: &str, payload: &str, ts_ms: u64) -> String {
    format!(
        r#"{{"seq":{seq},"session_id":"{session}","kind":"{kind}","payload":{payload},"ts_ms":{ts_ms}}}"#
    )
}

/// 只推帧的 WS mock 网关：`GET /events` 握手成功后按顺序推 `frames` 文本帧。
///
/// 绑定 `127.0.0.1:0`（随机端口），与协议 §2 的监听形态一致；token 不校验
/// （本 mock 只喂解码器，鉴权另有真实网关用例覆盖）。
///
/// 服务端**不引入 `tokio-tungstenite`**：`contracts-check` 的 dev-dependencies 已冻结
/// （新增会改 `Cargo.lock` 这个共享区），故按 RFC 6455 §1.2/§4/§5.2/§5.5 用 `tokio::io`
/// 手写最小子集：`Sec-WebSocket-Accept = base64(SHA1(key + GUID))` + 服务端文本帧
/// （不掩码、FIN=1、opcode=0x1）。被侧的客户端解码器仍是 `lengsuo-shell` 真身。
pub struct WsMock {
    addr: SocketAddr,
    accept: JoinHandle<()>,
    conns: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
}

impl WsMock {
    /// 监听地址（客户端据此构造 `GatewayHandshake.port`）。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 停掉 mock：中止 accept 与所有在途连接任务。
    pub async fn shutdown(self) {
        self.accept.abort();
        let tasks: Vec<JoinHandle<()>> = {
            let mut guard = self.conns.lock().expect("mock 状态锁");
            std::mem::take(&mut *guard)
        };
        for task in tasks {
            task.abort();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 起一个 WS mock，握手后按序推 `frames`。
pub async fn ws_mock(frames: Vec<String>) -> WsMock {
    let listener = TcpListener::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .expect("mock 网关绑定失败");
    let addr = listener.local_addr().expect("mock 绑定随机端口成功");
    let conns: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let accept = {
        let conns = Arc::clone(&conns);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    return;
                };
                let task = tokio::spawn(events_conn(stream, frames.clone()));
                conns.lock().expect("mock 状态锁").push(task);
            }
        })
    };
    // 等监听器进入可接受状态，避免客户端首连偶发拒绝。
    tokio::time::sleep(Duration::from_millis(20)).await;
    WsMock {
        addr,
        accept,
        conns,
    }
}

/// 处理一条 `/events` WS 连接：握手 → 按序推帧 → 保持到对端关闭。
async fn events_conn(mut stream: TcpStream, frames: Vec<String>) {
    let Some(key) = read_ws_handshake(&mut stream).await else {
        return;
    };
    let accept = ws_accept_key(&key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    if stream.write_all(response.as_bytes()).await.is_err() {
        return;
    }
    for frame in &frames {
        if stream
            .write_all(&ws_text_frame(frame.as_bytes()))
            .await
            .is_err()
        {
            return;
        }
    }
    let _ = stream.flush().await;
    // 保持连接直到对端关闭：读到的字节一律丢弃（客户端的 Close/控制帧不影响推帧），
    // 避免提前 EOF 让测试看到假失败。
    let mut scratch = [0u8; 1024];
    while matches!(stream.read(&mut scratch).await, Ok(n) if n > 0) {}
}

/// 读到 `\r\n\r\n` 收尾，返回 `Sec-WebSocket-Key` 头原值（RFC 6455 §4.2.1）。
async fn read_ws_handshake(stream: &mut TcpStream) -> Option<String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]);
    head.lines().find_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case("sec-websocket-key"))
            .map(|(_, value)| value.trim().to_owned())
    })
}

/// `Sec-WebSocket-Accept = base64(SHA1(key + GUID))`（RFC 6455 §4.2.2）。
///
/// SHA-1 与 base64 各二十来行，手写以免新增 `sha1`/`base64` 依赖（共享区 `Cargo.lock`
/// 不可改）。这只是**对端**的握手回执，安全性不依赖它，且测试只跑回环。
fn ws_accept_key(sec_websocket_key: &str) -> String {
    let mut input = sec_websocket_key.trim().as_bytes().to_vec();
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64(&sha1(&input))
}

/// 一个服务端文本帧：FIN=1、opcode=0x1、不掩码（RFC 6455 §5.2/§5.5）。
///
/// 载荷长度用 7 / 7+16 / 7+64 三种编码，与协议长度字段一一对应。
fn ws_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x81u8];
    let len = payload.len();
    if len < 126 {
        frame.push(len as u8);
    } else if len <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

/// SHA-1（FIPS 180-4）。仅用于 WS 握手回执。
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for block in padded.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// 标准 base64（RFC 4648 §4，带 `=` 填充）。
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(ALPHABET[(n >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(n >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

/// 常量自检：手写的 SHA-1/base64/帧编码必须与 RFC 6455 §1.3 的样例逐字节一致，
/// 否则事件面用例会因为「mock 自己就是错的」而红得莫名其妙。
#[test]
fn ws_handshake_primitives_match_rfc6455_example() {
    assert_eq!(
        ws_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d（FIPS 180-2 样例）。
    assert_eq!(base64(&sha1(b"abc")), "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=");
    // 短帧：单字节长度。
    assert_eq!(ws_text_frame(b"hi"), vec![0x81, 0x02, b'h', b'i']);
    // 126..=65535：7+16 位长度。
    let mid = vec![b'x'; 300];
    let frame = ws_text_frame(&mid);
    assert_eq!(&frame[..4], &[0x81, 126, 0x01, 0x2C]);
    assert_eq!(frame.len(), 4 + 300);
}
