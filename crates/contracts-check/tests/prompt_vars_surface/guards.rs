//! 护栏自证：合成漂移样本必须让对应断言红（`catch_unwind` 捕获 panic）。

use super::registration::{
    assert_r1_fields_assigned, assert_r2_no_batch_default, assert_r3_template_refs_exist,
    assert_r4_assigned_fields_known,
};
use std::any::Any;

// ── 合成漂移样本（短：不复制整份真实源码）──

/// 合成结构体：两个字段。
const DRIFT_MODEL: &str = "pub struct PromptVars { pub alpha: String, pub beta: String }";

/// 合成接线：少赋一个字段（`beta` 没赋值）。
const DRIFT_CONTEXT_MISSING: &str =
    "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: String::new() } }";

/// 合成接线：用 `..Default::default()` 批量兜底。
const DRIFT_CONTEXT_DEFAULT: &str =
    "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: String::new(), ..Default::default() } }";

/// 合成接线：赋了一个 `PromptVars` 里没有的字段。
const DRIFT_CONTEXT_UNKNOWN: &str = "fn prompt_vars(&self) -> PromptVars { PromptVars { alpha: \
     String::new(), beta: String::new(), gamma: String::new() } }";

/// 合成模板：引用了一个不存在的变量。
const DRIFT_TEMPLATE: &str = "text(\"{{no_such_variable}}\")";

/// 合成模板：探针形态（占位符 + 调用形态各一个）。
pub(super) const DRIFT_TEMPLATE_PROBE: &str = "text(\"{{probe_text}}\")\nflag(\"probe_flag\")";

/// 跑一个闭包并把 panic 捕获下来（静默 panic hook，避免污染测试输出）。
pub(super) fn catch_panic<F: FnOnce()>(f: F) -> std::thread::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous);
    outcome
}

/// 取出 panic payload 里的文本。
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// 断言「护栏确实红了」：必须捕获到 panic，且 panic 文案带上标签。
pub(super) fn assert_panic_labeled(outcome: std::thread::Result<()>, needle: &str) {
    let payload = outcome.expect_err("护栏必须能红：漂移样本竟未触发 panic");
    let message = panic_message(payload.as_ref());
    assert!(
        message.contains(needle),
        "panic 文案应包含 `{needle}`，实际：{message}"
    );
}

// ── 用例 ──

/// ⑤ 护栏自证：合成漂移样本（删掉一个字段赋值）⇒ R1 **必须**红。
#[test]
fn guard_detects_missing_field_assignment() {
    let outcome = catch_panic(|| assert_r1_fields_assigned(DRIFT_MODEL, DRIFT_CONTEXT_MISSING));
    assert_panic_labeled(outcome, "未在 prompt_vars() 里显式赋值");
}

/// ⑥ 护栏自证：合成漂移样本（模板引用不存在的变量）⇒ R3 **必须**红。
#[test]
fn guard_detects_unknown_template_variable() {
    let outcome =
        catch_panic(|| assert_r3_template_refs_exist(DRIFT_MODEL, &[("drift.rs", DRIFT_TEMPLATE)]));
    assert_panic_labeled(outcome, "PromptVars 里没有的变量");
}

/// ⑦ 护栏自证：合成漂移样本（批量兜底）⇒ R2 **必须**红。
#[test]
fn guard_detects_batch_default_fallback() {
    let outcome = catch_panic(|| assert_r2_no_batch_default(DRIFT_CONTEXT_DEFAULT));
    assert_panic_labeled(outcome, "批量兜底");
}

/// ⑧ 护栏自证：合成漂移样本（赋了不存在的字段）⇒ R4 **必须**红。
#[test]
fn guard_detects_unknown_assigned_field() {
    let outcome =
        catch_panic(|| assert_r4_assigned_fields_known(DRIFT_MODEL, DRIFT_CONTEXT_UNKNOWN));
    assert_panic_labeled(outcome, "不存在的字段");
}
