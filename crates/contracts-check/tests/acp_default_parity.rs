//! ACP 协议**默认参数**（轮询间隔 / 事件队列上限）多份手写实现的一致性守卫。
//!
//! # 这条断言治什么病
//!
//! 「ACP 默认参数」这个概念在 `crates/agent-acp` 内被**五处**各自手写，彼此**零断言绑定**：
//!
//! | 概念 | 位置 | 常量 |
//! |---|---|---|
//! | 轮询间隔 | `src/client.rs` | `pub const DEFAULT_POLL_INTERVAL` |
//! | 轮询间隔 | `src/host.rs` | `pub const DEFAULT_POLL_INTERVAL` |
//! | 轮询间隔 | `src/host_router.rs` | `pub(crate) const POLL_INTERVAL` |
//! | 事件队列上限 | `src/client.rs` | `pub const DEFAULT_MAX_EVENTS` |
//! | 事件队列上限 | `src/host_router.rs` | `pub const DEFAULT_MAX_EVENTS` |
//!
//! 且 `host_router.rs` 的文档注释用**散文**承诺了「与 `crate::client::DEFAULT_MAX_EVENTS`
//! 同值」⇒ 一个**没人检查**的不变式：散文写下的承诺，改一侧不会有任何东西变红。
//! （`crates/*/tests` 此前对这两个常量名**零命中**。）
//!
//! # 为什么这是真风险（不是理论）
//!
//! - **轮询间隔**同时决定「分派线程轮询 / `Host::stop` 的响应延迟上界」与「client
//!   `close` 的响应延迟上界」⇒ 两侧取值一旦发散（如 client 50ms / host 100ms），
//!   「一次调用最多等多久」这条**跨端隐含契约**就静默不一致，且无人察觉。
//! - **事件队列上限**是 host 侧与 client 侧队列容量的默认值 ⇒ 发散即「一侧队列先满
//!   丢最旧事件、另一侧不丢」的**事件观测量不对称**。
//!
//! # 口径（只守关系，不锁取值）
//!
//! `agent-acp` **不在**本 crate 的 dev-dependencies 里，而 `Cargo.toml` 属共享区、
//! 本卡不得改 ⇒ 照本 crate 同类测试（`tests/task_status_drift_guard.rs`、
//! `tests/acp_jsonrpc_envelope_keys.rs`）的先例：`include_str!` **编译期**绑定源文件
//! 文本 + 手写扫字面量，**不 import 被检查的 crate**、**不引入正则**。
//!
//! ⛔ 只守**关系**——三处轮询间隔相等、两处队列上限相等、每个定义点恰出现一次、
//! 取值落在合理区间——**不锁**具体数值 ⇒ 正当调整（如三处同步 50ms → 100ms）必不红。
//! 解析不出数字时**显式失败**（`expect_value`），绝不默认放过 ⇒ 不会「扫成空集后全绿」。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// `client.rs` 源文件文本（**编译期**绑定：源文件改动即触发重编）。
const CLIENT_SOURCE: &str = include_str!("../../agent-acp/src/client.rs");
/// `host.rs` 源文件文本（**编译期**绑定）。
const HOST_SOURCE: &str = include_str!("../../agent-acp/src/host.rs");
/// `host_router.rs` 源文件文本（**编译期**绑定）。
const HOST_ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/host_router.rs");

/// 被绑定源文件的相对路径（供「文件仍在该处」的存在性断言用）。
const SOURCE_PATHS: [(&str, &str); 3] = [
    ("client.rs", "../agent-acp/src/client.rs"),
    ("host.rs", "../agent-acp/src/host.rs"),
    ("host_router.rs", "../agent-acp/src/host_router.rs"),
];

/// **全部定义点**：`(文件标签, 源文件文本, 常量名)`——本守卫要钉死的整个定义面。
///
/// 单表唯一来源：轮询间隔与队列上限两组断言都从这张表**筛出来**，避免再抄一份
/// 常量清单（否则清单本身就又成了会漂移的第二份手写实现）。
const DEFINITION_POINTS: [(&str, &str, &str); 5] = [
    ("client.rs", CLIENT_SOURCE, "DEFAULT_POLL_INTERVAL"),
    ("client.rs", CLIENT_SOURCE, "DEFAULT_MAX_EVENTS"),
    ("host.rs", HOST_SOURCE, "DEFAULT_POLL_INTERVAL"),
    ("host_router.rs", HOST_ROUTER_SOURCE, "POLL_INTERVAL"),
    ("host_router.rs", HOST_ROUTER_SOURCE, "DEFAULT_MAX_EVENTS"),
];

/// 1 毫秒的纳秒数（取值口径统一到纳秒，`from_millis` / `from_secs` 都能比）。
const NANOS_PER_MILLI: u64 = 1_000_000;
/// 轮询间隔的合理上界（毫秒）：超过这个量级，「一次调用的响应延迟上界」就失控了。
const MAX_POLL_INTERVAL_MILLIS: u64 = 60_000;

// ---------------------------------------------------------------- 扫描器 ----

/// 一行是不是注释：去掉缩进后以 `//` 开头（只认行注释；被扫的源文件里没有块注释）。
fn is_comment_line(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// 扫出**常量定义行**：去掉缩进后，`const <name>:` 之前**只允许**可见性修饰
/// ⇒ 注释（文档散文里的常量名）与正文里对常量的**引用**都不会被读成定义。
fn definition_lines<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!("const {name}:");
    source
        .lines()
        .filter(|line| {
            if is_comment_line(line) {
                return false;
            }
            let trimmed = line.trim_start();
            let Some(at) = trimmed.find(&needle) else {
                return false;
            };
            let visibility = trimmed[..at].trim();
            visibility.is_empty()
                || visibility
                    .strip_prefix("pub")
                    .map(|rest| {
                        rest.is_empty()
                            || (rest.starts_with('(')
                                && rest.ends_with(')')
                                && rest
                                    .chars()
                                    .all(|c| c.is_ascii_alphanumeric() || "()_".contains(c)))
                    })
                    .unwrap_or(false)
        })
        .collect()
}

/// 某源文件里某个常量的**定义行数**（防空自检：解析成空集时断言必须能有话说）。
fn definition_count(source: &str, name: &str) -> usize {
    definition_lines(source, name).len()
}

/// 解析无符号整数字面量（允许 `_` 分组；非法即 `None`，**不**默认成 0）。
fn parse_unsigned(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    let trimmed = digits.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

/// 把定义行里的 `Duration::from_<单位>(N)` 解析成**纳秒**（认 `nanos` / `micros` /
/// `millis` / `secs`）⇒ 三处一起换单位写法时比的仍是**同一件事**（时长，不假红）；
/// 认不出的写法 ⇒ `None` ⇒ 上层**显式失败**，绝不悄悄按 0 或按默认值放过。
fn poll_interval_nanos(source: &str, name: &str) -> Option<u64> {
    let line = *definition_lines(source, name).first()?;
    let after_from = line.split("from_").nth(1)?;
    let (unit, tail) = after_from.split_once('(')?;
    let (digits, _) = tail.split_once(')')?;
    let unit_nanos = match unit {
        "nanos" => 1,
        "micros" => 1_000,
        "millis" => NANOS_PER_MILLI,
        "secs" => 1_000 * NANOS_PER_MILLI,
        _ => return None,
    };
    Some(parse_unsigned(digits)? * unit_nanos)
}

/// 解析 `const <name>: usize = <N>;` 里 `=` 与 `;` 之间的字面量（类型标注那段落在
/// `:` 与 `=` 之间，天然被跳过）；解析不出数字 ⇒ `None` ⇒ 上层**显式失败**。
fn max_events_value(source: &str, name: &str) -> Option<u64> {
    let line = *definition_lines(source, name).first()?;
    let after_eq = line.split_once('=')?.1;
    parse_unsigned(after_eq.split(';').next()?)
}

/// 取一个**必须解析出来**的取值；解析不出即 panic（不得默认放过）。
fn expect_value(value: Option<u64>, what: &str) -> u64 {
    value.unwrap_or_else(|| {
        panic!(
            "解析不出 {what} 的数字——源文件的写法变了（本守卫只认 `Duration::from_<单位>(N)` / \
             `= N;` 这两种形态）。死守这条：解析不出来就红，绝不默认放过，\
             否则「扫成空集 ⇒ 后面全绿」的假绿无从发现"
        )
    })
}

/// 从**唯一**定义点清单里筛出轮询间隔那三处（`POLL_INTERVAL` 与 `DEFAULT_POLL_INTERVAL`）。
fn poll_definition_points() -> Vec<(&'static str, &'static str, &'static str)> {
    DEFINITION_POINTS
        .iter()
        .copied()
        .filter(|(_, _, name)| name.ends_with("POLL_INTERVAL"))
        .collect()
}

/// 从**唯一**定义点清单里筛出事件队列上限那两处。
fn max_events_definition_points() -> Vec<(&'static str, &'static str, &'static str)> {
    DEFINITION_POINTS
        .iter()
        .copied()
        .filter(|(_, _, name)| name.ends_with("MAX_EVENTS"))
        .collect()
}

/// 被检查源文件的绝对路径（`CARGO_MANIFEST_DIR` + 相对路径）。
fn source_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

// ------------------------------------------------------------------ 断言 ----

/// ① **三处轮询间隔取值相等**（不锁具体数值）：三处各自决定一个「响应延迟上界」，
/// 取值发散即「一次调用最多等多久」这条**跨端隐含契约**静默失效。
#[test]
fn the_three_poll_intervals_agree() {
    let points = poll_definition_points();
    assert_eq!(
        points.len(),
        3,
        "定义点清单里轮询间隔不再是 3 处（实为 {} 处）——清单缺项会让本守卫少守一处",
        points.len()
    );

    let mut observed: Vec<(&str, u64)> = Vec::new();
    for (file, source, name) in points {
        let what = format!("{file} 的 {name}");
        observed.push((file, expect_value(poll_interval_nanos(source, name), &what)));
    }

    let distinct: BTreeSet<u64> = observed.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        distinct.len(),
        1,
        "三处轮询间隔取值发散（实测 {observed:?}，单位纳秒）——\
         client / host / host_router 各决定一个「响应延迟上界」，两侧不等即跨端隐含契约失效。\
         三处必须同改（正当调整：同时改成同一个新值即绿）"
    );
}

/// ② **两处事件队列上限取值相等**（不锁具体数值）：给 `host_router.rs` 文档注释里
/// 那句「与 `crate::client::DEFAULT_MAX_EVENTS` 同值」的散文承诺补上机器可读的对应物。
#[test]
fn the_two_event_queue_limits_agree() {
    let points = max_events_definition_points();
    assert_eq!(
        points.len(),
        2,
        "定义点清单里事件队列上限不再是 2 处（实为 {} 处）——清单缺项会让本守卫少守一处",
        points.len()
    );

    let mut observed: Vec<(&str, u64)> = Vec::new();
    for (file, source, name) in points {
        let what = format!("{file} 的 {name}");
        observed.push((file, expect_value(max_events_value(source, name), &what)));
    }

    let distinct: BTreeSet<u64> = observed.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        distinct.len(),
        1,
        "两处事件队列上限取值发散（实测 {observed:?}）——\
         一侧队列先满丢最旧事件、另一侧不丢 ⇒ 事件观测量静默不对称"
    );
}

/// ③ **定义点计数（防空自检）**：每个文件里对应常量的定义行**恰好 1 次**——
/// 扫不到 ⇒ 常量被移走/改名，①②④ 会以「什么都没比」的方式假绿；
/// 多于一处 ⇒ 同一概念又长出了第二份手写实现，正是本卡要钉死的事。
#[test]
fn every_definition_point_appears_exactly_once() {
    for (file, source, name) in DEFINITION_POINTS {
        let count = definition_count(source, name);
        assert_eq!(
            count, 1,
            "{file} 里 `const {name}:` 的定义行有 {count} 处（期望恰 1 处）——\
             0 处 ⇒ 常量被移走/改名，本守卫扫成空集后全绿是假的；\
             多于 1 处 ⇒ 同一概念又长出了第二份手写实现，本轮要钉死的正是这件事"
        );
    }
}

/// ④ **取值合理性（防空转）**：轮询间隔落在 `[1ms, 60s]`，队列上限 `>= 1`。
///
/// 解析不出数字时 `expect_value` 直接 panic ⇒ 「解析不出来」本身让测试红，
/// 不存在「拿不到值就当通过」这条路。
#[test]
fn parsed_defaults_are_within_a_sane_range() {
    for (file, source, name) in poll_definition_points() {
        let what = format!("{file} 的 {name}");
        let nanos = expect_value(poll_interval_nanos(source, name), &what);
        assert!(
            nanos >= NANOS_PER_MILLI,
            "{what} 解析出 {nanos} 纳秒（< 1 毫秒）——轮询过密，退化成忙等"
        );
        assert!(
            nanos <= MAX_POLL_INTERVAL_MILLIS * NANOS_PER_MILLI,
            "{what} 解析出 {nanos} 纳秒（> {} 毫秒）——\
             一次调用的响应延迟上界过大，且 stop/close 的收敛速度失控",
            MAX_POLL_INTERVAL_MILLIS
        );
    }

    for (file, source, name) in max_events_definition_points() {
        let what = format!("{file} 的 {name}");
        let max_events = expect_value(max_events_value(source, name), &what);
        assert!(
            max_events >= 1,
            "{what} 解析出 {max_events}——队列上限为 0 时事件如实时丢弃，\
             事件观测量退化为「什么都看不到」"
        );
    }
}

/// ⑤ **文件仍在该处（防移动假绿）**：三个源文件必须仍在 `include_str!` 绑定的路径上。
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

/// ⑥ **负向锚**：对**不存在**的常量名（含**只出现在注释里**的名字），解析结果必须为空
/// ⇒ 证明「扫到了」不是解析器的假阳（否则 ① 是永远绿的空断言）。
#[test]
fn the_scanner_finds_nothing_for_a_name_that_is_not_defined() {
    const ABSENT: &str = "THIS_CONSTANT_DOES_NOT_EXIST";

    for (file, source) in [
        ("client.rs", CLIENT_SOURCE),
        ("host.rs", HOST_SOURCE),
        ("host_router.rs", HOST_ROUTER_SOURCE),
    ] {
        assert!(
            definition_lines(source, ABSENT).is_empty(),
            "{file} 里居然「找到」了不存在的常量 {ABSENT}——扫描器在乱认定义，\
             ① 的相等断言会因此变成空对空"
        );
        assert_eq!(
            poll_interval_nanos(source, ABSENT),
            None,
            "{file} 对不存在的常量 {ABSENT} 解析出了取值——解析器假阳"
        );
        assert_eq!(
            max_events_value(source, ABSENT),
            None,
            "{file} 对不存在的常量 {ABSENT} 解析出了取值——解析器假阳"
        );
    }

    // 同一宿主文件里的**另一个**真实常量名也不该被误认成它：`POLL_INTERVAL`
    // 与 `DEFAULT_POLL_INTERVAL` 是前缀关系，扫描口径必须能区分两者。
    assert_eq!(
        definition_count(CLIENT_SOURCE, "POLL_INTERVAL"),
        0,
        "`client.rs` 只有 `DEFAULT_POLL_INTERVAL`，`POLL_INTERVAL` 不该命中——\
         常量名匹配成了子串匹配"
    );
    assert_eq!(
        definition_count(HOST_ROUTER_SOURCE, "DEFAULT_POLL_INTERVAL"),
        0,
        "`host_router.rs` 只有 `POLL_INTERVAL`，`DEFAULT_POLL_INTERVAL` 不该命中——\
         常量名匹配成了子串匹配"
    );

    // 只出现在**注释**里的定义不算定义（否则文档散文会被读成实现）。
    let commented = "/// pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);\n\
                     // pub const DEFAULT_MAX_EVENTS: usize = 4096;\n";
    assert!(
        definition_lines(commented, "DEFAULT_POLL_INTERVAL").is_empty()
            && definition_lines(commented, "DEFAULT_MAX_EVENTS").is_empty(),
        "注释行里的常量被当成定义了——排除注释的口径失效，\
         被注释掉的旧取值会冒充现行定义"
    );
}

/// ⑦ **扫描器真在读字面量**（合成样本，**不改生产代码**）：换一个取值后必须扫出
/// **新值**、且**不等于**真实值 ⇒ 证明 ① 咬得住漂移，不是「恒返回同一个常量」的摆设。
#[test]
fn the_scanner_reads_the_literal_so_a_diverged_interval_could_be_caught() {
    let real = expect_value(
        poll_interval_nanos(CLIENT_SOURCE, "DEFAULT_POLL_INTERVAL"),
        "client.rs 的 DEFAULT_POLL_INTERVAL",
    );

    let other = real + NANOS_PER_MILLI; // 比真实值多 1 毫秒（纳秒口径）
    let sample =
        format!("pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_nanos({other});\n");

    assert_eq!(
        poll_interval_nanos(&sample, "DEFAULT_POLL_INTERVAL"),
        Some(other),
        "合成样本的取值没被读出来——扫描器没在读字面量"
    );
    assert_ne!(
        poll_interval_nanos(&sample, "DEFAULT_POLL_INTERVAL"),
        Some(real),
        "换了个取值却仍扫出原值——扫描器恒返回某个常量，① 的相等断言是摆设"
    );

    // 单位换算这一层也要真在算：同一时长用 `from_millis` 写，扫出的纳秒必须一致。
    let millis = other / NANOS_PER_MILLI;
    assert_eq!(
        poll_interval_nanos(
            &format!("pub const POLL_INTERVAL: Duration = Duration::from_millis({millis});\n"),
            "POLL_INTERVAL",
        ),
        Some(other),
        "`from_millis` 与 `from_nanos` 没换算到同一口径——三处「相等」会因单位写法不同而误判"
    );
}
