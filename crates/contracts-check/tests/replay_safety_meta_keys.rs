//! `ReplaySafetyMeta` 三个冻结 wire 键的逐字拼写守卫（W2）。
//!
//! # 这条守卫治什么病
//!
//! `crates/agent-acp/src/replay.rs` 的 [`ReplaySafetyMeta`]（27 册 §1 规则
//! A27-1）挂三个**跨进程对端**直接按字符串消费的冻结 wire 键，经
//! `ReplayEvent.meta` 的载体键 `_meta` 挂上事件元数据：
//!
//! | 字段 | wire 键（serde rename） | 语义 |
//! |---|---|---|
//! | `history_replay` | `codebuddy.ai/historyReplay` | 历史重放产物标记 |
//! | `prompt_replay_safe` | `codebuddy.ai/promptReplaySafe` | 可安全重放标记 |
//! | `is_compact_internal` | `codebuddy.ai/isCompactInternal` | 压缩内部产物标记 |
//!
//! round-trip 测试不构成契约钉住：序列化/反序列化双边同走 serde，`rename`
//! 被改、键被误改成 snake_case 时依旧自洽照绿，炸的只有按字符串消费这些键的
//! 跨进程对端——静默失聪，零测试变红。既有 `tests/replay_surface.rs` 守的是
//! `session:replay` 分页语义、`contracts-check/src/replay.rs` 的
//! `REPLAY_DATA_KEYS` 是数据键，均不构成这三键的钉住（2026-10-24，base
//! fb1e0522 实测全 tests/ 对 `codebuddy.ai/` 与三键字面量零命中）。
//!
//! # 做法与口径
//!
//! `contracts-check` 的 dev-dependencies **没有** `agent-acp`，且 `Cargo.toml`
//! 属共享区不得改，照同族守卫（`tests/wire_tag_enum_trio.rs` 等）走**源码文本
//! 扫描**：`include_str!` 编译期绑定源文件文本（源文件被移动 = 编译期红）。
//!
//! - 期望表是**手写的冻结真相**：三键逐字面量，不做「点分后段」或大小写变换
//!   推导——推导跟着源码一起漂，钉不住。
//! - 判定抽成纯函数（输入 `&str` 源文本 → 键集合），真源断言与合成样本自证
//!   走同一批函数。
//! - 合成样本自证（照 `action_dispatch_surface.rs` 的自证先例）：键改拼、追加
//!   未登记键两类漂移必须真能红（真源不为测试而改）。

use std::collections::BTreeSet;

/// agent-acp replay 源文本（**编译期**绑定：源文件改动即触发重编）。
const REPLAY_SOURCE: &str = include_str!("../../agent-acp/src/replay.rs");

/// 冻结 wire 键的统一前缀（规则 A27-1，原样命名）。
const FROZEN_PREFIX: &str = "codebuddy.ai/";

/// 三键期望表（手写逐字冻结真相；正当登记新键 = 同步加一行，不红）。
const EXPECTED_KEYS: &[&str] = &[
    "codebuddy.ai/historyReplay",
    "codebuddy.ai/promptReplaySafe",
    "codebuddy.ai/isCompactInternal",
];

/// 载体键：三键挂在事件元数据 `_meta` 上（`ReplayEvent.meta` 的 serde rename）。
const META_CARRIER_KEY: &str = "_meta";

// ---- 判定（纯函数：&str 源文本 → 集合；真源断言与合成样本自证共用）----

/// 扫源文本里全部 `rename = "…"` 字面量取值。
///
/// 左边界不得是标识符字符（防误吸 `xxrename = "…"`）；`rename_all = "…"` 的
/// `rename` 后紧跟 `_`，天然不匹配本针。多行 `#[serde(...)]` 属性与单行形态
/// 都能扫到（按整段文本找针，不按行解析属性）。
fn scan_rename_values(source: &str) -> BTreeSet<String> {
    let needle = "rename = \"";
    let mut out = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = source[from..].find(needle) {
        let at = from + rel;
        let value_start = at + needle.len();
        let left_ok = at == 0
            || !source[..at]
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if left_ok {
            if let Some(end) = source[value_start..].find('"') {
                out.insert(source[value_start..value_start + end].to_owned());
            }
        }
        from = value_start;
    }
    out
}

/// 判定纯函数：源文本 → `codebuddy.ai/` 前缀的 wire 键集合。
fn scan_frozen_keys(source: &str) -> BTreeSet<String> {
    scan_rename_values(source)
        .into_iter()
        .filter(|k| k.starts_with(FROZEN_PREFIX))
        .collect()
}

/// 双向差集：`(实际有而期望缺的多余项, 期望有而实际缺的缺失项)`。
/// 多余项按集合序输出（确定性），缺失项按期望表序输出。
fn frozen_key_diff(actual: &BTreeSet<String>) -> (Vec<String>, Vec<&'static str>) {
    let expected: BTreeSet<&str> = EXPECTED_KEYS.iter().copied().collect();
    let extra: Vec<String> = actual
        .iter()
        .filter(|k| !expected.contains(k.as_str()))
        .cloned()
        .collect();
    let missing: Vec<&'static str> = EXPECTED_KEYS
        .iter()
        .copied()
        .filter(|k| !actual.contains(*k))
        .collect();
    (extra, missing)
}

// ---- 断言面 ----

/// ① **扫描器防空扫**：真源扫出的 `rename = "…"` 字面量集合必须非空、且至少
/// 包含三键——扫描器坏掉/源形态变了导致扫成空集时，这里显式红，
/// 绝不让后续断言「空对空全绿」。
#[test]
fn scanner_is_not_vacuous_on_real_source() {
    let renames = scan_rename_values(REPLAY_SOURCE);
    assert!(
        !renames.is_empty(),
        "真源一个 rename = \"…\" 字面量都没扫到——扫描器或源形态坏了，本守卫其余断言全是空对空"
    );
    let frozen = scan_frozen_keys(REPLAY_SOURCE);
    for key in EXPECTED_KEYS {
        assert!(
            frozen.contains(*key),
            "扫描器防空扫失败：期望键 {key} 不在扫出的 codebuddy.ai/ 键集合 {frozen:?} 里"
        );
    }
}

/// ② **三键逐字钉死**：三键必须逐字出现在真源的 `rename = "…"` 字面量里；
/// 缺失时打出 `missing` 并注明漂移后果。
#[test]
fn three_frozen_wire_keys_are_pinned_verbatim() {
    let frozen = scan_frozen_keys(REPLAY_SOURCE);
    let (extra, missing) = frozen_key_diff(&frozen);
    assert!(
        missing.is_empty(),
        "A27-1 冻结 wire 键拼写漂移 ⇒ 跨进程对端静默失聪：missing: {missing:?}\
         （扫出集合 {frozen:?}，extra: {extra:?}）——冻结键是逐字真相，对端按字符串消费"
    );
}

/// ③ **载体键 `_meta` 钉死**：`ReplayEvent.meta` 的 `rename = "_meta"` 必须在
/// 真源——三个回放键全挂在事件元数据 `_meta` 上，载体键漂移同样断链。
#[test]
fn meta_carrier_key_is_pinned() {
    let renames = scan_rename_values(REPLAY_SOURCE);
    assert!(
        renames.contains(META_CARRIER_KEY),
        "`ReplayEvent.meta` 的载体键漂了（扫出的 rename 集合 {renames:?}，期望 \
         rename = \"{META_CARRIER_KEY}\"）——三个回放安全键挂在事件元数据 \
         {META_CARRIER_KEY} 上，载体键漂移同样断链"
    );
}

/// ④ **不得漂移成其他拼写**：真源扫出的 `codebuddy.ai/` 前缀键集合必须与期望
/// 三键集合**双向相等**——多出的前缀键视为未登记新键，缺了的视为拼写漂移。
#[test]
fn codebuddy_prefixed_keys_bidirectionally_equal_expected() {
    let frozen = scan_frozen_keys(REPLAY_SOURCE);
    let (extra, missing) = frozen_key_diff(&frozen);
    assert!(
        extra.is_empty() && missing.is_empty(),
        "codebuddy.ai/ 前缀键与 A27-1 期望表不同步：\
         extra: {extra:?}（新键须先登记 A27-1 口径）；\
         missing: {missing:?}（A27-1 冻结 wire 键拼写漂移 ⇒ 跨进程对端静默失聪）"
    );
}

/// ⑤ **合成样本自证**（照 `action_dispatch_surface.rs` 的自证先例）：用真源
/// 文本的内存副本构造两类漂移，证明判定逻辑真能红——键改拼 ⇒ 报
/// `missing`；追加未登记键 ⇒ 报 `extra`。真源本身不为测试而改。
#[test]
fn synthetic_drifts_are_caught_by_the_judgment() {
    // 基线：真源判定必须干净（双向相等），否则下面各断言失去意义。
    let baseline = frozen_key_diff(&scan_frozen_keys(REPLAY_SOURCE));
    assert_eq!(baseline, (vec![], vec![]), "基线自证失败：真源本应双向相等");

    // 漂移 A：真源副本上把一个键改拼成 zzzNope ⇒ 旧键必须报 missing、
    // 新词必须报 extra（判定逻辑对「拼写漂移」敏感）。
    let respelled = REPLAY_SOURCE.replace("codebuddy.ai/historyReplay", "codebuddy.ai/zzzNope");
    assert_ne!(respelled, REPLAY_SOURCE, "替换没生效，样本构造有误");
    let (extra, missing) = frozen_key_diff(&scan_frozen_keys(&respelled));
    assert_eq!(
        missing,
        vec!["codebuddy.ai/historyReplay"],
        "键被改拼后判定逻辑必须报 missing"
    );
    assert_eq!(
        extra,
        vec!["codebuddy.ai/zzzNope".to_owned()],
        "改拼出来的新词必须同时报 extra"
    );

    // 漂移 B：真源副本上追加一行未登记键 ⇒ extra 非空、missing 仍空
    // （判定逻辑对「未登记新键」敏感）。
    let with_extra = format!("{REPLAY_SOURCE}\n#[serde(rename = \"codebuddy.ai/extraKey\")]\n");
    let (extra, missing) = frozen_key_diff(&scan_frozen_keys(&with_extra));
    assert_eq!(
        extra,
        vec!["codebuddy.ai/extraKey".to_owned()],
        "未登记新键必须报 extra（新键须先登记 A27-1 口径）"
    );
    assert!(missing.is_empty(), "追加键不该造成缺失：{missing:?}");

    // 扫描器自检：没有 rename 的文本扫出空集——空集本身是真空而非扫描器
    // 假空；真源非空由断言 ① 背书。
    assert!(scan_frozen_keys("fn f() {}").is_empty());
}
