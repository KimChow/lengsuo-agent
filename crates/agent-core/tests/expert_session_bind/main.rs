//! 专家召唤与切换（专家体系 · 卡 6，`h4-expert-session-bind`）集成测试。
//!
//! 规格：`harness-expert.md` §10.2（五步联动）+ §11.2（I1–I4 四条不变量）。
//! 每个测试名以不变量编号开头，便于逐条对账（见 `docs/expert-session-bind-status.md`）。
//!
//! 按职责拆为四组（仓内惯例同 `tests/agent_loop_modes/`）：
//! - `support.rs`：专家包夹具、捕获型 provider、建会话 / 走回合；
//! - `summon.rs`：召唤（设 `expert_id` ⇒ Expert + 事件）；
//! - `switch.rs`：切换 / 清空（Expert 保持、回落 Craft）、I1 历史、人格段重写；
//! - `failures.rs`：I3 召唤失败不写字段、I4 会话间隔离、组装期降级。
//!
//! **HOME 隔离**：注册表同时扫用户级 `$HOME/.lengsuo/experts`。本目标的所有用例在
//! 同一进程内跑（Rust 会并行起线程），故任用例建 Runtime 前都先经 [`isolate_home`]
//! （`Once`，只生效一次）把 `HOME` 指向临时目录——否则开发机装着的专家包（含损坏包）
//! 会干扰「专家不存在」类断言。

mod failures;
mod summon;
mod support;
mod switch;

use std::sync::Once;

/// 把用户级专家根隔离到临时 home（全进程一次）。
pub fn isolate_home() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let home = std::env::temp_dir().join(format!("expert-bind-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("隔离 home");
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
    });
}
