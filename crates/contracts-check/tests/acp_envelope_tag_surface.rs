//! ACP **信封级判别键面**（顶层 `type` 字段）的结构化登记断言（W2 · lane proto）。
//!
//! # 这条断言治什么病
//!
//! ACP 的 control 面在 JSON-RPC 三分类**之外**还有一层信封，顶层靠 `type` 字段
//! 判别（`control_request` / `control_notification` / `control_response`）。
//! 这三个字面量有**双来源派生**——**两处独立手写**，缺机器断言：
//!
//! - **来源 A（权威）**：`crates/agent-acp/src/control.rs` 里三个
//!   `serde(rename = "…")`（`ControlRequestTag` / `ControlNotificationTag` /
//!   `ControlResponseTag`）；
//! - **来源 B（消费方）**：同文件 `TopLevel::classify` 的 `match` 分支字面量。
//!
//! # 为什么它是**承重**的
//!
//! `classify` 是 `agent-acp` 编解码的**唯一入口**（`codec.rs` 的 `decode_frame`
//! 调它）。任一来源的字面量漂移 ⇒ 该信封**不被识别** ⇒ 落兜底臂 ⇒
//! 抛 `MalformedMessage` ⇒ **整帧被拒**（**硬失败**，不是静默降级）；且 serde
//! 侧用同一拼写去反序列化 ⇒ **编解码两端会一起错**（对端可能认得、我方不认）。
//!
//! # 为什么是**源码扫描**而不是运行时调用
//!
//! `agent-acp` 不是本 crate 的依赖（`Cargo.toml` 属共享区、本卡不得改），而
//! `agent-gateway` 虽依赖它却**没有 `pub use` 重新导出** `TopLevel` ⇒ 运行时
//! **够不着**。这**不是降级**：本卡的核心断言就是「两处独立手写的字面量是否
//! 一致」，**源码扫描正是对症手段**（运行时调用反而看不到「手写 vs 派生」这个
//! 双来源问题）。
//!
//! # 口径
//!
//! **只守关系，不锁数量**：实现正当新增一个信封类型时，把新标签登记进
//! `contracts_check::ACP_ENVELOPE_TAGS` 即可；任何「恰好 3 个」的断言都会在正当
//! 扩充时必红（教训：`tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。
//!
//! 台账：`docs/contracts/acp-host.md` §12。

use contracts_check::{ACP_CONTROL_SUBTYPES, ACP_ENVELOPE_TAGS, ACP_METHODS};
use std::collections::BTreeSet;

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
///
/// `agent-acp` 不是本 crate 的依赖（dev-dependencies 里没有它，`Cargo.toml` 属
/// 共享区、本卡不得改），故按 `tests/acp_control_surface.rs` 的先例用
/// `include_str!` 绑定源文件**文本**，在测试里做源码扫描。
const CONTROL_SOURCE: &str = include_str!("../../agent-acp/src/control.rs");

/// **来源 A** 的一条登记：某个 `*Tag` 枚举体里的 `serde(rename)`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct EnumRename {
    /// 枚举名（如 `ControlRequestTag`）。
    enum_name: String,
    /// 枚举体里 `serde(rename)` 的字面量（如 `control_request`）。
    wire: String,
}

/// **来源 B** 的一个 `match` 臂（从 `classify` 扫出）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifyArm {
    /// 字面量臂的 wire 值；兜底臂为 `None`。
    wire: Option<String>,
    /// 字面量臂映射到的变体名（如 `ControlRequest`）；兜底臂为 `None`。
    variant: Option<String>,
    /// 臂的正文（多行拼接）；兜底臂里含错误类型与拒绝理由。
    body: String,
}

/// 一个顶层 `type` 取值在 `classify` 里的归类结果。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Classify {
    /// 命中某个字面量臂。
    Recognized {
        /// 命中的变体名。
        variant: String,
    },
    /// 落兜底臂 ⇒ **整帧被拒**。
    Rejected {
        /// 兜底臂正文（含错误类型与拒绝理由）。
        reason: String,
    },
    /// **没有兜底臂** ⇒ 未登记的 `type` 不再被拒（契约破了）。
    Unbounded,
}

/// 契约登记的信封标签面。
fn registered_surface() -> BTreeSet<&'static str> {
    ACP_ENVELOPE_TAGS.iter().copied().collect()
}

/// 一行的缩进宽度（按字符数计）。
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

/// 扫出源文件里**枚举体**内的 `#[serde(rename = "…")]`。
///
/// 口径：**只扫枚举体**——枚举体里的 `rename` 是**变体**的 wire 值，而**结构体
/// 字段**上的 `#[serde(rename = "type")]` 是**字段键名**（`kind` 字段的键），
/// 两者语义不同，混进来会把 `"type"` 当成信封标签。
fn scan_enum_renames(source: &str) -> Vec<EnumRename> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let Some(enum_name) = enum_name_of(line) else {
            continue;
        };
        let base_indent = indent_of(line);
        for body in lines.iter().skip(idx + 1) {
            let trimmed = body.trim();
            if trimmed == "}" && indent_of(body) <= base_indent {
                break; // 枚举收尾
            }
            let Some(wire) = serde_rename_literal(trimmed) else {
                continue;
            };
            out.push(EnumRename {
                enum_name: enum_name.clone(),
                wire: wire.to_owned(),
            });
        }
    }
    out
}

/// 从 `pub enum <名字> {` 那一行取出枚举名。
fn enum_name_of(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("pub enum ")?;
    let name = rest.split_whitespace().next()?.trim_end_matches('{').trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_owned())
}

/// 取一行 `#[serde(rename = "…")]` 里的字面量。
///
/// ⚠️ 只认**精确**的 `#[serde(rename = "` 前缀 ⇒ `#[serde(rename_all = "…")]`
/// 与 `#[serde(other)]` **不会**被误吸。
fn serde_rename_literal(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("#[serde(rename = \"")?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// 扫出 `TopLevel::classify` 的 `match` 臂（字面量臂 + 兜底臂）。
fn scan_classify_arms(source: &str) -> Vec<ClassifyArm> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains("pub fn classify"))
        .expect("control.rs 里找不到 `pub fn classify`——源文件形态变了");

    let base_indent = indent_of(lines[start]);
    let mut arms: Vec<ClassifyArm> = Vec::new();
    let mut idx = start + 1;
    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();
        if trimmed == "}" && indent_of(line) <= base_indent {
            break; // 函数体收尾
        }
        if let Some((wire, _pattern)) = parse_arm(trimmed) {
            // 收该臂的多行正文（字面量臂的正文跨 3 行）。
            let arm_indent = indent_of(line);
            let mut body = String::from(trimmed);
            let mut j = idx + 1;
            while j < lines.len() && indent_of(lines[j]) > arm_indent {
                body.push('\n');
                body.push_str(lines[j].trim());
                j += 1;
            }
            let variant = wire.as_deref().and_then(|_| arm_variant(&body));
            arms.push(ClassifyArm {
                wire,
                variant,
                body,
            });
            idx = j;
            continue;
        }
        idx += 1;
    }
    arms
}

/// 从**单行**里解析一个 `match` 臂的形状。
///
/// 返回 `(wire 值, 臂的匹配模式)`：字面量臂（`"control_request" => …`）的 wire
/// 是 `Some(字面量)`；兜底臂（`other => …`）的 wire 是 `None`。
///
/// 只认**单个 token**的匹配模式 ⇒ `format!(…)` 之类含 `=>` 的行不会被误吸。
fn parse_arm(trimmed: &str) -> Option<(Option<String>, String)> {
    let (pattern, _) = trimmed.split_once("=>")?;
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.contains(char::is_whitespace) {
        return None;
    }
    let wire = match pattern.strip_prefix('"') {
        Some(rest) => Some(rest.strip_suffix('"')?.to_owned()),
        None => None,
    };
    Some((wire, pattern.to_owned()))
}

/// 从臂正文里取出 `Self::<变体>` 的变体名。
fn arm_variant(body: &str) -> Option<String> {
    let idx = body.find("Self::")?;
    let rest = &body[idx + "Self::".len()..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// 按扫到的臂表归类一个顶层 `type` 取值。
///
/// 这条**重演** `classify` 的 `match` 语义：命中字面量臂 ⇒ 识别；否则落兜底臂
/// ⇒ **整帧被拒**。臂表是**从源码扫出来的**（不是写死的）⇒ 源码改了，本函数跟着变。
fn classify_wire_type(arms: &[ClassifyArm], tag: &str) -> Classify {
    if let Some(arm) = arms.iter().find(|a| a.wire.as_deref() == Some(tag)) {
        return Classify::Recognized {
            variant: arm.variant.clone().unwrap_or_default(),
        };
    }
    match arms.iter().find(|a| a.wire.is_none()) {
        Some(fallback) => Classify::Rejected {
            reason: fallback.body.clone(),
        },
        None => Classify::Unbounded,
    }
}

/// serde `snake_case` 派生（**逐字复刻** serde 的实现：每个**大写**字符前插
/// `_`，再转小写）。
fn serde_snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in name.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

/// **来源 A 面**：`*Tag` 枚举体里 `serde(rename)` 的取值。
fn rename_source_surface(source: &str) -> Vec<String> {
    scan_enum_renames(source)
        .into_iter()
        .filter(|r| r.enum_name.ends_with("Tag"))
        .map(|r| r.wire)
        .collect()
}

/// **来源 B 面**：`classify` 的字面量臂取值。
fn classify_source_surface(source: &str) -> Vec<String> {
    scan_classify_arms(source)
        .into_iter()
        .filter_map(|a| a.wire)
        .collect()
}

/// 集合差集（`left` 有、`right` 没有的）。
fn diff_set(left: &BTreeSet<String>, right: &BTreeSet<String>) -> Vec<String> {
    left.difference(right).cloned().collect()
}

/// 比对登记面与某个来源面，返回**不一致**的说明清单（空 = 一致）。
///
/// 数量也守：两侧**个数**必须相等（重复取值会让集合差集少跑一圈 ⇒ 差集断言
/// 假绿）。数量用**两侧自身长度**比，**不**写死「3」。
fn surface_mismatches(
    registered: &BTreeSet<String>,
    implemented: &[String],
    label: &str,
) -> Vec<String> {
    let unique: BTreeSet<&str> = implemented.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    if unique.len() != implemented.len() {
        out.push(format!("{label} 侧扫出重复取值：{implemented:?}"));
    }
    if registered.len() != ACP_ENVELOPE_TAGS.len() {
        out.push(format!(
            "登记面内部有重复项（去重前 {} 项、去重后 {} 项）",
            ACP_ENVELOPE_TAGS.len(),
            registered.len()
        ));
    }
    if implemented.len() != registered.len() {
        out.push(format!(
            "{label} 侧取值数量 {} 与登记面数量 {} 不一致（实现 {implemented:?}）",
            implemented.len(),
            registered.len()
        ));
    }
    let implemented_set: BTreeSet<String> = implemented.iter().cloned().collect();
    let missing = diff_set(registered, &implemented_set);
    let extra = diff_set(&implemented_set, registered);
    if !missing.is_empty() {
        out.push(format!("{label} 侧：登记有实现无 {missing:?}"));
    }
    if !extra.is_empty() {
        out.push(format!("{label} 侧：实现有登记无 {extra:?}"));
    }
    out
}

/// **来源 A 的不一致清单**（serde `rename` vs 登记面）。
fn serde_rename_mismatches(source: &str) -> Vec<String> {
    surface_mismatches(
        &registered_set(),
        &rename_source_surface(source),
        "serde rename",
    )
}

/// **来源 B 的不一致清单**（`classify` 字面量臂 vs 登记面）。
fn classify_arm_mismatches(source: &str) -> Vec<String> {
    surface_mismatches(
        &registered_set(),
        &classify_source_surface(source),
        "classify 字面量臂",
    )
}

/// 登记面（`String` 化，便于与扫出来的集合直接比）。
fn registered_set() -> BTreeSet<String> {
    registered_surface().iter().map(|s| s.to_string()).collect()
}

/// **实现值 == 登记值**（来源 A：serde `rename`）。
///
/// 扫 `control.rs` 源码文本，取 `*Tag` 枚举体里 `serde(rename = "…")` 的字面量，
/// 与 [`ACP_ENVELOPE_TAGS`] **逐值**相等（集合相等 + 数量相等）。
///
/// 数量用**登记常量自身的长度**做自洽检查，**不**写死「3」——实现正当新增一个
/// 信封类型时，把新标签登记进常量即可，本断言随之变绿。
#[test]
fn serde_renames_equal_the_registered_surface() {
    let mismatched = serde_rename_mismatches(CONTROL_SOURCE);
    assert!(
        mismatched.is_empty(),
        "来源 A（serde rename）与登记面不一致：{mismatched:?}——\
         serde `rename` 是**唯一权威来源**，实现与登记必须逐值相等"
    );
}

/// **消费方拼写 == 登记值**（来源 B：`classify` 的 `match` 字面量）。
///
/// `match` 分支的**顺序不承重** ⇒ 只比**集合**，不比顺序。
#[test]
fn classify_arms_equal_the_registered_surface() {
    let mismatched = classify_arm_mismatches(CONTROL_SOURCE);
    assert!(
        mismatched.is_empty(),
        "来源 B（classify 字面量臂）与登记面不一致：{mismatched:?}——\
         classify 是编解码的唯一入口，字面量漂移 ⇒ 整帧被拒"
    );
}

/// **双来源自证**：来源 A 与来源 B 是**两处独立手写**，必须各自被扫到、且相等。
///
/// 这条是本卡的核心价值：若两侧拼写漂移（一边改了、另一边没改），两边**各自**
/// 与登记面比对仍可能同时绿（登记面也跟着改了），只有**两侧互相比对**才抓得到。
#[test]
fn the_two_handwritten_sources_are_scanned_independently_and_agree() {
    let renames = rename_source_surface(CONTROL_SOURCE);
    let arms = classify_source_surface(CONTROL_SOURCE);

    assert!(
        !renames.is_empty(),
        "来源 A（serde rename）一个也没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    assert!(
        !arms.is_empty(),
        "来源 B（classify 字面量臂）一个也没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );

    let rename_set: BTreeSet<String> = renames.into_iter().collect();
    let arm_set: BTreeSet<String> = arms.into_iter().collect();
    assert_eq!(
        rename_set, arm_set,
        "两处独立手写的来源不一致：serde rename 侧 {rename_set:?} vs classify 侧 {arm_set:?}——\
         漂移的代价是**整帧被拒**（硬失败），不是静默降级"
    );
}

/// **登记面 ⊆ 实际能被 `classify` 识别的集合**。
///
/// 每个登记的标签都必须命中 `classify` 的一个字面量臂 ⇒ 否则该信封落兜底臂
/// ⇒ 整帧被拒。
#[test]
fn every_registered_tag_is_recognized_by_classify() {
    let arms = scan_classify_arms(CONTROL_SOURCE);
    for tag in ACP_ENVELOPE_TAGS {
        let verdict = classify_wire_type(&arms, tag);
        assert!(
            matches!(verdict, Classify::Recognized { .. }),
            "登记的信封标签 {tag} 在 classify 的 match 里没有对应的字面量臂（实得 {verdict:?}）——\
             该信封会被落进兜底臂 ⇒ 整帧被拒（MalformedMessage），不是静默降级"
        );
    }
}

/// **反面：一个未登记的 `type` 必被拒**。
///
/// `agent-acp` 不可从本 crate 触达（见模块文档）⇒ 走**源码级判定**：
/// ① `classify` 的 `match` 存在兜底臂；② 兜底臂返回 `MalformedMessage`；
/// ③ 拒绝理由含「未知顶层 type」；④ 一个确实未登记的 `type` 落进兜底臂。
#[test]
fn an_unregistered_type_is_rejected() {
    let arms = scan_classify_arms(CONTROL_SOURCE);
    let Some(fallback) = arms.iter().find(|a| a.wire.is_none()) else {
        panic!("classify 的 match 里没有兜底臂——未登记的 type 将不再被拒（契约破了）")
    };
    assert!(
        fallback.body.contains("MalformedMessage"),
        "兜底臂不再返回 MalformedMessage——未登记的 type 不再是**硬失败**：兜底臂正文：{}",
        fallback.body
    );
    assert!(
        fallback.body.contains("未知顶层 type"),
        "兜底臂的错误信息不再含「未知顶层 type」——拒绝语义变了：兜底臂正文：{}",
        fallback.body
    );

    let verdict = classify_wire_type(&arms, "__not_registered__");
    match verdict {
        Classify::Rejected { reason } => assert!(
            reason.contains("未知顶层 type"),
            "未登记的 type 被拒的理由里没有「未知顶层 type」：{reason}"
        ),
        other => panic!("未登记的 type 没有被拒（实得 {other:?}）——classify 的兜底语义坏了"),
    }
}

/// **每个字面量臂指向的变体，其名必须与 wire 值一致**。
///
/// 臂的 wire 值与它映射到的 Rust 变体是**同一个信封类型**的两种写法 ⇒ 变体名
/// 的 `snake_case` 派生必须等于 wire 值。否则「`control_request` 的帧被当成
/// control_notification 解析」这类**接线错位**就抓不到了。
#[test]
fn each_literal_arm_maps_to_the_variant_named_by_its_wire_value() {
    let arms = scan_classify_arms(CONTROL_SOURCE);
    let literal_arms: Vec<&ClassifyArm> = arms.iter().filter(|a| a.wire.is_some()).collect();
    assert!(
        !literal_arms.is_empty(),
        "一个字面量臂都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    for arm in literal_arms {
        let wire = arm.wire.as_deref().unwrap_or_default();
        let Some(variant) = arm.variant.as_deref() else {
            panic!("字面量臂 {wire} 的正文里找不到 `Self::…` 变体——臂的形态变了")
        };
        assert_eq!(
            serde_snake_case(variant),
            wire,
            "字面量臂 {wire} 映射到的变体是 {variant}（派生名 {} ≠ wire 值）——\
             信封标签与 Rust 类型接线错位",
            serde_snake_case(variant)
        );
    }
}

/// **信封面与载荷内判别键面正交**：登记的信封标签**不得**出现在 method 面
/// （顶层 `method` 字段）或 control subtype 面（载荷内的 `subtype` 字段）。
///
/// 三个面分属不同层（信封面 = 顶层 `type`；method 面 = 顶层 `method`；
/// subtype 面 = 载荷内 `subtype`）⇒ 混进来会让使用者以为「这是某个方法名」。
#[test]
fn the_envelope_face_is_orthogonal_to_the_payload_faces() {
    let registered = registered_surface();
    let offenders: Vec<&str> = ACP_METHODS
        .iter()
        .copied()
        .chain(ACP_CONTROL_SUBTYPES.iter().copied())
        .filter(|name| registered.contains(name))
        .collect();
    assert!(
        offenders.is_empty(),
        "信封面里出现了载荷内的判别键 {offenders:?}——信封面只登记顶层 `type` 的取值，\
         method 面与 subtype 面分属另外两层"
    );
}

/// **防空扫描**：扫描器本身坏了（口径改坏 / 源文件形态变了）时，断言会
/// 「因为什么都没扫到」而假绿 ⇒ 单独守一条「扫得到东西」。
#[test]
fn scanner_actually_finds_the_envelope_tag_surface() {
    let renames = scan_enum_renames(CONTROL_SOURCE);
    assert!(
        !renames.is_empty(),
        "一个 serde rename 都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    assert!(
        !scan_classify_arms(CONTROL_SOURCE).is_empty(),
        "一个 classify 臂都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
}

/// **扫描口径的自证**（合成样本）：结构体字段上的 `rename` 与容器上的
/// `rename_all` **都不得**被吸进来源 A 面。
///
/// ⛔ 不写成「真实源码里只有 `*Tag` 枚举体有 rename」——那是把**实现形态**钉死，
/// 实现正当新增别的枚举体时会假红（教训同款）。改成**合成样本**证明扫描口径本身
/// 的三条规则：
///
/// ① 结构体字段上的 `#[serde(rename = "type")]`（`kind` 字段的**键名**）**不扫**；
/// ② 容器上的 `#[serde(rename_all = "snake_case")]`（**派生规则**）**不扫**；
/// ③ 非 `*Tag` 枚举体里的**变体级** `rename` **会被扫到**，但被来源 A 面的
///    `*Tag` 过滤**挡在外面**（它不是信封标签）。
#[test]
fn scanner_excludes_struct_field_renames_and_container_rename_all() {
    let sample = "\
pub struct ControlRequest {
    #[serde(rename = \"type\")]
    pub kind: ControlRequestTag,
}
#[serde(rename_all = \"snake_case\")]
pub enum ControlSubtype {
    ElicitationCreate,
}
pub enum ControlOther {
    #[serde(rename = \"other_tag_value\")]
    Other,
}
pub enum ControlRequestTag {
    #[serde(rename = \"control_request\")]
    ControlRequest,
}
";
    let scanned = scan_enum_renames(sample);
    let values: Vec<&str> = scanned.iter().map(|r| r.wire.as_str()).collect();
    assert!(
        !values.contains(&"type"),
        "结构体字段上的 `rename` 被扫进来了（{values:?}）——它是**字段键名**，不是信封标签"
    );
    assert!(
        !values.contains(&"snake_case"),
        "容器上的 `rename_all` 被扫进来了（{values:?}）——它是**派生规则**，不是 wire 值"
    );
    assert!(
        values.contains(&"other_tag_value"),
        "枚举体里的变体级 rename 没被扫到（{values:?}）——扫描口径坏了"
    );
    // 来源 A 面只收 `*Tag` 枚举体 ⇒ 上面那个非 `*Tag` 枚举体的取值被挡在外面。
    let source_a = rename_source_surface(sample);
    assert_eq!(
        source_a,
        vec!["control_request".to_owned()],
        "来源 A 面的 `*Tag` 过滤坏了——只该收 `ControlRequestTag` 的取值"
    );
}

/// **扫描器**真的能咬人：把源码里的 serde `rename` 改错一处，**比对逻辑**必须
/// 报出差异（否则上面那些断言只是**永远绿**的摆设）。
///
/// 用**源码文本的内存副本**做变异（不改文件）⇒ 证明「断言存在」与「断言有效」
/// 是两回事。比对走的是**断言本身用的那个函数**（不是另写一份）。
///
/// 变异目标**从源码里现取**（不写死某个拼写）⇒ 实现换标签名时本断言仍然有效。
#[test]
fn scanner_detects_a_serde_rename_drift() {
    let original = rename_source_surface(CONTROL_SOURCE)
        .first()
        .cloned()
        .expect("来源 A 一个都没扫到——扫描口径坏了，本断言失去意义");
    let drifted_spelling = format!("{original}_drifted");
    let needle = format!("#[serde(rename = \"{original}\")]");
    let drifted = CONTROL_SOURCE.replace(
        &needle,
        &format!("#[serde(rename = \"{drifted_spelling}\")]"),
    );
    assert_ne!(
        drifted, CONTROL_SOURCE,
        "源码里找不到 {needle} 这一处——变异样本没造出来，本断言失去意义（源文件形态变了）"
    );

    let mismatched = serde_rename_mismatches(&drifted);
    assert!(
        !mismatched.is_empty(),
        "serde rename 漂移（`{original}` → `{drifted_spelling}`）必须被报出来——\
         实报：{mismatched:?}"
    );
    let reported = mismatched.join(" ");
    assert!(
        reported.contains(&drifted_spelling) && reported.contains(&original),
        "报出的差异必须同时指明漂移后的拼写与原拼写：{mismatched:?}"
    );
}

/// **双来源**真的能咬人：把 `classify` 的字面量臂改错一处，**两侧互比**必须
/// 报出差异（来源 A 侧没改、来源 B 侧改了 ⇒ 两边不再相等）。
///
/// 变异目标**从源码里现取**（不写死某个拼写）⇒ 实现换标签名时本断言仍然有效。
#[test]
fn scanner_detects_a_classify_arm_drift_against_the_serde_source() {
    let original = classify_source_surface(CONTROL_SOURCE)
        .first()
        .cloned()
        .expect("来源 B 一个都没扫到——扫描口径坏了，本断言失去意义");
    let drifted_spelling = format!("{original}_drifted");
    let needle = format!("\"{original}\" => serde_json::from_value(v)");
    let drifted = CONTROL_SOURCE.replace(
        &needle,
        &format!("\"{drifted_spelling}\" => serde_json::from_value(v)"),
    );
    assert_ne!(
        drifted, CONTROL_SOURCE,
        "源码里找不到 `{needle}` 这一处——变异样本没造出来，本断言失去意义（源文件形态变了）"
    );

    let rename_set: BTreeSet<String> = rename_source_surface(&drifted).into_iter().collect();
    let arm_set: BTreeSet<String> = classify_source_surface(&drifted).into_iter().collect();
    assert_ne!(
        rename_set, arm_set,
        "classify 字面量臂漂移必须让两侧不再相等——实扫：serde 侧 {rename_set:?} vs classify 侧 {arm_set:?}"
    );
    assert!(
        arm_set.contains(&drifted_spelling) && !arm_set.contains(&original),
        "漂移后的 classify 侧取值不对——实扫：{arm_set:?}"
    );
}
