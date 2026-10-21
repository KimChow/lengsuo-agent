//! ACP 转发传播超时 vs 网关 RPC 超时：**两档超时**的关系断言。
//!
//! # 这条断言治什么病
//!
//! 网关 `/rpc` 路径上有**两档**超时。此前它们的**大小关系只由散文承载**
//! （`crates/agent-gateway/tests/acp_forward_whitelist.rs` 与
//! `crates/agent-gateway/tests/acp_ping_peer.rs` 的注释里写着「5s / 30s」），
//! **没有任何机器可判定的断言** ⇒ 谁把 `FORWARD_TIMEOUT` 调大到 ≥ 网关 RPC 超时，
//! 错误码会从 502 退化成 408，而**没有任何测试会红**。
//!
//! # 两档各是什么
//!
//! | 档 | 值 | 真相来源 | 触发后的信封码 |
//! |---|---|---|---|
//! | 转发传播超时（内层） | **5s** | `crates/agent-gateway/src/acp.rs` 的 `FORWARD_TIMEOUT` | `502 "acp forwarding failed"` |
//! | 网关 RPC 超时（外层） | **30s**（默认，可配） | `crates/agent-core/src/config.rs` 的 `DEFAULT_RPC_TIMEOUT_MS` | `408 "request timed out"` |
//!
//! # 为什么「转发档必须更小」（**承重**）
//!
//! 网关 `/rpc` 的外层 `timeout`（`rpc.rs` 的 `handle`）用 RPC 超时档；ACP 转发在
//! **内层**先撞自己的传播超时，转发失败被映射成 `502`（`rpc.rs` 的 `acp_forward`）。
//!
//! - **转发档 < RPC 档**（默认 5s < 30s）：对端不应答 ⇒ 先撞 5s ⇒ 转发回传输错
//!   ⇒ **502**；外层 30s **永不命中**转发路径 ⇒ 用户拿到的错误码是**真实的**
//!   （「ACP 对端不应答」）。
//! - **转发档 ≥ RPC 档**：外层 30s **先**到 ⇒ 转发路径被外层 `timeout` 砍掉 ⇒
//!   用户拿到 **`408 "request timed out"`** ⇒ **错误码退化**：408 的语义是
//!   「整个 RPC 没做完」，**不是**「ACP 对端不应答」⇒ **误导性错误码**。
//!
//! ⇒ 这条大小关系是**承重**的 ⇒ 必须有机器可判定的断言。
//!
//! # 做法
//!
//! `FORWARD_TIMEOUT` 是**私有 const**（非 `pub`）⇒ 契约层拿不到；`agent-gateway` /
//! `agent-core` 虽是本 crate 的 dev-dependency，但 `Cargo.toml` 属共享区、本卡不得改
//! ⇒ 只能 `include_str!` **编译期绑定**源文件**文本**做手写字符扫描（沿用
//! `tests/acp_notification_surface.rs` 与 `tests/action_dispatch_surface.rs` 的
//! 既有做法，不新增依赖、不用正则库）。
//!
//! # ⚠️ 已知限制（**刻意不写成断言**）
//!
//! `rpc_timeout_ms` **可配**（`config.rs` 的 `"rpcTimeoutMs"`）⇒ 用户把它配到
//! **≤ 转发超时**时，同样会退化成 408。⇒ 那是**可配行为**、不是不变量 ⇒ 只在
//! `contracts_check::ACP_FORWARD_TIMEOUT_SECS` 的文档注释里**登记为已知限制**，
//! **不**写成断言（写成断言会在正当调小配置时必红）。

use contracts_check::ACP_FORWARD_TIMEOUT_SECS;

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
const ACP_SOURCE: &str = include_str!("../../agent-gateway/src/acp.rs");
const CORE_CONFIG_SOURCE: &str = include_str!("../../agent-core/src/config.rs");
const RPC_SOURCE: &str = include_str!("../../agent-gateway/src/rpc.rs");

/// 网关 RPC 超时的**冻结默认值**（毫秒）——契约里写死的那一档。
///
/// 冻结依据：`AGENT-KICKOFF.md` 第 22 行的信封冻结句「…`408` 超时（默认 30s，
/// `rpc_timeout_ms` 可调）」。⇒ 改它**不是**实现调整，而是**契约变更** ⇒ 本值
/// 可以（且应该）锁住。
const FROZEN_RPC_DEFAULT_MS: u64 = 30_000;

/// 一行是不是注释行：去掉缩进后以 `//` 开头。
///
/// ⛔ **必须排除注释行**：`acp.rs` 的 `FORWARD_TIMEOUT` 与 `config.rs` 的
/// `DEFAULT_RPC_TIMEOUT_MS` 上方都挂着 `///` 文档注释，不排除就可能把**散文里的
/// 字样**当成实现。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 从源码文本里解析转发传播超时的**秒数**。
///
/// 口径（手写字符扫描，不用正则）：扫**非注释行**，找
/// `const FORWARD_TIMEOUT`，要求类型标注**恰好**是 `Duration`、右值**恰好**是
/// `Duration::from_secs(<数字>)`。类型/右值形态不符 ⇒ 返回 `None`（**不猜**）。
fn parse_forward_timeout_secs(source: &str) -> Option<u64> {
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let Some(at) = line.find("const FORWARD_TIMEOUT") else {
            continue;
        };
        let after = &line[at + "const FORWARD_TIMEOUT".len()..];
        let colon = after.find(':')?;
        let eq = after[colon + 1..].find('=')?;
        let ty = after[colon + 1..colon + 1 + eq].trim();
        if ty != "Duration" {
            return None;
        }
        let rhs = after[colon + 1 + eq + 1..].trim();
        let args = rhs.strip_prefix("Duration::from_secs(")?;
        let close = args.find(')')?;
        return args[..close].trim().parse::<u64>().ok();
    }
    None
}

/// 从源码文本里解析网关 RPC 超时的**默认毫秒数**。
///
/// 口径（手写字符扫描，不用正则）：扫**非注释行**，找
/// `const DEFAULT_RPC_TIMEOUT_MS`，要求类型标注**恰好**是 `u64`、右值是**纯数字**
/// （允许 `30_000` 这种下划线分隔写法）。
fn parse_default_rpc_timeout_ms(source: &str) -> Option<u64> {
    for line in source.lines() {
        if is_comment_line(line) {
            continue;
        }
        let Some(at) = line.find("const DEFAULT_RPC_TIMEOUT_MS") else {
            continue;
        };
        let after = &line[at + "const DEFAULT_RPC_TIMEOUT_MS".len()..];
        let colon = after.find(':')?;
        let eq = after[colon + 1..].find('=')?;
        let ty = after[colon + 1..colon + 1 + eq].trim();
        if ty != "u64" {
            return None;
        }
        let rhs = after[colon + 1 + eq + 1..].trim();
        let rhs = rhs.strip_suffix(';')?.trim();
        let digits: String = rhs.chars().filter(|c| *c != '_').collect();
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        return digits.parse::<u64>().ok();
    }
    None
}

/// **断言 A**：实现值 == 契约登记值。
///
/// 红了说明：`crates/agent-gateway/src/acp.rs` 的 `FORWARD_TIMEOUT` 改了秒数，
/// 而 `contracts_check::ACP_FORWARD_TIMEOUT_SECS` 没跟着改 ⇒ 契约与实现漂移，
/// 先同步契约常量（并同步本仓本地保留区文档的对应小节），再合并。
#[test]
fn forward_timeout_implementation_matches_the_registered_contract() {
    let scanned = parse_forward_timeout_secs(ACP_SOURCE).expect(
        "扫不到 `const FORWARD_TIMEOUT: Duration = Duration::from_secs(N);`——\
         解析口径或源文件形态坏了，断言会假绿",
    );
    assert_eq!(
        scanned, ACP_FORWARD_TIMEOUT_SECS,
        "转发传播超时：实现 {scanned}s != 契约登记 {ACP_FORWARD_TIMEOUT_SECS}s——\
         实现改了而契约没跟上（或反之），先同步 \
         contracts_check::ACP_FORWARD_TIMEOUT_SECS 再合并"
    );
}

/// 转发档是否**违反**「严格小于 RPC 档」这条关系。
///
/// 抽成独立函数，是为了让断言 B 与 [`tier_relation_rejects_a_raised_forward_timeout`]
/// 的合成样本走**同一条**判定口径 ⇒ 后者能证明「把实现调大到 ≥30s 时这条关系
/// **真的会被判红**」，而不是只证明断言 B 对当前值绿。
fn tier_relation_is_violated(forward_ms: u64, rpc_default_ms: u64) -> bool {
    forward_ms >= rpc_default_ms
}

/// **断言 B**：转发档 **严格小于** 网关 RPC 默认档。
///
/// 这条**就是**承重不变量的本体：转发超时一旦 ≥ 网关 RPC 超时，外层 `timeout`
/// 会先到 ⇒ 转发路径被砍 ⇒ 用户拿 `408 "request timed out"` 而不是
/// `502 "acp forwarding failed"` ⇒ **错误码退化**（408 的语义是「整个 RPC
/// 没做完」，不是「ACP 对端不应答」）。
///
/// ⚠️ 两边都**扫出来**，不硬编码：
/// - 左边 = **实现侧**扫出的 `FORWARD_TIMEOUT` 秒数（⇒ 有人把实现调到 ≥30s 时
///   **这条不变量本身**就会红，不必先改契约常量）；
/// - 右边 = **从 `agent-core/src/config.rs` 扫出来的** `DEFAULT_RPC_TIMEOUT_MS`
///   （见断言 C）⇒ 改 core 的默认值也会被本卡捕获。
///
/// （断言 A 守的是**契约登记值 == 实现值**；本条守的是**两档之间的大小关系**——
/// 两条独立：实现改了而契约没改 ⇒ A 红；两边都改了但大小关系破了 ⇒ 本条红。）
#[test]
fn forward_timeout_tier_is_below_the_gateway_rpc_default() {
    let rpc_default_ms = parse_default_rpc_timeout_ms(CORE_CONFIG_SOURCE)
        .expect("扫不到 `pub const DEFAULT_RPC_TIMEOUT_MS: u64 = N;`——解析口径或源文件形态坏了");
    let forward_ms = parse_forward_timeout_secs(ACP_SOURCE)
        .expect("扫不到 `const FORWARD_TIMEOUT: Duration = Duration::from_secs(N);`——解析口径或源文件形态坏了")
        * 1_000;
    assert!(
        !tier_relation_is_violated(forward_ms, rpc_default_ms),
        "转发传播超时 {forward_ms}ms 不小于网关 RPC 默认超时 {rpc_default_ms}ms——\
         外层 RPC 超时会先于转发超时命中 ⇒ 转发失败被外层 timeout 砍掉 ⇒ \
         用户拿 408（\"整个 RPC 没做完\"）而不是 502（\"ACP 对端不应答\"）⇒ \
         错误码退化、语义误导。必须让转发档严格小于 RPC 档"
    );
}

/// **断言 B 的红线自证**：把转发档**调到 ≥ RPC 档**时，这条关系**真的会被判红**。
///
/// 用**合成样本**（不改 `crates/agent-gateway/`——本卡不得碰它）走与断言 B
/// **同一条**判定口径 [`tier_relation_is_violated`] ⇒ 证明「有人把
/// `FORWARD_TIMEOUT` 调大到 30s / 45s 时，断言会红」，而不是只证明当前值绿。
#[test]
fn tier_relation_rejects_a_raised_forward_timeout() {
    let rpc_default_ms = FROZEN_RPC_DEFAULT_MS;
    // 现值：5s < 30s ⇒ 不违反。
    assert!(
        !tier_relation_is_violated(5_000, rpc_default_ms),
        "现值 5s 竟被判成违反——判定口径坏了"
    );
    // 调大到与 RPC 档**相等** ⇒ 违反（外层会先命中）。
    assert!(
        tier_relation_is_violated(30_000, rpc_default_ms),
        "转发档 == RPC 档竟未被判违反——「必须严格小于」的口径坏了"
    );
    // 调大到 45s ⇒ 违反。
    assert!(
        tier_relation_is_violated(45_000, rpc_default_ms),
        "转发档 45s 竟未被判违反——判定口径坏了"
    );
    // 调小到 1s ⇒ 仍不违反（关系是单向的：转发档小 ⇒ 正确）。
    assert!(
        !tier_relation_is_violated(1_000, rpc_default_ms),
        "转发档 1s 竟被判成违反——判定口径坏了"
    );
}

/// **断言 C**：网关 RPC 超时的**默认值本身可查**，且等于冻结值 30_000。
///
/// 冻结依据见 [`FROZEN_RPC_DEFAULT_MS`] 的文档注释（信封冻结句「`408` 超时
/// （默认 30s）」）。⇒ 改 core 的默认值 = **契约变更**，不是实现调整。
#[test]
fn gateway_rpc_default_timeout_is_the_frozen_thirty_seconds() {
    let scanned = parse_default_rpc_timeout_ms(CORE_CONFIG_SOURCE)
        .expect("扫不到 `pub const DEFAULT_RPC_TIMEOUT_MS: u64 = N;`——解析口径或源文件形态坏了");
    assert_eq!(
        scanned, FROZEN_RPC_DEFAULT_MS,
        "网关 RPC 超时默认值 {scanned}ms != 冻结值 {FROZEN_RPC_DEFAULT_MS}ms——\
         这是**契约变更**（信封冻结句写的是「默认 30s」），不是实现调整；\
         若确要改，先改契约文档与冻结值，再改实现"
    );
}

/// **断言 D**：**防空扫描**——两个解析器都**确实扫到了东西**。
///
/// ⛔ 口径坏了（源文件形态变了 / 解析器写错）时，A/B/C 会「因为什么都没扫到」而
/// **假绿** ⇒ 必须单独守一条「扫得到」。
#[test]
fn scanners_actually_find_both_timeout_constants() {
    assert!(
        ACP_SOURCE.contains("const FORWARD_TIMEOUT"),
        "agent-gateway/src/acp.rs 里找不到 `const FORWARD_TIMEOUT` 声明——\
         实现改了常量名或搬走了，解析器会扫空 ⇒ 断言假绿"
    );
    assert!(
        CORE_CONFIG_SOURCE.contains("const DEFAULT_RPC_TIMEOUT_MS"),
        "agent-core/src/config.rs 里找不到 `const DEFAULT_RPC_TIMEOUT_MS` 声明——\
         实现改了常量名或搬走了，解析器会扫空 ⇒ 断言假绿"
    );
    assert!(
        parse_forward_timeout_secs(ACP_SOURCE).is_some(),
        "转发超时的秒数解析不到——解析口径或源文件形态坏了，断言会假绿"
    );
    assert!(
        parse_default_rpc_timeout_ms(CORE_CONFIG_SOURCE).is_some(),
        "RPC 超时的默认毫秒数解析不到——解析口径或源文件形态坏了，断言会假绿"
    );
}

/// **两档的语义锚点仍在**：转发失败仍映射成 `502`，外层超时仍映射成 `408`。
///
/// 承重推理的**另一半**：`502` 与 `408` 是**两个不同的码**才有「退化」可言。
/// ⇒ 实现若把转发失败的映射改掉（或两档码并成一个），本卡的推理前提就变了 ⇒
/// 这里钉住两个锚点字面量。
#[test]
fn the_two_tiers_keep_their_distinct_envelope_codes() {
    assert!(
        RPC_SOURCE.contains("(502, \"acp forwarding failed\")"),
        "rpc.rs 里找不到转发失败 → 502 的映射——承重推理的前提变了（502 与 408 \
         必须是两个不同的码才有「退化」可言），先更新契约再改实现"
    );
    assert!(
        RPC_SOURCE.contains("408, \"request timed out\""),
        "rpc.rs 里找不到外层超时 → 408 的映射——承重推理的前提变了，先更新契约再改实现"
    );
}

/// 解析器**排除注释行 + 卡死类型标注**这两条规则的自我证明。
///
/// ⛔ 若不排除注释，被注释掉的 `const FORWARD_TIMEOUT: …` 就会被当成实现 ⇒
/// 扫出来的秒数是**注释里那个**。本断言用**合成样本**把规则钉死（样本里的注释
/// 行是**完整可解析的声明**——否则这条自检只是「碰巧绿」）。
#[test]
fn scanner_excludes_comment_lines_and_requires_the_declared_type() {
    let sample = "\
/// const FORWARD_TIMEOUT: Duration = Duration::from_secs(99);
const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);
";
    assert_eq!(
        parse_forward_timeout_secs(sample),
        Some(5),
        "注释行里的声明被当成了实现，或真声明没被扫到——注释排除规则失效"
    );

    let wrong_type = "const FORWARD_TIMEOUT: u64 = 5;";
    assert_eq!(
        parse_forward_timeout_secs(wrong_type),
        None,
        "类型标注不是 `Duration` 却仍被解析——类型口径失效"
    );

    let wrong_rhs = "const FORWARD_TIMEOUT: Duration = Duration::from_millis(5000);";
    assert_eq!(
        parse_forward_timeout_secs(wrong_rhs),
        None,
        "右值不是 `Duration::from_secs(…)` 却仍被解析——右值口径失效"
    );
}

/// RPC 默认值解析器**认得下划线分隔**的写法，且认不得别的形态。
#[test]
fn rpc_default_parser_accepts_underscore_separated_digits_only() {
    assert_eq!(
        parse_default_rpc_timeout_ms("pub const DEFAULT_RPC_TIMEOUT_MS: u64 = 30_000;"),
        Some(30_000)
    );
    assert_eq!(
        parse_default_rpc_timeout_ms("pub const DEFAULT_RPC_TIMEOUT_MS: u64 = 30000;"),
        Some(30_000)
    );
    assert_eq!(
        parse_default_rpc_timeout_ms("pub const DEFAULT_RPC_TIMEOUT_MS: u32 = 30_000;"),
        None,
        "类型标注不是 `u64` 却仍被解析——类型口径失效"
    );
    assert_eq!(
        parse_default_rpc_timeout_ms("pub const DEFAULT_RPC_TIMEOUT_MS: u64 = 30_000u64;"),
        None,
        "右值不是纯数字却仍被解析——右值口径失效"
    );
}
