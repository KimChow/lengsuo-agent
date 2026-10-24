//! 网关**信封 `(code, msg)` 配对面**守卫：实现源码里发出的 `msg` 文案与它的配对
//! 码，冻结/承诺项必须扫得到——改文案必须变红。
//!
//! # 这条断言治什么病
//!
//! `agent-gateway` 的 RPC 信封是 `{code, msg, data}`，`msg` 是**客户端直接读的
//! 字段**（对端展示面原文展示），但文案散落在 `rpc.rs` / `server.rs` 的字面量里。
//! 既有守卫 `tests/envelope_code_surface.rs` **只扫 code 三位数字面**（不扫
//! `msg` 文案、不扫配对；且按其自述「恰好三位」口径，`(0, "ok")` 的 `0` 连 code
//! 也扫不到）⇒ 实现改任何 msg 文案（或把 408 的 msg 改成别的）**没有任何东西
//! 变红**，对端展示面静默漂移。
//!
//! 实测（2026-10-24，base `dcbe11a`）：全部发射点去重后 13 个 `(code,msg)` 配对，
//! 其中 `(0,"ok")`、`(408,"request timed out")`、`(500,"request failed")`、
//! `(502,"acp forwarding failed")` 的 **msg 文案全仓零断言**（code 在别处有断言，
//! 文案没有）。
//!
//! # 做法
//!
//! 照同类守卫先例（`tests/envelope_code_surface.rs`、`tests/acp_role_symmetry.rs`）：
//! `include_str!` **编译期**绑定 `agent-gateway/src/rpc.rs` 与 `server.rs` 源文本 +
//! **手写字符扫描**，不用正则库、不新增依赖。
//!
//! # 断言口径（关系，不锁总数）
//!
//! - 冻结配对 **⊆** 扫出集：冻结配对**缺失** ⇒ 红；实现正当新增 `(code,msg)`
//!   使扫出集变大 ⇒ **不红**；
//! - 数字段口径取 **1–3 位**、左右非数字——`(0, "ok")` 的 code 是一位数，与
//!   `envelope_code_surface` 的「恰好三位」口径刻意不同，别互相照抄；
//! - fail-closed：任一扫描零命中 ⇒ 显式失败，绝不「扫成空集后全绿」。
use contracts_check::ENVELOPE_CODES;
use std::collections::{BTreeMap, BTreeSet};

/// `rpc.rs` 源文件文本（**编译期**绑定：源文件改动即触发重编）。
const RPC_SOURCE: &str = include_str!("../../agent-gateway/src/rpc.rs");
/// `server.rs` 源文件文本（**编译期**绑定）。
const SERVER_SOURCE: &str = include_str!("../../agent-gateway/src/server.rs");

/// 被扫描的网关源码文件：`rpc.rs` 是 RPC 分发与错误映射的 msg 落点，
/// `server.rs` 是鉴权中间件与未知路由的 msg 落点。
const SCANNED_FILES: [(&str, &str); 2] = [("rpc.rs", RPC_SOURCE), ("server.rs", SERVER_SOURCE)];

/// 冻结/承诺的 `(code, msg)` 配对（13 项）：必须**逐项在**扫出集里。
///
/// `ENVELOPE_CODES` 只登记码不登记文案，`msg` 配对只有本表承载。
const FROZEN_PAIRS: &[(u16, &str)] = &[
    (0, "ok"),                      // 成功（dispatch Ok 分支）
    (400, "invalid JSON"),          // 请求体解析失败（handle）
    (400, "invalid payload"),       // payload 非法（bad）
    (400, "invalid configuration"), // 配置非法（core / InvalidConfig）
    (403, "forbidden"),             // 隔离不匹配 / 审批不属于该会话；鉴权中间件同文案
    (403, "server-bound identity"), // 会话创建携带身份/环境字段
    (404, "not found"),             // 会话/审批不存在（core / *NotFound）
    (404, "unknown action"),        // 未注册动作
    (408, "request timed out"),     // RPC 超时（见下方专条）
    (409, "session closed"),        // 会话已结束（core / SessionClosed）
    (500, "operation failed"),      // CoreError 兜底
    (500, "request failed"),        // 阻塞任务 join 失败兜底
    (502, "acp forwarding failed"), // ACP 转发传输失败
];

/// 一行是不是注释：去掉缩进后以 `//` 开头（`///` 文档注释同前缀，一并命中）。
///
/// ⛔ **必须排除**：`rpc.rs` 的 `///` 文档注释里写着
/// `→ (502, "acp forwarding failed")` 这类**成对**字样——不排除就把散文当实现。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 数字段是不是处在「发码位置」：
/// 1. 前面（跳过空白）是 `(` —— `envelope(400, …)` 实参与 `Err((400, …))` 元组
///    两种写法同口径；
/// 2. 紧跟在 `"code":` 之后（允许中间空白）——直接构造信封 JSON 的
///    `json!({"code":404,"msg":…})` 写法。
///
/// 其余数字（`REPLAY_DEFAULT_LIMIT = 200`、`.mode(0o600)`、`from_secs(5)`、
/// `u64::MAX` 的 `64`）两条判据都过不了，不会误收。
fn in_code_position(before: &str, prev_non_ws: Option<u8>) -> bool {
    prev_non_ws == Some(b'(') || before.trim_end().ends_with("\"code\":")
}

/// 从逗号之后取 msg 字符串字面量内容；形态不符返回 `None`。
///
/// 支持两种后继：`"invalid JSON"`（`envelope(` / `Err((` 形态）与
/// `"msg":"unknown route"`（`json!` 形态——逗号后先挂键再挂值）。
/// 转义处理只为「下一个未转义 `"` 收尾」不失手（这批文案都无转义）。
fn msg_after_comma(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(',')?.trim_start();
    let rest = rest
        .strip_prefix("\"msg\":")
        .map(str::trim_start)
        .unwrap_or(rest);
    let rest = rest.strip_prefix('"')?;
    let mut content = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                content.push('\\');
                content.push(chars.next()?);
            }
            '"' => return Some(content),
            _ => content.push(c),
        }
    }
    None // 引号没闭合（行内扫描，字符串不跨行）
}

/// 从源码文本里扫出全部 `(code, msg)` 发射点（排除注释行，手写字符扫描）。
///
/// 口径：数字段 **1–3 位**且左右都不是数字（一位 `0` 是码；`u64` 的 `64`、
/// `02x` 的 `02` 不许切进来），且处于 [`in_code_position`] 的发码位置，
/// 再从逗号后取到 msg 字符串才算一对。
fn scan_pairs(source: &str) -> Vec<(u16, String)> {
    let mut out = Vec::new();
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
            if left_digit || right_digit || end - start > 3 {
                continue;
            }
            let mut k = start;
            while k > 0 && bytes[k - 1].is_ascii_whitespace() {
                k -= 1;
            }
            let prev = if k > 0 { Some(bytes[k - 1]) } else { None };
            if !in_code_position(&line[..start], prev) {
                continue;
            }
            if let Some(msg) = msg_after_comma(&line[end..]) {
                if let Ok(code) = line[start..end].parse::<u16>() {
                    out.push((code, msg));
                }
            }
        }
    }
    out
}

/// 两文件扫出集的并集（按 `(code, msg)` 配对去重：同一配对在多个发射点出现是
/// 常态，如 `(403, "forbidden")` 有三个发射点）。
fn scanned_pairs() -> BTreeSet<(u16, String)> {
    SCANNED_FILES
        .iter()
        .flat_map(|(_, source)| scan_pairs(source))
        .collect()
}

/// **防空扫 + 登记交叉核对**：两文件扫出的配对非空，且 code 全部已在
/// [`ENVELOPE_CODES`] 登记。
///
/// 扫成空集 ⇒ 扫描口径或源绑定坏了，下游断言会「因为什么都没扫到」假绿
/// ⇒ fail-closed 显式失败。`ENVELOPE_CODES` 只登记码，msg 配对面由本文件守。
#[test]
fn scanned_pairs_are_nonempty_and_codes_are_registered() {
    let registered: BTreeSet<u16> = ENVELOPE_CODES.iter().copied().collect();
    let all = scanned_pairs();
    assert!(
        !all.is_empty(),
        "两文件一个 (code,msg) 配对都没扫到——扫描口径或 include_str! 绑定坏了，\
         断言会假绿"
    );
    for (file, source) in SCANNED_FILES {
        let scanned = scan_pairs(source);
        assert!(
            !scanned.is_empty(),
            "{file} 一个发射点都没扫到——扫描口径或源绑定坏了，断言会假绿"
        );
        let unregistered: Vec<u16> = scanned
            .iter()
            .map(|(code, _)| *code)
            .filter(|code| !registered.contains(code))
            .collect();
        assert!(
            unregistered.is_empty(),
            "{file} 发出但 ENVELOPE_CODES 未登记的码 {unregistered:?}——\
             契约落后于实现，先补登记再合并"
        );
    }
}

/// **冻结/承诺配对子集**：[`FROZEN_PAIRS`] 必须**逐项在**扫出集里。
///
/// ⛔ 刻意**不锁「恰好 13 个」**：实现正当新增 `(code,msg)` 时扫出集变大不该红，
/// 只有冻结配对**缺失**才红——缺失即客户端可见的文案漂移，先恢复文案或走
/// 显式裁定改契约，不许静默改口。
#[test]
fn frozen_pairs_are_present_in_the_implementation() {
    let pairs = scanned_pairs();
    let missing: Vec<(u16, &str)> = FROZEN_PAIRS
        .iter()
        .copied()
        .filter(|&(code, msg)| !pairs.contains(&(code, msg.to_owned())))
        .collect();
    assert!(
        missing.is_empty(),
        "冻结的 (code,msg) 配对没扫到（缺失）{missing:?}——msg 是客户端直接读的\
         字段，改文案 = 对端展示面静默漂移；恢复文案，或先走显式裁定更新契约再合并"
    );
}

/// **同 code 的 msg 互不相同**：扫出集内同一 code 不会挂两个相同文案。
///
/// 注意同一**配对**在多个发射点重复出现是常态（`rpc.rs` 自己就把
/// `(403, "forbidden")` 发了两处），故在按配对去重后的集合上断言：同一 code
/// 名下出现重复文案 = 扫描器或实现有毛病。
#[test]
fn a_code_never_carries_two_identical_msgs() {
    let pairs = scanned_pairs();
    assert!(
        !pairs.is_empty(),
        "一个 (code,msg) 配对都没扫到——扫描口径或 include_str! 绑定坏了，断言会假绿"
    );
    let mut msgs_by_code: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    for (code, msg) in &pairs {
        msgs_by_code.entry(*code).or_default().push(msg);
    }
    for (code, msgs) in &msgs_by_code {
        let unique: BTreeSet<&str> = msgs.iter().copied().collect();
        assert_eq!(
            unique.len(),
            msgs.len(),
            "code {code} 名下挂了重复文案 {msgs:?}——扫出集被污染，扫描器或实现有毛病"
        );
    }
}

/// **408 配对专条**：`(408, "request timed out")` 必须在扫出集里。
///
/// 双重出处：`AGENT-KICKOFF.md` 第 22 行把 `408`（超时）逐字冻结进信封语义——
/// 文案虽非冻结原文，`tests/acp_timeout_tier.rs` 通篇把 `408 "request timed out"`
/// 当承诺文案使用；而此配对的 msg 此前全仓零断言（code 有 `rpc.rs` 的
/// `code==408` 断言，文案没有）⇒ 单独一条钉死，防止 code 不动、文案静默改口。
#[test]
fn timeout_pair_408_request_timed_out_is_frozen() {
    let pairs = scanned_pairs();
    assert!(
        pairs.contains(&(408, "request timed out".to_owned())),
        "(408, \"request timed out\") 不在扫出集——408 超时文案漂移或扫描器坏了;\
         该配对由 AGENT-KICKOFF 第 22 行冻结的 408 超时语义与 \
         tests/acp_timeout_tier.rs 的承诺文案双重背书，改文案须先走显式裁定。\
         实扫到的配对：{pairs:?}"
    );
}

/// 扫描器**排除注释行**这条规则的自我证明。
///
/// `rpc.rs` 的 `///` 文档注释里写着 `→ (502, "acp forwarding failed")` 这类
/// **成对**字样，不排除就会双计/误收。样本里的注释行是**完整可解析的发射**——
/// 否则这条自检只是「碰巧绿」，起不到守规则的作用。
#[test]
fn scanner_excludes_comment_lines() {
    let sample = "\
/// 散文里写的配对不算实现：→ (502, \"acp forwarding failed\")
// let dead = envelope(502, \"acp forwarding failed\", Value::Null);
Json(envelope(408, \"request timed out\", Value::Null))
";
    let scanned = scan_pairs(sample);
    assert_eq!(
        scanned.len(),
        1,
        "注释行里的发射被当成了实现——注释排除规则失效：{scanned:?}"
    );
    assert_eq!(scanned[0], (408, "request timed out".to_owned()));
}

/// 扫描器**两种发射形态 + 非码数字拒收**的自我证明（合成样本，锁值安全——
/// 样本是本文件自带的，不锁实现）。
///
/// 形态 A：`envelope(400, "invalid JSON", …)`；形态 B：`Err((400, "invalid
/// payload".into()))`（多一层括号，同口径覆盖）；形态 C：`json!({"code":404,
/// "msg":"unknown route",…})`（`server.rs` 的信封直构写法）。反例：`200` /
/// `0o600` / `from_secs(5)` / `u64` 的 `64` 都不是码，不得成对。
#[test]
fn scanner_accepts_both_emission_shapes_and_rejects_non_code_digits() {
    let sample = "\
let value = envelope(400, \"invalid JSON\", Value::Null);
let failure = Err((400, \"invalid payload\".into()));
Json(json!({\"code\":404,\"msg\":\"unknown route\",\"data\":null}))
const LIMIT: u64 = 200;
file.options().mode(0o600);
let wait = Duration::from_secs(5);
let big = u64::MAX;
";
    let scanned: BTreeSet<(u16, String)> = scan_pairs(sample).into_iter().collect();
    let expected: BTreeSet<(u16, String)> = [
        (400u16, "invalid JSON".to_owned()),
        (400, "invalid payload".to_owned()),
        (404, "unknown route".to_owned()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        scanned, expected,
        "扫描口径（三种发射形态）或拒收口径（非码数字）坏了：{scanned:?}"
    );
}
