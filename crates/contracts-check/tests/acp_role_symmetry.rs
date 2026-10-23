//! ACP **两个角色**（`client` / `host`）的「出队结果 + 事件面」对称性守卫。
//!
//! # 这条断言治什么病
//!
//! ACP 的两个角色——`client`（主动发起请求的一端）与 `host`（被 spawn 的一端）——
//! 由**同一套** wire 类型驱动，源码用**散文**声明它们的事件面「同形 / 逐条一致」，
//! 但这些承诺**没有任何机器守着**：
//!
//! | 概念 | 位置 | 定义 |
//! |---|---|---|
//! | 出队结果（client） | `src/client.rs` | `pub enum NextEvent` : `Event(ClientEvent)` / `Idle` / `Closed` |
//! | 出队结果（host） | `src/host_router.rs` | `pub enum NextEvent` : `Event(HostEvent)` / `Idle` / `Closed` |
//! | 事件面（client） | `src/router.rs` | `pub enum ClientEvent` |
//! | 事件面（host） | `src/host_router.rs` | `pub enum HostEvent` |
//!
//! 散文承诺（原文摘录）：
//! - `host_router.rs`：「[`Host::next_event`] 的结果（**与 [`crate::client::NextEvent`] 同形**）」；
//! - `host_router.rs`：「帧归类表见 [`crate::host`] 模块头（**两处必须一致**）」；
//! - `host.rs` 模块头：「入站帧语义（**与 [`crate::router`] 的 `RouteState` 逐条一致**）」，
//!   以及「读侧状态机在 `host_router`（归类语义 + 计数）……**三者的语义是同一张表**」。
//!
//! 实测（于本仓内跑，base `f5fcf92`）：
//! `grep -rn "NextEvent\|HostEvent\|ClientEvent" crates/contracts-check/tests/`
//! ⇒ **零命中** ⇒ 「两个角色的状态机同形」这句承诺此前**没有任何断言**。
//!
//! # 为什么这是真风险（不是理论）
//!
//! - `NextEvent` 是**连接生命周期状态**的取值面（`Event` / `Idle` / `Closed`）——
//!   它承载的正是「连接还能不能用」这个 **fail-closed 判定**。网关侧消费
//!   `agent_acp::host::NextEvent`，壳 / 桌面侧消费 `agent_acp::client::NextEvent`，
//!   两条消费路径由**两个同名不同路径**的类型供数 ⇒ 任一侧增删一个状态
//!   （如给 host 加 `Draining`、给 client 加 `Paused`），另一侧**不会有任何东西变红**：
//!   两个角色对「连接终止 / 空闲」的认知**静默分叉**，散文承诺变成假的。
//! - `ClientEvent` 与 `HostEvent`：host 侧多出的那条腿（`UnexpectedResponse`）有
//!   **明确文档依据**——host 不发起请求，故入站的 `Response` 只能是错帧 / 迟到帧
//!   （已计数，仅作观测）。这条「差集恰好登记的那几项」的关系同样无人守：
//!   若有人误在 `ClientEvent` 加一个 `Response` 变体，两侧就会出现**语义矛盾**
//!   且静默通过。
//!
//! # 口径（只守关系，不锁数量）
//!
//! `agent-acp` **不在**本 crate 的 dev-dependencies 里，而 `Cargo.toml` 属共享区、
//! 本卡不得改 ⇒ 照本 crate 同类守卫（`tests/acp_default_parity.rs`、
//! `tests/task_status_drift_guard.rs`）的先例：`include_str!` **编译期**绑定源文件
//! 文本 + 手写扫变体名，**不 import 被检查的 crate**、**不引入正则**。
//!
//! ⛔ 「同形」用**集合相等**表达，**不写「恰好 N 个」** ⇒ 两侧同步扩充**不红**，
//! **单侧**增删 ⇒ 红。解析零命中时**显式失败**（`expect_variants` / `panic!`），
//! 绝不默认放过 ⇒ 不会「扫成空集后全绿」。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `client.rs` 源文件文本（**编译期**绑定：源文件改动即触发重编）。
const CLIENT_SOURCE: &str = include_str!("../../agent-acp/src/client.rs");
/// `host_router.rs` 源文件文本（**编译期**绑定）。
const HOST_ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/host_router.rs");
/// `router.rs` 源文件文本（**编译期**绑定）。
const ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/router.rs");

/// 被绑定源文件的相对路径（供「文件仍在该处」的存在性断言用）。
const SOURCE_PATHS: [(&str, &str); 3] = [
    ("client.rs", "../agent-acp/src/client.rs"),
    ("host_router.rs", "../agent-acp/src/host_router.rs"),
    ("router.rs", "../agent-acp/src/router.rs"),
];

/// **全部被检查的 enum 定义点**：`(文件标签, 源文件文本, enum 名)`。
///
/// 单表唯一来源：状态集与事件面的每条断言都从这张表筛出来，避免再抄一份 enum
/// 清单（否则清单本身又成了会漂移的第二份手写实现）。
const ENUM_POINTS: [(&str, &str, &str); 4] = [
    ("client.rs", CLIENT_SOURCE, "NextEvent"),
    ("host_router.rs", HOST_ROUTER_SOURCE, "NextEvent"),
    ("host_router.rs", HOST_ROUTER_SOURCE, "HostEvent"),
    ("router.rs", ROUTER_SOURCE, "ClientEvent"),
];

/// **已知 `NextEvent` 状态的下界**：两侧的状态集必须**含**这三项。
///
/// ⛔ 这是**下界**（`⊆`），不是「恰好三个」——`NextEvent` 是连接生命周期状态面，
/// 协议正当扩充（两侧同步加一个状态）不该红；但若解析退化成空集，或某一侧
/// 少了一个状态，这条下界会红。
const KNOWN_NEXT_EVENT_STATES: [&str; 3] = ["Event", "Idle", "Closed"];

/// **host 侧独有的事件登记面**：出现在 `HostEvent`、但**不**出现在 `ClientEvent`
/// 的变体。新增 host-only 事件时**只需往这张表加一项**（一处改动，不是「必红」）。
///
/// 出处：`host.rs` 模块头「`Rpc::Response` → host 不发起请求，计
/// `unexpected_responses`（不 panic）」，以及 `host_router.rs` 里
/// `HostEvent::UnexpectedResponse` 的逐条注释「入站响应：host 不发起请求，
/// 故为对端错帧 / 迟到帧（已计数，仅作观测）」。
const HOST_ONLY_EVENTS: [&str; 1] = ["UnexpectedResponse"];

// ---------------------------------------------------------------- 扫描器 ----

/// 一个枚举变体：变体名 + 可选的元组载荷类型名。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Variant {
    /// 变体名。
    name: String,
    /// 元组载荷的类型名（`Event(ClientEvent)` ⇒ `Some("ClientEvent")`；单元变体 ⇒ `None`）。
    payload: Option<String>,
}

/// 一行是不是注释：去掉缩进后以 `//` 开头（只认行注释；被扫的源文件里没有块注释）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 扫出 `pub enum <enum_name> { … }` 的**变体**（**保留源码顺序**）。
///
/// 口径：定位 `pub enum <Name> {` 那一行，逐行吃到该枚举的收尾 `}`；跳过空行、
/// `//`（含 `///` 文档注释）与 `#[…]` 属性行；其余行按变体声明解析——
/// `Event(ClientEvent),` ⇒ 名 `Event` + 载荷 `ClientEvent`，`Idle,` ⇒ 名 `Idle`。
///
/// ⛔ 找不到枚举头、或某行读不出成形的变体名 ⇒ 直接 `panic!`：源文件形态变了必须
/// 让守卫**显式失败**，不许静默跳过（否则集合断言会以空对空的方式假绿）。
fn scan_enum_variants(source: &str, enum_name: &str, file: &str) -> Vec<Variant> {
    let header = format!("pub enum {enum_name} {{");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with(&header))
        .unwrap_or_else(|| {
            panic!(
                "{file} 里找不到 `{header}`——源文件形态变了；\
                 扫不到枚举 ⇒ 后续集合断言会以空对空的方式假绿，故此处必须显式失败"
            )
        });

    let mut out = Vec::new();
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed == "}" {
            break; // 枚举收尾
        }
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
            continue; // 空行 / 文档注释 / 属性行
        }
        let decl = trimmed.trim_end_matches(',').trim();
        let (name, payload) = match decl.split_once('(') {
            Some((name, rest)) => {
                let (inner, _) = rest.split_once(')').unwrap_or_else(|| {
                    panic!("{file} 的 {enum_name}::{decl} 载荷括号不成对——源文件形态变了")
                });
                (name.trim(), Some(inner.trim()))
            }
            None => (decl, None),
        };
        assert!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "{file} 的 {enum_name} 里有一行读不出变体名（原文 `{trimmed}`）——源文件形态变了；\
             按字面吞下去会把非变体行当成变体，集合断言随之失真"
        );
        out.push(Variant {
            name: name.to_owned(),
            payload: payload.map(str::to_owned),
        });
    }
    out
}

/// 一个 `pub enum` 的**变体名集合**（丢掉载荷）。
fn variant_names(source: &str, enum_name: &str, file: &str) -> BTreeSet<String> {
    scan_enum_variants(source, enum_name, file)
        .into_iter()
        .map(|v| v.name)
        .collect()
}

/// 取一份**必须扫出东西**的变体集合；扫成空集即 `panic!`（不得默认放过）。
fn expect_variants(source: &str, enum_name: &str, file: &str) -> BTreeSet<String> {
    let found = variant_names(source, enum_name, file);
    assert!(
        !found.is_empty(),
        "{file} 的 {enum_name} 扫出 0 个变体——解析零命中；\
         死守这条：扫不到就红，绝不默认放过，否则「扫成空集 ⇒ 后面全绿」的假绿无从发现"
    );
    found
}

/// 一个角色的 `NextEvent` **状态集**。
fn next_event_states(source: &str, file: &str) -> BTreeSet<String> {
    expect_variants(source, "NextEvent", file)
}

/// `pub enum <enum_name>` 里某个变体的**元组载荷类型名**；变体不存在 / 无载荷 ⇒ `panic!`。
fn payload_of(source: &str, enum_name: &str, variant: &str, file: &str) -> String {
    let hit = scan_enum_variants(source, enum_name, file)
        .into_iter()
        .find(|v| v.name == variant)
        .unwrap_or_else(|| panic!("{file} 的 {enum_name} 里没有变体 {variant}——源文件形态变了"));
    hit.payload.unwrap_or_else(|| {
        panic!(
            "{file} 的 {enum_name}::{variant} 没有元组载荷——\
             「同形 ≠ 同载荷」这条口径要求它带上**本角色**的载荷类型名"
        )
    })
}

/// **host 事件面减去 client 事件面**（= host 侧独有的事件）。
///
/// 文件标签只管 `panic!` 文案（真实源与合成样本共用这一处比较口径）。
fn host_only_event_diff(
    client_source: &str,
    client_file: &str,
    host_source: &str,
    host_file: &str,
) -> BTreeSet<String> {
    let client = expect_variants(client_source, "ClientEvent", client_file);
    let host = expect_variants(host_source, "HostEvent", host_file);
    host.difference(&client).cloned().collect()
}

/// 某个 `pub enum <Name> {` 的**定义处数**（防空自检：0 ⇒ 被移走 / 改名；
/// 多于 1 ⇒ 同一概念长出了第二份手写实现）。
fn enum_definition_sites(source: &str, enum_name: &str) -> usize {
    let header = format!("pub enum {enum_name} {{");
    source
        .lines()
        .filter(|line| !is_comment_line(line) && line.trim_start().starts_with(&header))
        .count()
}

/// 某个 enum 名的**别名定义行**（`pub type <Name>` / `use … as <Name>` /
/// `use <Name> as …`）。
///
/// 只认**以 `use` / `type` 开头的声明行** ⇒ 不会误伤正文里的用法
/// （如 `NextEvent::Event(…)`、函数签名里的返回类型）。
fn enum_alias_lines<'a>(source: &'a str, enum_name: &str) -> Vec<&'a str> {
    let as_alias = format!("as {enum_name}");
    let aliased = format!("{enum_name} as ");
    let typed = format!("type {enum_name}");
    source
        .lines()
        .filter(|line| {
            if is_comment_line(line) {
                return false;
            }
            let trimmed = line.trim_start();
            let is_decl = trimmed.starts_with("use ")
                || trimmed.starts_with("pub use ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("pub type ");
            is_decl
                && (trimmed.contains(&typed)
                    || trimmed.contains(&as_alias)
                    || trimmed.contains(&aliased))
        })
        .collect()
}

/// 一串变体名字面量 → `BTreeSet<String>`（合成样本与登记表的期望集共用）。
fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// 登记表 `HOST_ONLY_EVENTS` 的集合形态。
fn registered_host_only() -> BTreeSet<String> {
    names(&HOST_ONLY_EVENTS)
}

/// 被检查源文件的绝对路径（`CARGO_MANIFEST_DIR` + 相对路径）。
fn source_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

// ------------------------------------------------------------------ 断言 ----

/// ① **两侧 `NextEvent` 状态集相等**（「同形」承诺的机器可读对应物）。
///
/// 用**集合相等**表达，不写数量断言 ⇒ 两侧同步扩充不红、单侧增删即红；
/// 另加 `KNOWN_NEXT_EVENT_STATES` **下界** ⇒ 解析退化成空集、或少了一个状态也红。
#[test]
fn both_roles_agree_on_the_next_event_state_set() {
    let client = next_event_states(CLIENT_SOURCE, "client.rs");
    let host = next_event_states(HOST_ROUTER_SOURCE, "host_router.rs");

    // 下界先行：已知三项必须都在**两侧**。
    for state in KNOWN_NEXT_EVENT_STATES {
        assert!(
            client.contains(state),
            "client 角色的 NextEvent 缺已知状态 `{state}`（实扫 {client:?}）"
        );
        assert!(
            host.contains(state),
            "host 角色的 NextEvent 缺已知状态 `{state}`（实扫 {host:?}）"
        );
    }

    assert_eq!(
        client, host,
        "两个角色的 NextEvent 状态集不再同形（client 实扫 {client:?}，host 实扫 {host:?}）——\
         这是「连接还能不能用」的 fail-closed 判定面，两侧由**同名不同路径**的类型分别供数；\
         单侧增删一个状态（如给 host 加 `Draining`）会让两个角色对「终止 / 空闲」的认知静默分叉。\
         正当扩充：两侧同时加同一个状态（本条即绿）"
    );
}

/// ② **「同形」≠「同载荷」**：两侧 `NextEvent::Event` 的载荷必须是**各自角色**的事件面
/// （`ClientEvent` / `HostEvent`）。
///
/// ⇒ 把「同形」钉死为**状态集同形**：防止有人把两个 enum 合并成一个，或把载荷也
/// 改成同型而丢掉角色差异。
#[test]
fn the_two_next_events_differ_only_in_event_payload_type() {
    let client_payload = payload_of(CLIENT_SOURCE, "NextEvent", "Event", "client.rs");
    let host_payload = payload_of(HOST_ROUTER_SOURCE, "NextEvent", "Event", "host_router.rs");

    assert_eq!(
        client_payload, "ClientEvent",
        "client 角色 `NextEvent::Event` 的载荷不再是 `ClientEvent`（实为 `{client_payload}`）——\
         载荷换成别的东西，出队结果就不再是「client 事件面的一步」"
    );
    assert_eq!(
        host_payload, "HostEvent",
        "host 角色 `NextEvent::Event` 的载荷不再是 `HostEvent`（实为 `{host_payload}`）——\
         载荷换成别的东西，出队结果就不再是「host 事件面的一步」"
    );
    assert_ne!(
        client_payload, host_payload,
        "两侧 `NextEvent::Event` 的载荷变成了同一个类型——\
         两个 enum 被判成「同形且同载荷」时，角色差异（host 不发起请求那条腿）就被抹平了；\
         若真要把两者合并，须显式改这条口径与本文件的登记面，不许静默同型"
    );
}

/// ③ **事件面的角色差集 = 登记面**：
/// `ClientEvent ⊆ HostEvent`，且 `HostEvent \ ClientEvent` 与 `HOST_ONLY_EVENTS`
/// **双向相等**（登记里的每项都在差集里，差集里的每项都登记过）。
#[test]
fn client_event_surface_is_a_subset_and_the_host_only_diff_is_registered() {
    let client = expect_variants(ROUTER_SOURCE, "ClientEvent", "router.rs");
    let host = expect_variants(HOST_ROUTER_SOURCE, "HostEvent", "host_router.rs");

    let missing: Vec<&String> = client.difference(&host).collect();
    assert!(
        missing.is_empty(),
        "client 事件面不再 ⊆ host 事件面：host 侧缺 {missing:?}\
         （client 实扫 {client:?} / host 实扫 {host:?}）——\
         两个角色由同一套 wire 类型驱动，host 认不得某个 client 认得的入站事件即归类表分叉"
    );

    let registered = registered_host_only();
    let diff = host_only_event_diff(
        ROUTER_SOURCE,
        "router.rs",
        HOST_ROUTER_SOURCE,
        "host_router.rs",
    );
    assert_eq!(
        diff, registered,
        "host 独有事件集与登记表 HOST_ONLY_EVENTS 不一致\
         （差集实扫 {diff:?}，登记 {registered:?}）——\
         差集里的每项都必须登记过（否则「两侧差集恰好这几项」这条承诺被静默改写）；\
         登记表里的每项也必须在差集里（否则该登记已失效）。\
         正当新增 host-only 事件：往 HOST_ONLY_EVENTS 加一项即可（一处改动）"
    );
}

/// ④ **定义点计数 + 别名拒绝 + 非空（防空自检）**：每个被检查的 enum 名在各自
/// 源文件里**恰有一处** `pub enum <Name> {` 定义、**没有**别名定义、且变体集合非空。
///
/// 0 处 ⇒ 被移走 / 改名，①②③ 会以「什么都没比」的方式假绿；多于 1 处 ⇒ 同一概念
/// 长出第二份手写实现，正是本卡要钉死的事；别名 ⇒ 「被检查的那个 enum」与
/// 「实际在用的那个」分叉。
#[test]
fn every_checked_enum_is_defined_exactly_once_and_scans_non_empty() {
    for (file, source, enum_name) in ENUM_POINTS {
        let sites = enum_definition_sites(source, enum_name);
        assert_eq!(
            sites, 1,
            "{file} 里 `pub enum {enum_name} {{` 有 {sites} 处（期望恰 1 处）——\
             0 处 ⇒ 枚举被移走 / 改名，本守卫扫成空集后全绿是假的；\
             多于 1 处 ⇒ 同一概念长出了第二份手写实现，本轮要钉死的正是这件事"
        );

        let aliases = enum_alias_lines(source, enum_name);
        assert!(
            aliases.is_empty(),
            "{file} 里出现了 {enum_name} 的别名定义 {aliases:#?}——\
             别名会让「被检查的那个 enum」与「实际在用的那个」分叉，本守卫必须显式拒绝"
        );

        let found = variant_names(source, enum_name, file);
        assert!(
            !found.is_empty(),
            "{file} 的 {enum_name} 扫出 0 个变体——解析零命中即显式失败，绝不默认放过\
             （防空集对空集式的假绿）"
        );
    }
}

/// ⑤ **合成样本自证（负向锚）**：把 host 的 `Idle` 改名成 `Paused` 的**假源文本**
/// 必须被判为「两侧不相等」；还原改名后又必须重新相等。
///
/// ⇒ 证明 ① 的相等断言**真在比较状态集**，不是恒绿摆设。
#[test]
fn a_state_renamed_on_one_side_would_be_caught() {
    let client = next_event_states(CLIENT_SOURCE, "client.rs");
    const SAMPLE_FILE: &str = "host_router.rs（合成样本）";

    // 合成样本：与真实 host 同形，但把 `Idle` 改名为 `Paused`（**不改生产代码**）。
    let renamed = "\
pub enum NextEvent {
    Event(HostEvent),
    Paused,
    Closed,
}
";
    let scanned = next_event_states(renamed, SAMPLE_FILE);
    assert_eq!(
        scanned,
        names(&["Event", "Paused", "Closed"]),
        "合成样本的变体名没被逐字读出来（实扫 {scanned:?}）——扫描器口径变了"
    );
    assert_ne!(
        scanned, client,
        "把 host 的 `Idle` 改名成 `Paused` 之后两侧竟然还判为「同形」——\
         ① 的相等断言没在真比较状态集，是恒绿摆设"
    );

    // 反向自证：把 `Paused` 还原成 `Idle`，同一个样本必须**重新**与 client 同集
    // ⇒ 差异确实来自那个变体名，不是解析器把整个样本读歪了。
    let restored = renamed.replace("Paused", "Idle");
    assert_eq!(
        next_event_states(&restored, SAMPLE_FILE),
        client,
        "只差一个变体名的两份样本被判成不同集、还原改名后又对不上——解析器没在读变体名"
    );
}

/// ⑥ **未知 / 未登记变体不得被静默忽略**：host 事件面多出一个**未登记**变体的合成
/// 样本，其差集必须**不再等于**登记表（且新变体确实进了差集）。
///
/// 同一条断言对**真实**源文件跑一遍 ⇒ 证明上面的「不相等」不是比较口径坏掉，
/// 而是登记面确实咬住了新增项。
#[test]
fn an_unregistered_host_only_event_is_not_silently_ignored() {
    let registered = registered_host_only();

    let extra = "\
pub enum HostEvent {
    Notification(JsonRpcNotification),
    ControlNotification(ControlNotification),
    ControlRequest(ControlRequest),
    UnexpectedResponse(JsonRpcResponse),
    SomethingNew(JsonRpcResponse),
}
";
    let diff = host_only_event_diff(
        ROUTER_SOURCE,
        "router.rs",
        extra,
        "host_router.rs（合成样本）",
    );
    assert!(
        diff.contains("SomethingNew"),
        "host 侧新增的未登记变体没进差集（差集实扫 {diff:?}）——未登记变体被静默忽略了"
    );
    assert_ne!(
        diff, registered,
        "合成样本多了个 host-only 变体，差集却仍等于登记表——\
         新增项被静默吸收，「差集恰好登记这几项」这条承诺会因此变成假的"
    );

    // 真实源文件：差集**必须**等于登记表（否则上一条的「不相等」毫无信息量）。
    assert_eq!(
        host_only_event_diff(
            ROUTER_SOURCE,
            "router.rs",
            HOST_ROUTER_SOURCE,
            "host_router.rs"
        ),
        registered,
        "真实源文件的 host 独有差集与登记表不等——登记面已漂移，须同步 HOST_ONLY_EVENTS"
    );
}

/// ⑦ **文件仍在该处（防移动假绿）**：三个源文件必须仍在 `include_str!` 绑定的路径上。
/// 编译期绑定挡得住「文件被删」，挡不住「路径被改对而内容换成了别的东西」
/// ⇒ 故用 `CARGO_MANIFEST_DIR` 拼路径再确认一次。
#[test]
fn the_three_source_files_still_exist() {
    for (label, rel) in SOURCE_PATHS {
        let path = source_path(rel);
        assert!(
            path.exists(),
            "{label} 源文件被移动了（期望在 {}）——include_str! 绑定的是旧路径，\
             本守卫会静默失效，须先核对路径再放行",
            path.display()
        );
    }
}
