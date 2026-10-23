//! 双源 `TaskStatus` 漂移守卫（ACP wire ↔ 内部任务态 映射契约）。
//!
//! # 这条断言治什么病
//!
//! 本仓存在**两个同名 `TaskStatus` 枚举**，且**彼此没有任何映射代码**：
//!
//! | 源 | 位置 | 变体（serde snake_case） |
//! |---|---|---|
//! | ACP 线上协议 | `crates/agent-acp/src/tasks.rs` | `Pending / InProgress / Completed / Failed / Cancelled`（5） |
//! | 内部任务机 | `crates/agent-tasks/src/task.rs` | `Pending / InProgress / Done / Cancelled`（4） |
//!
//! 实测（于本仓内跑）：
//!
//! - `grep -rn "TaskStatus" crates/agent-gateway/src/ crates/agent-acp/src/ | grep -iE "from|into|map|convert"` ⇒ **零命中**
//! - `grep -rln "agent_tasks::" crates/agent-gateway/src/ crates/contracts-check/` ⇒ **零命中**
//!
//! ⇒ 网关把内部任务态推给 ACP client 时**没有显式映射**，且两侧**语义已漂移**：
//! ① wire 用 `completed`、内部用 `done`（同义不同名）；② wire 有 `failed`（终态），
//! 内部**没有** `failed`；③ 内部 `Done` 与 wire `Completed` 是否等价**无任何文档或断言**。
//!
//! ⇒ 任何一侧重命名 / 增删变体，另一方会**静默不匹配**（fail-open）。
//!
//! # 做法与口径
//!
//! 照抄本 crate 同类测试（`tests/acp_control_surface.rs`、`tests/tasks_method_surface.rs`）
//! 的手法：**读源码文本 + 手写解析核对契约，不 import 被检查的 crate**
//! （`contracts-check` 的 dev-dependencies 里**没有** `agent-acp` / `agent-tasks`，
//! 而 `Cargo.toml` 属共享区、本卡**不得**改）。本文件用 `include_str!` **编译期绑定**
//! 两份源文件文本，另用 `CARGO_MANIFEST_DIR` 相对路径断言**文件仍在该处**（防移动后假绿）。
//!
//! ⛔ **本卡不新建任何映射实现**——只把「期望映射表 + 现状缺席」钉死为可执行断言。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// ACP wire 侧源文件文本（**编译期**绑定：源文件改动即触发重编）。
const WIRE_SOURCE: &str = include_str!("../../agent-acp/src/tasks.rs");
/// 内部任务机侧源文件文本（**编译期**绑定）。
const INTERNAL_SOURCE: &str = include_str!("../../agent-tasks/src/task.rs");

/// 相对 `CARGO_MANIFEST_DIR` 的路径（供「文件仍在该处」的存在性断言用）。
const WIRE_SOURCE_REL: &str = "../agent-acp/src/tasks.rs";
/// 内部任务机侧相对路径。
const INTERNAL_SOURCE_REL: &str = "../agent-tasks/src/task.rs";

/// **唯一合法映射表**（内部 → wire，全量覆盖 4/4）。
///
/// 这张表就是契约：内部每个变体**恰有一个** wire 目标；`Done → Completed`
/// 是**有意的**同义改名（见 `done_and_completed_are_deliberately_different_wire_words`）。
const INTERNAL_TO_WIRE: &[(&str, &str)] = &[
    ("Pending", "Pending"),
    ("InProgress", "InProgress"),
    ("Done", "Completed"),
    ("Cancelled", "Cancelled"),
];

/// **wire 独有**变体：在内部任务机里**没有**对应态 ⇒ 反向映射必须**显式失败**。
const WIRE_ONLY: &[&str] = &["Failed"];
/// **内部独有**变体：在 wire 上**没有**对应态（当前为空）。
const INTERNAL_ONLY: &[&str] = &[];

/// 扫出 `pub enum <enum_name> { … }` 的**变体名**（保留源码顺序）。
///
/// 口径：定位 `pub enum <enum_name> {` 那一行，**逐行**吃到该枚举的收尾 `}`；
/// 跳过空行、`//`（含 `///` 文档注释）与 `#[…]` 属性行（如内部态的 `#[default]`）；
/// 其余形如 `名字,` 的行即变体声明。找不到枚举头即 panic（源文件形态变了不许静默跳过）。
fn scan_enum_variants(source: &str, enum_name: &str) -> Vec<String> {
    let header = format!("pub enum {enum_name} {{");
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with(&header))
        .unwrap_or_else(|| panic!("源码里找不到 `{header}`——源文件形态变了"));

    let mut out = Vec::new();
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed == "}" {
            break; // 枚举收尾
        }
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
            continue; // 空行 / 文档注释 / 属性行
        }
        let name = trimmed.trim_end_matches(',').trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue; // 不是变体声明
        }
        out.push(name.to_owned());
    }
    out
}

/// 扫出 `pub enum <enum_name>` 的**容器属性** `#[serde(rename_all = "…")]` 的取值。
///
/// 只在枚举声明**上方三行**内找 ⇒ 不会误吸文件里其它类型（如 `struct Task`）的
/// 同名属性。
fn scan_rename_all(source: &str, enum_name: &str) -> Option<String> {
    let header = format!("pub enum {enum_name} {{");
    let lines: Vec<&str> = source.lines().collect();
    let enum_at = lines
        .iter()
        .position(|line| line.trim_start().starts_with(&header))?;
    lines[..enum_at].iter().rev().take(3).find_map(|line| {
        let rest = line.trim().strip_prefix("#[serde(rename_all = ")?;
        let value = rest.strip_prefix('"')?;
        let end = value.find('"')?;
        Some(value[..end].to_owned())
    })
}

/// serde `rename_all = "snake_case"` 的派生规则（**逐字复刻** serde：每个**大写**
/// 字符前插 `_`，再转小写）。
fn serde_snake_case(variant: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in variant.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

/// 一串可转 `String` 的项 → `BTreeSet<String>`（取所有权，避开临时值生命周期问题）。
fn as_set<I, S>(items: I) -> BTreeSet<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    items.into_iter().map(Into::into).collect()
}

/// 扫出某个 `pub enum` 的变体名集合。
fn variants_set(source: &str, enum_name: &str) -> BTreeSet<String> {
    as_set(scan_enum_variants(source, enum_name))
}

/// 被检查源文件的绝对路径（`CARGO_MANIFEST_DIR` + 相对路径）。
fn source_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// ① **wire 变体集合钉死**：`agent-acp` 侧必须恰为这 5 个。
#[test]
fn wire_task_status_variants_are_pinned() {
    let actual = variants_set(WIRE_SOURCE, "TaskStatus");
    let expected = as_set(["Pending", "InProgress", "Completed", "Failed", "Cancelled"]);
    assert_eq!(
        actual, expected,
        "ACP wire TaskStatus 变体集合漂移（实扫 {actual:?}，期望 {expected:?}）——\
         这份集合是被下游 client 直接消费的线上词，改一词即破坏兼容"
    );
}

/// ② **内部变体集合钉死**：`agent-tasks` 侧必须恰为这 4 个。
#[test]
fn internal_task_status_variants_are_pinned() {
    let actual = variants_set(INTERNAL_SOURCE, "TaskStatus");
    let expected = as_set(["Pending", "InProgress", "Done", "Cancelled"]);
    assert_eq!(
        actual, expected,
        "内部 TaskStatus 变体集合漂移（实扫 {actual:?}，期望 {expected:?}）——\
         内部任务机状态机是迁移矩阵（Pending→InProgress→Done/Cancelled）的前提"
    );
}

/// ③ **serde 规则钉死**：两侧都必须声明 `#[serde(rename_all = "snake_case")]`。
///
/// 这是「变体名 ⇒ wire 词」这半边的前提：规则一变，派生词就变，本文件的推导函数
/// 与映射表都必须同步。
#[test]
fn both_enums_declare_snake_case_rename_all() {
    assert_eq!(
        scan_rename_all(WIRE_SOURCE, "TaskStatus").as_deref(),
        Some("snake_case"),
        "ACP wire TaskStatus 的 serde 派生规则不再是 snake_case——\
         「变体名 ⇒ wire 词」的前提变了，映射表与本文档须同步"
    );
    assert_eq!(
        scan_rename_all(INTERNAL_SOURCE, "TaskStatus").as_deref(),
        Some("snake_case"),
        "内部 TaskStatus 的 serde 派生规则不再是 snake_case——\
         `as_str()` 与派生链的前提变了，映射表与本文档须同步"
    );
}

/// ④ **已文档化的差异钉死**：内部 `Done ⇒ "done"`、wire `Completed ⇒ "completed"`。
///
/// ⚠️ **两者不是同一个 wire 词**——这是**有意的**同义改名，**禁止静默改名**：
/// 谁把内部 `Done` 直接当作 wire 词推给 client，谁就制造了 `done` vs `completed`
/// 的静默不匹配（fail-open）。跨这条边界的唯一合法通道是本文件钉死的映射表。
#[test]
fn done_and_completed_are_deliberately_different_wire_words() {
    assert_eq!(serde_snake_case("Done"), "done");
    assert_eq!(serde_snake_case("Completed"), "completed");
    assert_ne!(
        serde_snake_case("Done"),
        serde_snake_case("Completed"),
        "内部 `Done` 与 wire `Completed` 派生出了**同一个** wire 词——\
         要么 serde 规则变了，要么有人把两侧改名成了同名；无论哪种，\
         本文档「两者不是同一个 wire 词」的前提都失效了，须同步"
    );

    // 映射表正是架在这条差异上的桥：内部 Done 唯一合法地译到 wire Completed。
    let done_target = INTERNAL_TO_WIRE
        .iter()
        .find(|(internal, _)| *internal == "Done")
        .map(|(_, wire)| *wire);
    assert_eq!(
        done_target,
        Some("Completed"),
        "映射表里内部 `Done` 的 wire 目标不再是 `Completed`——\
         这正是「同义不同名」的桥，桥键被改即漂移，禁止静默改名"
    );
}

/// ⑤ **映射表全量覆盖钉死**：内部每个变体**恰有一个** wire 目标（用集合大小断言，不手数）。
#[test]
fn mapping_table_covers_every_internal_variant_exactly_once() {
    let internal = variants_set(INTERNAL_SOURCE, "TaskStatus");

    // 键集合 == 内部变体集合，且**无重复键**（集合大小等于内部变体数即为「恰一个」）。
    let keys = as_set(INTERNAL_TO_WIRE.iter().map(|(i, _)| *i));
    assert_eq!(
        keys.len(),
        internal.len(),
        "映射表键数 {} ≠ 内部变体数 {}（有内部变体漏映射或重复映射）——\
         映射表必须对内部 4/4 全量覆盖",
        keys.len(),
        internal.len()
    );
    assert_eq!(
        keys, internal,
        "映射表键集合与内部变体集合不一致（实为 {keys:?} / {internal:?}）——\
         内部增删变体后映射表未同步"
    );

    // 目标**互不相同**：两个内部变体不得映到同一个 wire 词（否则反解歧义）。
    let targets = as_set(INTERNAL_TO_WIRE.iter().map(|(_, w)| *w));
    assert_eq!(
        targets.len(),
        INTERNAL_TO_WIRE.len(),
        "有内部变体映射到同一个 wire 词——反解不唯一，契约歧义"
    );

    // 目标必须都是真实存在的 wire 变体（防映射表指向幻影词）。
    let wire = variants_set(WIRE_SOURCE, "TaskStatus");
    let phantom: Vec<&String> = targets.difference(&wire).collect();
    assert!(
        phantom.is_empty(),
        "映射表指向了 wire 上不存在的词 {phantom:?}——映射表腐化"
    );
}

/// ⑥ **fail-closed 钉死**：wire `Failed` 在内部**没有**对应态。
///
/// 断言 `内部集合 ∪ WIRE_ONLY == wire 集合`，且差集**恰好只有** `Failed`。
///
/// ⛔ **反向映射对 `Failed` 必须显式失败（`None` / `Err`），禁止静默落到 `Done`**：
/// 内部没有「失败」这个态，任何把 wire `Failed` 反解成内部 `Done` 的写法都会把
/// 「任务失败」静默上报成「任务完成」——这是最危险的一类 fail-open。
#[test]
fn failed_is_wire_only_and_has_no_internal_preimage() {
    let internal = variants_set(INTERNAL_SOURCE, "TaskStatus");
    let wire = variants_set(WIRE_SOURCE, "TaskStatus");

    // 口径：内部 `Done` 与 wire `Completed` **有意的同义改名**（见测试 ④），
    // 故「内部集合」与 wire 集合**不能原名直接比**——先按钉死的映射表把内部态译到
    // wire 词，再比。这才是「内部态在 wire 上的像 ∪ WIRE_ONLY == wire」的真实含义。
    let mapped: BTreeSet<String> = INTERNAL_TO_WIRE
        .iter()
        .map(|(_, w)| (*w).to_owned())
        .collect();
    // 映射表必须覆盖全部内部变体（否则「像」不全，本测试会以另一路径假绿）。
    let covered: BTreeSet<String> = INTERNAL_TO_WIRE
        .iter()
        .map(|(i, _)| (*i).to_owned())
        .collect();
    assert_eq!(
        covered, internal,
        "映射表没有覆盖全部内部变体（{covered:?} / {internal:?}）——先补映射再谈 fail-closed"
    );

    // ① 映射像 ∪ WIRE_ONLY == wire（全量覆盖，不多不少）。
    let union: BTreeSet<String> = mapped
        .iter()
        .cloned()
        .chain(as_set(WIRE_ONLY.iter().copied()))
        .collect();
    assert_eq!(
        union, wire,
        "`内部态的映射像 ∪ WIRE_ONLY` 不等于 wire 变体集合（{union:?} / {wire:?}）——\
         WIRE_ONLY 漏登记了某个 wire 独有词，或两侧集合已漂移"
    );

    // ② wire \ 映射像 恰好 == WIRE_ONLY（即恰好只有 Failed）。
    let wire_diff: BTreeSet<String> = wire.difference(&mapped).cloned().collect();
    let wire_only = as_set(WIRE_ONLY.iter().copied());
    assert_eq!(
        wire_diff, wire_only,
        "wire 有、内部态映射像里缺的变体不再恰好是 {wire_only:?}（实为 {wire_diff:?}）——\
         wire 新增终态却未在内部补映射锚点"
    );

    // ③ 内部变体里**没有**任何一个是「wire 独有」的反面：INTERNAL_ONLY 为空 ⇒
    //    内部每个态都译得出 wire 词（映射表覆盖 + 像 ⊆ wire 已由上面与测试 ⑤ 保证）。
    let internal_only = as_set(INTERNAL_ONLY.iter().copied());
    assert!(
        internal_only.is_empty(),
        "INTERNAL_ONLY 不再是空集（{internal_only:?}）——内部新增了无 wire 对应的态，\
         必须显式登记并补映射表"
    );

    // ④ 反向映射（**由钉死的正向表反解而来，不是新建实现**）里 `Failed` **无前像**。
    //    这就是「必须显式失败」的机器可读形态：无前像 ⇒ 只能 None/Err，绝不静默落 Done。
    let reverse: BTreeMap<&str, &str> = INTERNAL_TO_WIRE.iter().map(|(i, w)| (*w, *i)).collect();
    assert_eq!(
        reverse.get("Failed"),
        None,
        "反向映射里 wire `Failed` 竟然有前像 {:?}——内部本无「失败」态；\
         禁止把 Failed 静默落到 Done（会把失败上报成完成）",
        reverse.get("Failed")
    );
    // 对照：`Completed` 的反解**唯一**且是 `Done`——证明上面的 None 不是扫描器坏掉。
    assert_eq!(
        reverse.get("Completed"),
        Some(&"Done"),
        "wire `Completed` 的反解应唯一为内部 `Done`"
    );
}

/// ⑦ **路径存在性钉死**：两份源文件必须仍在该处，防被移动后断言静默通过。
#[test]
fn both_source_files_still_exist() {
    for (label, rel) in [
        ("ACP wire", WIRE_SOURCE_REL),
        ("内部任务机", INTERNAL_SOURCE_REL),
    ] {
        let path = source_path(rel);
        assert!(
            path.exists(),
            "{label} 源文件被移动了（期望在 {}）——include_str! 绑定的是旧路径，\
             本守卫会静默失效，须先核对路径再放行",
            path.display()
        );
    }
}

/// ⑧ **防空扫描**：扫描器坏掉时断言会「因为什么都没扫到」假绿 ⇒ 用合成样本自证。
///
/// 合成样本里放一行**注释掉的**变体与一个 `#[default]` 属性：若不排除，会被当成
/// 真变体 ⇒ 集合断言假绿。
#[test]
fn variant_scanner_excludes_comments_and_attributes() {
    let sample = "\
#[derive(Debug, Default)]
#[serde(rename_all = \"snake_case\")]
pub enum TaskStatus {
    /// 文档注释
    Pending,
    #[default]
    // InProgress,
    Done,
}
";
    assert_eq!(
        scan_enum_variants(sample, "TaskStatus"),
        vec!["Pending".to_owned(), "Done".to_owned()],
        "扫描口径变了（注释行/属性行被当成变体，或多行字段被吸进来）"
    );
    assert_eq!(
        scan_rename_all(sample, "TaskStatus").as_deref(),
        Some("snake_case"),
        "rename_all 扫描口径变了"
    );
    // 真实源文件必须扫得到东西（否则上面的集合断言全是空对空）。
    assert!(!scan_enum_variants(WIRE_SOURCE, "TaskStatus").is_empty());
    assert!(!scan_enum_variants(INTERNAL_SOURCE, "TaskStatus").is_empty());
}
