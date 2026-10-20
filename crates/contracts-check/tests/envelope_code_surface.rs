//! 响应码面（envelope code surface）穷举：**实现源码里发出的码 ⊆ 契约登记的码**。
//!
//! 这条断言治的是「实现发了码、契约没登记」的漂移（与动作面同一类病）：
//! `agent-gateway` 的 RPC 分发把错误码散落在 `rpc.rs`/`server.rs` 的各处
//! 字面量里，契约侧只登记了一部分时，客户端无法判断「收到的码是合法的、还是
//! 协议漂移」。
//!
//! 做法照抄 `tests/action_surface.rs::scan_action_literals`：**只读文件、手写
//! 字符扫描、不新增依赖、不用正则库**。扫的是**能作为响应码发出的字面量**，
//! 而不是「看起来像码的数字」——见 [`scan_code_literals`] 的口径说明。
use contracts_check::ENVELOPE_CODES;
use std::collections::BTreeSet;

/// 被扫描的网关源码文件（相对 `agent-gateway/src/`）。
///
/// 覆盖面说明：`rpc.rs` 是 RPC 分发与错误映射的唯一落点，`server.rs` 是鉴权
/// 中间件与未知路由的落点；其余网关源文件（`acp.rs` / `actions.rs` /
/// `handshake_windows.rs` / `lib.rs`）**实测零命中**（其数字都不是响应码，
/// 详见 [`scan_code_literals`] 的「实测」一节），故不纳入扫描文件表。
const SCANNED_FILES: &[&str] = &["rpc.rs", "server.rs"];

/// 一行是不是注释：去掉缩进后以 `//` 开头。
///
/// 只认**行注释**是够的：被扫描的两个文件里**没有块注释**（`rpc.rs` 里唯一
/// 出现的 `/*` 在一行 `///` 注释内的 `session/*` 路径写法里，而该行本身就被
/// 本函数跳过）。真正的干扰项是 `rpc.rs` 的 `///` **行文档注释**（如
/// ``/// → `(502, "acp forwarding failed")` ``），必须排除，否则**注释里写的
/// 码会被算进实现面**。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 从源码文本里抽出所有「码字面量」。
///
/// 口径：三位数字，且满足下列**之一**：
/// 1. 前面（跳过空白）是 `(`——`envelope(400, …)` 的实参、`Err((400, …))` 的
///    元组写法；
/// 2. 紧跟在 `"code":` 之后（允许中间有空白）——直接构造信封 JSON 时
///    `{"code":404, …}` 的写法。
///
/// 另外要求该数字串**恰好三位**、且**左右都不是数字**（避免把 `65000`、
/// `4242` 这类更长数字的前/后三位切出来）。
///
/// 具体做法（手写字符扫描，不用正则）：
/// 1. 逐行处理，跳过行注释（见 [`is_comment_line`]）；
/// 2. 在行内找**每一个**数字串的起止位置，取「恰好三位」且左右都不是数字的片段；
/// 3. 再按上面第 1/2 条判它是「码的写法」才收进结果集。
///
/// ⚠️ 为什么第 3 条要卡这么死：被扫描文件里还有**不是码**的数字——`rpc.rs` 的
/// `REPLAY_DEFAULT_LIMIT = 200`、`server.rs` 的 `.mode(0o600)`。前者前面是
/// `=`、后者前面是 `o`，两条都过不了「前面是 `(`」；`"code":` 判据也不会误收。
///
/// # 实测（2026-10-20，main `001e222`）
///
/// 排除注释后：`rpc.rs` 命中 `{400, 403, 404, 408, 409, 500, 502}`、
/// `server.rs` 命中 `{403, 404}`；`acp.rs` / `actions.rs` /
/// `handshake_windows.rs` / `lib.rs` 零命中。本断言**不锁这套实测值**
/// （锁值会在正当扩充时必红），只守「扫出的 ⊆ 登记的」这条关系。
fn scan_code_literals(source: &str) -> BTreeSet<u16> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            if !bytes[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let end = i;
            let left_digit = start > 0 && bytes[start - 1].is_ascii_digit();
            let right_digit = end < bytes.len() && bytes[end].is_ascii_digit();
            if left_digit || right_digit || end - start != 3 {
                continue;
            }
            // 判据 1：前面（跳过空白）是 `(`。
            let mut k = start;
            while k > 0 && bytes[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            let prev = if k > 0 { bytes[k - 1] } else { b' ' };
            // 判据 2：紧跟在 `"code":` 之后。
            let after_code_key = line[..start].trim_end().ends_with("\"code\":");
            if prev == b'(' || after_code_key {
                if let Ok(code) = line[start..end].parse::<u16>() {
                    out.insert(code);
                }
            }
        }
    }
    out
}

/// 读被扫描的网关源文件（相对本 crate 的 `CARGO_MANIFEST_DIR`）。
fn gateway_source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("agent-gateway")
        .join("src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()))
}

/// 核心关系不变量：**实现源码里扫出的码 ⊆ 契约登记的码**。
///
/// 红了说明：实现发了新码，但 `contracts_check::ENVELOPE_CODES` 没登记——先补
/// 登记（并同步 `docs/contracts/shell-agent.md` §2），再合并。
///
/// ⚠️ 刻意**只守关系、不锁数量/取值**：实现正当新增错误码时，把新码登记进
/// `ENVELOPE_CODES` 后本断言自然变绿。任何「扫出恰好 N 个码」「扫出集合等于
/// 某个定值」的断言都会在正当扩充时必红（上一张动作面卡就踩过这个坑）。
#[test]
fn gateway_code_literals_are_subset_of_envelope_codes() {
    let registered: BTreeSet<u16> = ENVELOPE_CODES.iter().copied().collect();
    for file in SCANNED_FILES {
        let scanned = scan_code_literals(&gateway_source(file));
        // 防空扫描：扫不出任何码 ⇒ 扫描器本身坏了（路径写错 / 口径改坏），
        // 断言会「因为什么都没扫到」而假绿。
        assert!(
            !scanned.is_empty(),
            "{file} 一个码字面量都没扫到——扫描口径或文件路径坏了，断言会假绿"
        );
        let unregistered: Vec<u16> = scanned.difference(&registered).copied().collect();
        assert!(
            unregistered.is_empty(),
            "{file} 发出但 ENVELOPE_CODES 未登记的码（差集）{unregistered:?}——\
             契约落后于实现，先补登记再合并"
        );
    }
}
