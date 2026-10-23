//! 登记面与两个来源面的结构比对（`serde rename` / `classify` 字面量臂）。

use super::scanner::{scan_classify_arms, scan_enum_renames};
use super::{Classify, ClassifyArm, CONTROL_SOURCE};
use contracts_check::ACP_ENVELOPE_TAGS;
use std::collections::BTreeSet;

/// 契约登记的信封标签面。
pub(super) fn registered_surface() -> BTreeSet<&'static str> {
    ACP_ENVELOPE_TAGS.iter().copied().collect()
}

/// 按扫到的臂表归类一个顶层 `type` 取值。
///
/// 这条**重演** `classify` 的 `match` 语义：命中字面量臂 ⇒ 识别；否则落兜底臂
/// ⇒ **整帧被拒**。臂表是**从源码扫出来的**（不是写死的）⇒ 源码改了，本函数跟着变。
pub(super) fn classify_wire_type(arms: &[ClassifyArm], tag: &str) -> Classify {
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

/// **来源 A 面**：`*Tag` 枚举体里 `serde(rename)` 的取值。
pub(super) fn rename_source_surface(source: &str) -> Vec<String> {
    scan_enum_renames(source)
        .into_iter()
        .filter(|r| r.enum_name.ends_with("Tag"))
        .map(|r| r.wire)
        .collect()
}

/// **来源 B 面**：`classify` 的字面量臂取值。
pub(super) fn classify_source_surface(source: &str) -> Vec<String> {
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
pub(super) fn serde_rename_mismatches(source: &str) -> Vec<String> {
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
