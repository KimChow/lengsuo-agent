//! ACP **control 面**（`V.*` subtype）wire 拼写面的结构化登记断言（W2 · lane proto）。
//!
//! # 这条断言治什么病
//!
//! ACP 的 **control 面**——`{type:"control_request", request_id, request:{subtype:…}}`
//! 这层信封里的 `subtype`——此前**没有任何契约登记**：实现侧
//! `crates/agent-acp/src/control.rs` 的 `enum ControlSubtype`（12 个变体）的
//! **wire 拼写**只由 `as_str()` 里**一个手写 `match`** 决定；契约侧
//! `crates/contracts-check/src/lib.rs` **零** control 常量，`docs/contracts/` 下也
//! 没有任何文件列出这 11 个名字。
//!
//! ⇒ 「这 11 个名字叫什么」**只由一个人手写在一个 `match` 里**，**没有任何机器
//! 断言**守着它；而 serde 侧另有**一条独立的派生链**
//! （`#[serde(rename_all = "snake_case")]` ⇒ 变体名 ⇒ wire 名），两条来源今天
//! 一致纯属**手滑没发生**，不是**被断言守着的**。
//!
//! # 为什么它是**承重**语义：它就是 handler 的**路由键**
//!
//! 注册侧 `crates/agent-acp/src/host_send.rs` 的 `on_control` 用
//! `subtype.as_str()` 作 **map 的键**插入 handler 表；分派侧
//! `crates/agent-acp/src/host_router.rs` 的入站分派用**同一个** `as_str()`
//! **查表**。⇒ **拼写写错 ⇒ 查表不命中 ⇒ 请求掉进事件队列** ⇒ **不是报错，而是
//! 静默降级**：请求**永久悬挂**，除非上层自己 `respond_control` 兜底。
//!
//! # `Unknown` 为什么**不登记**
//!
//! `ControlSubtype::Unknown` 是 `#[serde(other)]` 兜底变体，承载的是「原文」而
//! 非某个固定拼写；`as_str()` 给它的 `"unknown"` 是**占位串**（仅供日志）。且
//! 注册侧**明确拒绝**注册它（`on_control` 里
//! `if subtype == ControlSubtype::Unknown { return; }`——「注册它等于死代码」）
//! ⇒ 它**不属于**「可注册 handler 的 wire 名面」。
//!
//! # 做法与口径
//!
//! 照抄 `tests/acp_notification_surface.rs` 的骨架：**手写字符扫描、不新增依赖、
//! 不用正则库**；用 `include_str!` **编译期绑定** `agent-acp/src/control.rs`
//! （`agent-acp` 不是本 crate 的依赖，`Cargo.toml` 属共享区、本卡不得改 ⇒ 只能
//! 扫文本、不能 `use agent_acp::…`）。
//!
//! **只守关系，不锁数量/取值**：实现正当新增一个可注册 subtype 时，把新名字登记
//! 进 `contracts_check::ACP_CONTROL_SUBTYPES` 即可，本组断言自然变绿；任何
//! 「恰好 11 个 control subtype」的断言都会在正当扩充时必红（教训：
//! `tests/method_ledger.rs` 的 `assert_eq!(ACP_METHODS.len(), 13)`）。
//!
//! ⛔ **不断言「两套拼写风格的面，名字集合不相交」**：`session/update`（method，
//! 斜杠）与 `session_update`（control，下划线）**归一化后同名**，但那是
//! **同层不同义**、不是 bug。本文件守的是**拼写风格互不相交**（control 面**不
//! 含** `/`；method 面的分隔符**不是** `_`），不是「名字集合不相交」。
//!
//! 台账：`docs/contracts/acp-method-ledger.md` §8。

use contracts_check::{ACP_CONTROL_SUBTYPES, ACP_METHODS};
use std::collections::{BTreeMap, BTreeSet};

/// 被扫描的源文件（**编译期**绑定：源文件改动即触发重编）。
///
/// `agent-acp` 不是本 crate 的依赖（dev-dependencies 里没有它，`Cargo.toml` 属
/// 共享区、本卡不得改），故按 `tests/acp_notification_surface.rs` 的先例用
/// `include_str!` 绑定源文件**文本**，在测试里做源码扫描。
const CONTROL_SOURCE: &str = include_str!("../../agent-acp/src/control.rs");

/// 注册侧（handler 表**插入**）。
const HOST_SEND_SOURCE: &str = include_str!("../../agent-acp/src/host_send.rs");

/// 分派侧（handler 表**查表**）。
const HOST_ROUTER_SOURCE: &str = include_str!("../../agent-acp/src/host_router.rs");

/// `ControlSubtype` 的一个变体。
#[derive(Debug, Clone, PartialEq, Eq)]
struct SubtypeVariant {
    /// 变体名（如 `ElicitationCreate`）。
    name: String,
    /// 是不是 `#[serde(other)]` 兜底变体（`Unknown`）——它承载「原文」。
    is_fallback: bool,
}

/// 扫出 `pub enum ControlSubtype { … }` 的变体。
///
/// 口径：定位 `pub enum ControlSubtype {` 那一行，**逐行**吃到该枚举的收尾
/// `}`；跳过空行、`//`（含 `///` 文档注释）与 `#[…]` 属性行；其余形如
/// `名字,` 的行即变体声明。带 `#[serde(other)]` 的那个变体标记为 `is_fallback`。
fn scan_control_subtype_variants(source: &str) -> Vec<SubtypeVariant> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub enum ControlSubtype {"))
        .expect("control.rs 里找不到 `pub enum ControlSubtype {`——源文件形态变了");

    let mut out = Vec::new();
    let mut pending_fallback = false;
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim();
        if trimmed == "}" {
            break; // 枚举收尾
        }
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue; // 空行 / 文档注释
        }
        if trimmed.starts_with("#[") {
            pending_fallback |= trimmed.contains("serde(other)");
            continue; // 属性行本身不是变体
        }
        let name = trimmed.trim_end_matches(',').trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue; // 不是变体声明（如多行字段）
        }
        out.push(SubtypeVariant {
            name: name.to_owned(),
            is_fallback: pending_fallback,
        });
        pending_fallback = false;
    }
    out
}

/// 扫出 `ControlSubtype` 的**容器属性** `#[serde(rename_all = "…")]` 的取值。
///
/// 这条派生规则是「serde 派生名」这半边的**前提**：规则一变，派生名就变，
/// 而本文件按 `snake_case` 推导 ⇒ 断言会红 ⇒ 提示同步。
fn scan_rename_all(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let enum_at = lines
        .iter()
        .position(|line| line.trim_start().starts_with("pub enum ControlSubtype {"))?;
    // 属性行在枚举声明的上方（serde 允许夹在 `#[derive]` 与 `pub enum` 之间）。
    lines[..enum_at].iter().rev().take(3).find_map(|line| {
        let rest = line.trim().strip_prefix("#[serde(rename_all = ")?;
        let value = rest.strip_prefix('"')?;
        let end = value.find('"')?;
        Some(value[..end].to_owned())
    })
}

/// 扫出 `as_str()` 里的 match 臂（`Self::<变体> => "<字面量>",`）。
///
/// 只在 `pub fn as_str` 的**函数体**内扫 ⇒ 不会把文件里其它 `Self::…` 用法
/// （`TopLevel::classify` 里的 `.map(Self::ControlRequest)`）误吸进来。
fn scan_as_str_arms(source: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains("pub fn as_str"))
        .expect("control.rs 里找不到 `pub fn as_str`——源文件形态变了");

    let mut out = Vec::new();
    let mut depth = 0i32;
    for line in lines.iter().skip(start) {
        depth += brace_delta(line);
        if depth <= 0 {
            break; // 函数体收尾
        }
        if let Some(arm) = parse_match_arm(line) {
            out.push(arm);
        }
    }
    out
}

/// 从**单行**里解析 `Self::<变体> => "<字面量>"` 这一形态的 match 臂。
fn parse_match_arm(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("Self::")?;
    let (variant, rhs) = rest.split_once("=>")?;
    let variant = variant.trim();
    if variant.is_empty()
        || !variant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let rhs = rhs.trim().trim_end_matches(',').trim();
    let literal = rhs.strip_prefix('"')?.strip_suffix('"')?;
    Some((variant.to_owned(), literal.to_owned()))
}

/// 一行里的花括号净增减量。
fn brace_delta(line: &str) -> i32 {
    line.matches('{').count() as i32 - line.matches('}').count() as i32
}

/// serde `rename_all = "snake_case"` 的派生规则（**逐字复刻** serde 的实现：
/// 每个**大写**字符前插 `_`，再转小写）。
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

/// 逐个具名变体比对「serde 派生名」与「手写拼写」，返回**不一致**的清单。
///
/// 放在函数里（而不是写死在测试里）是为了让「这条比对**真的能咬人**」可以被
/// **合成样本**证明（见 `scanner_detects_a_handwritten_spelling_drift`）——否则
/// 这条断言在实现无漂移时永远是绿的，绿得**没有证据**。
fn serde_derived_mismatches(source: &str) -> Vec<String> {
    let arms: BTreeMap<String, String> = scan_as_str_arms(source).into_iter().collect();
    let mut mismatched = Vec::new();
    for variant in scan_control_subtype_variants(source) {
        if variant.is_fallback {
            continue; // `#[serde(other)]` 兜底变体不参与派生名比对（见 §8「Unknown 不可注册」）
        }
        let derived = serde_snake_case(&variant.name);
        let Some(handwritten) = arms.get(&variant.name) else {
            mismatched.push(format!("{}（as_str() 里没有这个臂）", variant.name));
            continue;
        };
        if *handwritten != derived {
            mismatched.push(format!(
                "{}：serde 派生 {derived} ≠ 手写 {handwritten}",
                variant.name
            ));
        }
    }
    mismatched
}

/// **实现面**：具名变体的 wire 拼写（`变体名 → 字面量`）。
///
/// `#[serde(other)]` 兜底变体（`Unknown`）**不在**此面——它的 `"unknown"` 是
/// 占位串，不是可注册的 wire 名。
fn implementation_surface() -> BTreeMap<String, String> {
    let fallback: BTreeSet<String> = scan_control_subtype_variants(CONTROL_SOURCE)
        .into_iter()
        .filter(|v| v.is_fallback)
        .map(|v| v.name)
        .collect();
    scan_as_str_arms(CONTROL_SOURCE)
        .into_iter()
        .filter(|(variant, _)| !fallback.contains(variant))
        .collect()
}

/// **登记面**：契约登记的 control subtype wire 名。
fn registered_surface() -> BTreeSet<&'static str> {
    ACP_CONTROL_SUBTYPES.iter().copied().collect()
}

/// 一个名字的**首个分隔符**（`/` 或 `_`）；没有分隔符返回 `None`。
fn first_separator_style(name: &str) -> Option<char> {
    name.chars().find(|c| *c == '/' || *c == '_')
}

/// 取 `marker` 所在行**之后**、缩进**不深于**该行的下一个 `}` 之间的块体。
///
/// 用于 `on_control`（签名行不带 `{`）与路由分派臂（`TopLevel::ControlRequest`
/// 那支）：按**缩进**配对比按花括号深度更稳（块体里的小闭包/大括号不会截断）。
fn block_body_by_indent(source: &str, marker: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("源文件里找不到 {marker}"));
    let base_indent = indent_of(lines[start]);
    let mut body = String::new();
    for line in lines.iter().skip(start) {
        if line.trim() == "}" && indent_of(line) <= base_indent {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// 一行的缩进宽度（按字符数计）。
fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

/// **实现面 ⊆ 登记面**：实现里能路由的 subtype，契约必须登记。
///
/// 红了说明：实现新增了一个可注册 subtype，但 `ACP_CONTROL_SUBTYPES` 没登记
/// ——先补登记（并同步 `docs/contracts/acp-method-ledger.md` §8），再合并。
#[test]
fn implementation_surface_is_subset_of_the_registered_surface() {
    let registered = registered_surface();
    let unregistered: Vec<String> = implementation_surface()
        .into_iter()
        .filter(|(_, wire)| !registered.contains(wire.as_str()))
        .map(|(variant, wire)| format!("{variant}（{wire}）"))
        .collect();
    assert!(
        unregistered.is_empty(),
        "实现里存在但契约未登记的 control subtype（差集）{unregistered:?}——\
         wire 拼写就是 handler 路由键，漏登记等于契约落后于实现"
    );
}

/// **登记面 ⊆ 实现面**：契约登记的 subtype，实现里必须仍能路由到它。
///
/// 红了说明：登记面腐化了（实现改了拼写 / 删了变体）⇒ 把它撤下登记，或把
/// 实现改回来。
#[test]
fn registered_surface_is_subset_of_the_implementation_surface() {
    let implemented = implementation_surface();
    let missing: Vec<String> = registered_surface()
        .iter()
        .copied()
        .filter(|wire| !implemented.values().any(|w| w == wire))
        .map(|wire| wire.to_owned())
        .collect();
    assert!(
        missing.is_empty(),
        "契约登记但实现里扫不到的 control subtype（差集）{missing:?}——\
         登记面腐化了，撤下登记或把实现改回来；实扫到的实现面：{implemented:?}"
    );
}

/// **serde 派生名 == 手写拼写**：两条**独立来源**必须钉在一起。
///
/// - serde 侧：`#[serde(rename_all = "snake_case")]` ⇒ 变体名 ⇒ wire 名（**自动**）；
/// - 手工侧：`as_str()` 的 `match` 字面量（**手写**）。
///
/// 两条来源今天一致，但**没有任何断言**守它 ⇒ 手滑写错时，下游表现为**静默
/// 降级**（查表不命中 ⇒ 请求进事件队列 ⇒ 永久悬挂），不是报错。
#[test]
fn serde_derived_names_match_the_handwritten_wire_spellings() {
    let rename_all = scan_rename_all(CONTROL_SOURCE);
    assert_eq!(
        rename_all.as_deref(),
        Some("snake_case"),
        "ControlSubtype 的 serde 派生规则不再是 `snake_case`（实为 {rename_all:?}）——\
         「serde 派生名」这半边的前提变了：本断言的推导函数与台账 \
         docs/contracts/acp-method-ledger.md §8 都必须同步更新"
    );

    let mismatched = serde_derived_mismatches(CONTROL_SOURCE);
    assert!(
        mismatched.is_empty(),
        "serde 派生名与手写 wire 拼写不一致（差集）{mismatched:?}——\
         两条来源是独立的，拼写漂移的代价是**静默降级**（路由键查不中 ⇒ 请求悬挂）"
    );
}

/// **逐项锚点仍在**：每个登记名都必须仍能在 `as_str()` 的 `match` 里**逐字**
/// 找到（`Self::X => "字面量",` 形态），且**变体名**仍存在于枚举里。
///
/// 两条都守，是为防「只改了一边」：改了枚举没改拼写（`as_str()` 与 serde 派生名
/// 脱节 ⇒ 上面那条断言红），或改了拼写没改登记面（本条红）。
#[test]
fn each_registered_name_keeps_its_verbatim_match_arm_anchor() {
    let arms = scan_as_str_arms(CONTROL_SOURCE);
    let variants = scan_control_subtype_variants(CONTROL_SOURCE);
    for wire in ACP_CONTROL_SUBTYPES {
        let Some((variant, _)) = arms.iter().find(|(_, literal)| literal == wire) else {
            panic!(
                "登记的 control subtype {wire} 在 as_str() 的 match 里找不到对应臂——\
                 登记面腐化了，撤下登记或把实现改回来"
            );
        };
        let needle = format!("Self::{variant} => \"{wire}\",");
        assert!(
            CONTROL_SOURCE.contains(&needle),
            "control subtype {wire} 的 match 臂不再逐字存在（期望 `{needle}`）——\
             锚点被挪位或改写，撤下登记或把实现改回来"
        );
        assert!(
            variants.iter().any(|v| v.name == *variant),
            "control subtype {wire} 的变体 {variant} 不在 enum ControlSubtype 里了——\
             改了枚举没改登记，撤下登记或把实现改回来"
        );
    }
}

/// **`Unknown` 被排除**：它**不得**进登记面（注册它等于死代码）。
///
/// 三件事一起守：① 登记面里**没有** `"unknown"`；② 实现面里也**没有**它
/// （`Unknown` 不属于「可注册 handler 的 wire 名面」）；③ 实现里 `Unknown` 分支
/// **仍**返回占位串 `"unknown"`。
#[test]
fn the_unknown_variant_is_excluded_from_the_registered_surface() {
    assert!(
        !registered_surface().contains("unknown"),
        "登记面里出现了 \"unknown\"——`Unknown` 是 `#[serde(other)]` 兜底变体，\
         其 wire 拼写是**占位串**（无法还原原文），且注册侧明确拒绝注册它\
         （`on_control` 里 `if subtype == ControlSubtype::Unknown {{ return; }}`）——\
         登记它会误导使用者去注册一个**永不命中**的路由键"
    );
    assert!(
        !implementation_surface()
            .values()
            .any(|wire| wire == "unknown"),
        "实现面里出现了 \"unknown\"——`Unknown` 不属于可注册的 wire 名面"
    );

    let arms = scan_as_str_arms(CONTROL_SOURCE);
    let unknown_arm = arms
        .iter()
        .find(|(variant, _)| variant == "Unknown")
        .map(|(_, literal)| literal.as_str());
    assert_eq!(
        unknown_arm,
        Some("unknown"),
        "实现里 `Self::Unknown` 分支不再返回占位串 \"unknown\"（实为 {unknown_arm:?}）——\
         占位串的语义变了（它只是日志用、**不可还原**原文），台账 \
         docs/contracts/acp-method-ledger.md §8 须同步"
    );
}

/// **wire 拼写就是路由键**：注册侧用 `as_str()` **插入**、分派侧用 `as_str()`
/// **查表**——两侧必须是**同一个** `as_str()`。
///
/// 这条守的是台账 §8「拼错的代价是静默降级」那句话的**前提**：若哪天分派侧
/// 改用别的键（如变体名），那「拼错 ⇒ 查不中 ⇒ 请求悬挂」的推理就不再成立，
/// 台账必须改，而不是让两处实现悄悄分叉。
#[test]
fn the_wire_spelling_is_the_handler_routing_key() {
    let register = block_body_by_indent(HOST_SEND_SOURCE, "pub fn on_control(");
    assert!(
        register.contains(".as_str()"),
        "注册侧 on_control 不再用 as_str() 作路由键——台账 \
         docs/contracts/acp-method-ledger.md §8「wire 拼写就是 handler 路由键」\
         的前提变了，须同步改台账（实扫到的函数体：{register}）"
    );
    assert!(
        register.contains(".insert("),
        "注册侧 on_control 不再往 handler 表里插入——路由键的注册路径变了，须同步改台账"
    );

    let dispatch = block_body_by_indent(HOST_ROUTER_SOURCE, "TopLevel::ControlRequest(req) =>");
    assert!(
        dispatch.contains(".as_str()"),
        "分派侧不再用 as_str() 查 handler 表——路由键的分派路径变了，须同步改台账\
         （实扫到的分派块：{dispatch}）"
    );
    assert!(
        dispatch.contains(".get("),
        "分派侧不再查 handler 表——路由键的分派路径变了，须同步改台账"
    );
}

/// **control 面与其它两套拼写风格不相交**：登记面每一项都**不含** `/`（method
/// 面的分隔符）与 `:`（v0 动作面的分隔符）。
///
/// ⛔ **只守拼写风格**，⛔ **不**守「两面的名字集合不相交」：`session_update`
/// （control）与 `session/update`（method）**归一化后同名**，但那是**同层不同义**
/// ——断言集合不相交会立刻红，且红得毫无道理。
#[test]
fn control_names_never_use_the_other_faces_separators() {
    let offenders: Vec<&str> = ACP_CONTROL_SUBTYPES
        .iter()
        .copied()
        .filter(|name| name.contains('/') || name.contains(':'))
        .collect();
    assert!(
        offenders.is_empty(),
        "登记面里的 {offenders:?} 含 `/` 或 `:`——control 面是**下划线**拼写\
         （method 面用斜杠、v0 动作面用冒号），串风格会让人误以为是另一套命名空间里的名字"
    );
}

/// **method 面与 control 面的拼写风格不相交**：`ACP_METHODS` 里每一项若有分隔符，
/// 分隔符**必须**是 `/`（**不得**是 `_`）。
///
/// 「若**有**分隔符」而非「必须含 `/`」：`ACP_METHODS` 里有 `initialize` 这类
/// **单词**方法名（无分隔符），把它算成违规是**误判**。
#[test]
fn method_names_never_use_the_control_face_separator_style() {
    let offenders: Vec<&str> = ACP_METHODS
        .iter()
        .copied()
        .filter(|name| first_separator_style(name) == Some('_'))
        .collect();
    assert!(
        offenders.is_empty(),
        "ACP_METHODS 里的 {offenders:?} 用**下划线**作分隔符——method 面是**斜杠**\
         拼写（control 面才是下划线），两套拼写风格不得互串（`session/update` 与\
         `session_update` 归一化后同名，是**同层不同义**，不是同一个名字）"
    );
}

/// **登记面内部无重复**：同一名字登记两次会让差集断言静默少跑一圈。
#[test]
fn registered_surface_has_no_duplicates() {
    let unique = registered_surface();
    assert_eq!(
        unique.len(),
        ACP_CONTROL_SUBTYPES.len(),
        "ACP_CONTROL_SUBTYPES 内部有重复项：{:?}",
        ACP_CONTROL_SUBTYPES
    );
}

/// **防空扫描**：扫描器本身坏了（口径改坏 / 源文件形态变了）时，断言会
/// 「因为什么都没扫到」而假绿 ⇒ 单独守一条「扫得到东西」。
#[test]
fn scanner_actually_finds_the_control_subtype_surface() {
    let variants = scan_control_subtype_variants(CONTROL_SOURCE);
    assert!(
        !variants.is_empty(),
        "一个 `ControlSubtype` 变体都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    assert!(
        variants.iter().any(|v| v.is_fallback),
        "连 `#[serde(other)]` 兜底变体都没扫到——扫描口径坏了，断言会假绿；\
         扫到的变体：{variants:?}"
    );
    let arms = scan_as_str_arms(CONTROL_SOURCE);
    assert!(
        !arms.is_empty(),
        "一个 `as_str()` 的 match 臂都没扫到——扫描口径或源文件形态坏了，断言会假绿"
    );
    assert!(
        !implementation_surface().is_empty(),
        "实现面扫空了——扫描口径坏了，断言会假绿"
    );
}

/// 扫描器**排除注释行 / 非臂行**这条规则的自我证明。
///
/// 合成样本里故意放一行**注释掉的**完整臂——若不排除，它会被当成实现面 ⇒
/// 登记面与实现面会**假绿**（多出一个不存在的 subtype）。
#[test]
fn scanner_excludes_comment_lines_and_non_arm_lines() {
    let sample = "\
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ElicitationCreate => \"elicitation_create\",
            // Self::TerminalKill => \"terminal_kill\",
            Self::Unknown => \"unknown\",
        }
    }
";
    let arms = scan_as_str_arms(sample);
    assert_eq!(
        arms,
        vec![
            (
                "ElicitationCreate".to_owned(),
                "elicitation_create".to_owned()
            ),
            ("Unknown".to_owned(), "unknown".to_owned()),
        ],
        "扫描口径变了（注释行被当成臂 / 非臂行被吸进来）"
    );
}

/// 「serde 派生名 == 手写拼写」这条比对**真的能咬人**的自我证明（合成样本）。
///
/// ⛔ 真实实现今天**没有**漂移 ⇒ 那条断言在实现无漂移时永远是绿的。绿的
/// **没有证据** ⇒ 用合成样本造一处**漂移**（手写拼写少了一个下划线），证明
/// 比对逻辑**能**报出差异——否则「断言存在」与「断言有效」是两回事。
#[test]
fn scanner_detects_a_handwritten_spelling_drift() {
    let sample = "\
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = \"snake_case\")]
pub enum ControlSubtype {
    ElicitationCreate,
    SessionUpdate,
    #[serde(other)]
    Unknown,
}
impl ControlSubtype {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ElicitationCreate => \"elicitation_create\",
            Self::SessionUpdate => \"sessionupdate\",
            Self::Unknown => \"unknown\",
        }
    }
}
";
    let mismatched = serde_derived_mismatches(sample);
    assert_eq!(
        mismatched.len(),
        1,
        "手写拼写漂移（`sessionupdate`）必须被报出来——比对逻辑没生效的话，\
         「serde 派生名 == 手写拼写」那条断言就只是**永远绿**的摆设；实报：{mismatched:?}"
    );
    assert!(
        mismatched[0].contains("SessionUpdate") && mismatched[0].contains("session_update"),
        "报出的差异必须指明是哪个变体、派生名与手写名分别是什么：{mismatched:?}"
    );
    // serde 派生规则本身的自我证明（逐字复刻 serde 的实现）。
    assert_eq!(serde_snake_case("ElicitationCreate"), "elicitation_create");
    assert_eq!(serde_snake_case("FsReadTextFile"), "fs_read_text_file");
    assert_eq!(
        serde_snake_case("TerminalWaitForExit"),
        "terminal_wait_for_exit"
    );
    // 「首个分隔符」口径的自我证明：method 面是斜杠、control 面是下划线、
    // 单词方法名没有分隔符。
    assert_eq!(first_separator_style("session/update"), Some('/'));
    assert_eq!(first_separator_style("session_update"), Some('_'));
    assert_eq!(first_separator_style("initialize"), None);
}
