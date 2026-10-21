//! 组装段序冻结（A7）：记忆四段 → 身份段 → 风格层 → 用户规则 → 产品身份 → 硬规则。
//!
//! 依据：规格 §12（组装顺序）+ §16 派卡建议 A7。段序不变量以**段首行 / 开标签**
//! 定位，避免正文里重复出现的词误判；`## 身份` 与公共头身份段同名，故记忆身份层
//! 不用它做锚点（改以 `## 用户记忆` 起比）。

mod common;

use agent_prompts::{Composer, PromptMode};
use common::{full_vars, ALL_MODES};

/// 五模式都应在场的段锚点（`## 智能体循环` 不在此列：ask/quick 按设计不挂）。
const CORE_ANCHORS: [&str; 9] = [
    "## 内容政策",
    "## 用户记忆",
    "## 项目记忆",
    "## 当日日志",
    "<identity_context>",
    "<tone_and_style>",
    "## 用户自定义规则",
    "## 产品身份",
    "## 硬规则",
];

/// 锚点在输出中的字节位置；缺席即失败（用于「五模式都应在场」的核心锚点）。
fn position(out: &str, anchor: &str, mode: PromptMode) -> usize {
    out.find(anchor)
        .unwrap_or_else(|| panic!("{mode:?} 缺锚点 {anchor}\n--- 实际 ---\n{out}"))
}

/// 在场才比较：任一锚点缺席 ⇒ 跳过该对（`## 智能体循环` 在 ask/quick 缺席）。
fn assert_before(out: &str, mode: PromptMode, earlier: &str, later: &str) {
    let (Some(earlier_at), Some(later_at)) = (out.find(earlier), out.find(later)) else {
        return;
    };
    assert!(
        earlier_at < later_at,
        "{mode:?} 段序错误：{earlier}（{earlier_at}）应在 {later}（{later_at}）之前"
    );
}

#[test]
fn assembly_order_is_frozen_in_all_modes() {
    for mode in ALL_MODES {
        let out = Composer::build(mode, &full_vars());
        // `full_vars` 覆盖全部可选段 ⇒ 核心锚点五模式都应场。
        for anchor in CORE_ANCHORS {
            position(&out, anchor, mode);
        }
        // 1. 记忆段在身份段之前（按段首行逐个比，不比较同名的 `## 身份`）。
        for memory in ["## 用户记忆", "## 项目记忆", "## 当日日志"] {
            assert_before(&out, mode, memory, "<identity_context>");
        }
        // 2. 身份段在风格层之前。
        assert_before(&out, mode, "<identity_context>", "<tone_and_style>");
        // 3. 风格层在用户自定义规则之前。
        assert_before(&out, mode, "<tone_and_style>", "## 用户自定义规则");
        // 4. 用户自定义规则在产品身份之前。
        assert_before(&out, mode, "## 用户自定义规则", "## 产品身份");
        // 5. 产品身份在硬规则之前。
        assert_before(&out, mode, "## 产品身份", "## 硬规则");
        // 6. 硬规则是最后一段：其后不存在任何 `## ` 开头的行。
        let hard_at = position(&out, "## 硬规则", mode);
        for line in out[hard_at..].lines().skip(1) {
            assert!(
                !line.starts_with("## "),
                "{mode:?} 硬规则之后仍有段标题：{line}\n--- 实际 ---\n{out}"
            );
        }
        // 7. 公共层首段在行为段之前（ask/quick 无 agent loop ⇒ 跳过）。
        assert_before(&out, mode, "## 内容政策", "## 智能体循环");
    }
}

/// 确定性冻结：同一份变量连续两次 `build` 必须逐字节相同。
#[test]
fn build_is_bytewise_deterministic_for_frozen_order() {
    for mode in ALL_MODES {
        let vars = full_vars();
        assert_eq!(
            Composer::build(mode, &vars).as_bytes(),
            Composer::build(mode, &vars).as_bytes(),
            "{mode:?} 两次 build 输出不一致"
        );
    }
}
