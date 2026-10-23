//! 源码扫描基元、单臂语义与扫描器自证（含漂移检测）。

use super::surface::{
    classify_source_surface, classify_wire_type, registered_surface, rename_source_surface,
    serde_rename_mismatches,
};
use super::{Classify, ClassifyArm, CONTROL_SOURCE};
use contracts_check::{ACP_CONTROL_SUBTYPES, ACP_METHODS};
use std::collections::BTreeSet;

/// **来源 A** 的一条登记：某个 `*Tag` 枚举体里的 `serde(rename)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EnumRename {
    /// 枚举名（如 `ControlRequestTag`）。
    pub(super) enum_name: String,
    /// 枚举体里 `serde(rename)` 的字面量（如 `control_request`）。
    pub(super) wire: String,
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
pub(super) fn scan_enum_renames(source: &str) -> Vec<EnumRename> {
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
pub(super) fn scan_classify_arms(source: &str) -> Vec<ClassifyArm> {
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

// ── 用例 ──

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
