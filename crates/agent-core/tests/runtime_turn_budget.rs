//! 单回合步数上限可配置（W1-core turn-budget 卡）：Runtime 面契约测试。
//!
//! 金标 opencode v2 `/home/ubuntu/ref/opencode`：
//! - `packages/schema/src/agent.ts:33` `steps: PositiveInt.pipe(optional)`；
//! - `packages/schema/src/schema.ts:3` `PositiveInt = Schema.Int.check(isGreaterThan(0))`；
//! - `packages/core/src/session/runner/llm.ts:226`
//!   `stepLimitReached = loaded.agent.info.steps !== undefined && step >= loaded.agent.info.steps`。
//!
//! 本卡把 `Runtime::run_turn` 里写死的 `MAX_TURN_STEPS` 换成 `RuntimeConfig::max_turn_steps`：
//! 默认值 = 既有 `loop::DEFAULT_MAX_STEPS`（默认行为不变），可经 `config:set` 覆盖，
//! 非法值（0 / 负数 / 非整数）整体拒绝且不改动现值。判定口径与既有实现一致
//! （工具步结算后判定，见 `docs/max-steps-surrender-status.md` 偏离 3）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_core::config::{RuntimeConfig, DEFAULT_MAX_TURN_STEPS};
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, StepOptions, ToolChoice, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, TurnReceipt};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode, Role};
use serde_json::{json, Value};

/// 默认上限的契约镜像：`src/runtime/turns.rs` 的 `DEFAULT_MAX_TURN_STEPS`。
const DEFAULT_STEPS: usize = 30;

// ---------------- 脚本化 provider ----------------

/// 记录 (上下文, 选项) 的脚本化 provider：正常步走 `next_steps_with_context`，
/// 缴械步走 `next_steps_with_options`，两侧调用都入账供断言。
struct BudgetProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<Option<StepOptions>>>,
}

impl BudgetProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }

    /// 第 n 次调用（0 起）的选项；`None` 表示走旧入口（正常步）。
    fn options_at(&self, n: usize) -> Option<StepOptions> {
        self.seen.lock().expect("锁")[n]
    }

    fn serve(&self, options: Option<StepOptions>) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("锁").push(options);
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported("脚本已耗尽：出现计划外的调用"))
    }
}

impl Provider for BudgetProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("本卡只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        _context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.serve(None)
    }

    fn next_steps_with_options(
        &self,
        _context: &TurnContext,
        options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.serve(Some(*options))
    }
}

// ---------------- harness ----------------

/// 一步以工具调用终结（无文本、无 Finished）。
fn tool_step(name: &str, args: Value) -> Vec<ProviderStep> {
    vec![ProviderStep::ToolCall {
        name: name.to_string(),
        args,
    }]
}

/// 无工具、单步收敛的脚本（text + done）。
fn text_step(text: &str) -> Vec<ProviderStep> {
    vec![ProviderStep::Text(text.to_string()), ProviderStep::Finished]
}

/// 起一个真实 Runtime（可指定配置）。
fn runtime_with(provider: Arc<dyn Provider>, config: RuntimeConfig) -> Runtime {
    let dir = std::env::temp_dir().join(format!("turn-budget-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(provider)
        .config(config)
        .build()
        .expect("runtime")
}

/// Ask 模式会话 + Full 权限（与 `max_steps_surrender` 卡同款：让工具步得以结算）。
fn ask_session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "步数上限".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Ask,
        permission_mode: PermissionMode::Full,
        env_binding: None,
        isolation: Isolation {
            user_id: "u".into(),
            tenant_id: "t".into(),
            env_id: None,
        },
    })
    .expect("create session")
}

/// receipt 里唯一的 `done` 事件载荷。
fn done_payload(receipt: &TurnReceipt) -> Value {
    let dones: Vec<&Event> = receipt.events.iter().filter(|e| e.kind == "done").collect();
    assert_eq!(dones.len(), 1, "回合必须恰一个 done 收口");
    dones[0].payload.clone()
}

/// n 个只读工具步 + 末步（缴械步）的完整脚本。
fn script_of(n: usize, final_step: Vec<ProviderStep>) -> Vec<Vec<ProviderStep>> {
    let mut script: Vec<Vec<ProviderStep>> = (0..n)
        .map(|i| tool_step("fs.read", json!({ "path": format!("a{i}.txt") })))
        .collect();
    script.push(final_step);
    script
}

// ---------------- 契约 ----------------

/// 用例 1：默认配置 ⇒ 到限步数与既有行为一致：`DEFAULT_MAX_TURN_STEPS` 个工具步
/// 之后才是缴械步（第 31 次调用），`done{max_steps_reached}` 的 `steps` 含缴械步、
/// `maxSteps` = 默认常量（默认行为完全不变）。
#[test]
fn default_config_keeps_the_existing_step_ceiling() {
    let p = BudgetProvider::new(script_of(DEFAULT_STEPS, text_step("到限总结")));
    let rt = runtime_with(p.clone(), RuntimeConfig::default_v0());
    let s = ask_session(&rt);

    let receipt = rt.send_text(s.id, "读全部文件").expect("turn");
    assert_eq!(
        p.calls(),
        DEFAULT_STEPS + 1,
        "默认 {DEFAULT_STEPS} 个工具步 + 1 缴械步（默认行为不变）"
    );
    assert_eq!(
        p.options_at(DEFAULT_STEPS),
        Some(StepOptions {
            tool_choice: ToolChoice::None
        }),
        "第 {} 次调用即缴械步（金标 llm.ts:244）",
        DEFAULT_STEPS + 1
    );
    assert_eq!(
        RuntimeConfig::default_v0().max_turn_steps,
        DEFAULT_STEPS,
        "默认值来自既有常量 loop::DEFAULT_MAX_STEPS"
    );
    assert_eq!(DEFAULT_MAX_TURN_STEPS, DEFAULT_STEPS);

    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], DEFAULT_STEPS + 1, "steps 含缴械步");
    assert_eq!(done["maxSteps"], DEFAULT_STEPS, "maxSteps 报默认常量");
}

/// 用例 2：`maxTurnSteps = 2` ⇒ 第 2 个工具步之后即进入末步缴械：provider 恰 3 次调用，
/// 第 3 次是 `toolChoice=none`，`done.maxSteps` 报出实际生效值 2。
#[test]
fn configured_max_turn_steps_bounds_the_turn_and_is_reported() {
    let p = BudgetProvider::new(script_of(2, text_step("到限总结")));
    let rt = runtime_with(p.clone(), RuntimeConfig::default_v0());
    let s = ask_session(&rt);
    rt.config_set(&json!({ "maxTurnSteps": 2 })).expect("set");
    assert_eq!(rt.config_get().max_turn_steps, 2, "配置值生效");

    let receipt = rt.send_text(s.id, "读两个文件").expect("turn");
    assert_eq!(p.calls(), 3, "2 个工具步 + 1 缴械步");
    assert_eq!(
        p.options_at(2),
        Some(StepOptions {
            tool_choice: ToolChoice::None
        }),
        "第 3 次调用（0 起下标 2）即缴械步"
    );

    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["steps"], 3, "steps 含缴械步");
    assert_eq!(done["maxSteps"], 2, "maxSteps 报实际生效值");
}

/// 用例 2b：上限内的回合不受影响——配置 5，但 1 个工具步后自然收敛 ⇒ 走
/// `done{status:"completed"}`，不发缴械步（上限只约束不收敛的回合）。
#[test]
fn a_turn_below_the_limit_converges_normally() {
    let p = BudgetProvider::new(vec![
        tool_step("fs.read", json!({ "path": "a.txt" })),
        text_step("读完了"),
    ]);
    let rt = runtime_with(p.clone(), RuntimeConfig::default_v0());
    let s = ask_session(&rt);
    rt.config_set(&json!({ "maxTurnSteps": 5 })).expect("set");

    let receipt = rt.send_text(s.id, "读一个文件").expect("turn");
    assert_eq!(p.calls(), 2, "1 个工具步 + 1 收敛步");
    let done = done_payload(&receipt);
    assert_eq!(done["status"], "completed");
    assert_eq!(done["steps"], 2);
}

/// 用例 3：`apply_patch({"maxTurnSteps": 0})` ⇒ `InvalidConfig` 且现值不变。
#[test]
fn zero_max_turn_steps_is_rejected_without_mutating_the_current_value() {
    let rt = runtime_with(BudgetProvider::new(Vec::new()), RuntimeConfig::default_v0());
    rt.config_set(&json!({ "maxTurnSteps": 7 })).expect("set");
    let before = rt.config_get();

    let err = rt
        .config_set(&json!({ "maxTurnSteps": 0 }))
        .expect_err("0 不是正整数，必须拒绝");
    assert!(matches!(err, CoreError::InvalidConfig(_)));
    assert_eq!(rt.config_get(), before, "失败的 set 不得改动现值");
}

/// 用例 4：负数 / 字符串 / 浮点 / 布尔 / null / 数组 / 对象 ⇒ 均 `InvalidConfig`
/// 且现值不变（金标 `PositiveInt = Schema.Int.check(isGreaterThan(0))`：
/// 非整数与 ≤0 一律拒，`2.0` 这类「整值浮点」也拒——不是 `Schema.Int`）。
#[test]
fn each_invalid_shape_is_rejected_and_current_value_is_untouched() {
    let rt = runtime_with(BudgetProvider::new(Vec::new()), RuntimeConfig::default_v0());
    rt.config_set(&json!({ "maxTurnSteps": 9 })).expect("set");
    let before = rt.config_get();

    for bad in [
        json!(-1),
        json!(-100),
        json!("2"),
        json!(2.5),
        json!(2.0),
        json!(true),
        json!(null),
        json!([2]),
        json!({ "n": 2 }),
    ] {
        let err = rt
            .config_set(&json!({ "maxTurnSteps": bad }))
            .expect_err("非法值必须拒绝");
        assert!(
            matches!(err, CoreError::InvalidConfig(_)),
            "{bad} 应报 InvalidConfig"
        );
        assert_eq!(rt.config_get(), before, "{bad} 被拒后现值必须不变");
    }
}

/// 用例 4c：合法正整数被接受（含 1：恰一次正常步后即缴械）。
#[test]
fn positive_integer_values_are_accepted() {
    let rt = runtime_with(BudgetProvider::new(Vec::new()), RuntimeConfig::default_v0());
    for good in [1usize, 2, 30, 100] {
        let patched = rt
            .config_set(&json!({ "maxTurnSteps": good }))
            .expect("正整数必须接受");
        assert_eq!(patched.max_turn_steps, good);
    }
    let p = BudgetProvider::new(script_of(1, text_step("到限总结")));
    let rt = runtime_with(p.clone(), {
        let mut c = RuntimeConfig::default_v0();
        c.max_turn_steps = 1;
        c
    });
    let s = ask_session(&rt);
    let done = done_payload(&rt.send_text(s.id, "读一个文件").expect("turn"));
    assert_eq!(p.calls(), 2, "1 个工具步 + 1 缴械步");
    assert_eq!(done["maxSteps"], 1);
}

/// 用例 5：未知字段仍报错（回归保护：新分支没有放宽既有 `deny_unknown_fields` 规则）。
#[test]
fn unknown_configured_fields_are_still_rejected() {
    let rt = runtime_with(BudgetProvider::new(Vec::new()), RuntimeConfig::default_v0());
    let before = rt.config_get();
    for unknown in ["noSuchField", "maxturnsteps", "max_turn_steps", "maxSteps"] {
        let err = rt
            .config_set(&json!({ unknown: 3 }))
            .expect_err("未知字段必须拒绝，不静默忽略");
        assert!(
            matches!(err, CoreError::InvalidConfig(_)),
            "{unknown} 应报 InvalidConfig"
        );
    }
    // 合法新字段与未知字段混在同一 patch 里 ⇒ 整体失败、现值不变。
    let err = rt
        .config_set(&json!({ "maxTurnSteps": 5, "noSuchField": 1 }))
        .expect_err("混合 patch 里含未知字段 ⇒ 整体拒绝");
    assert!(matches!(err, CoreError::InvalidConfig(_)));
    assert_eq!(rt.config_get(), before, "失败的 set 不得改动现值");
}

/// 用例 6：序列化含新字段，且能被反序列化回同值（round-trip）；serde 名为 `maxTurnSteps`。
#[test]
fn config_serializes_and_deserializes_max_turn_steps() {
    let mut config = RuntimeConfig::default_v0();
    config.max_turn_steps = 42;

    // camelCase 序列化名（`config:get` 的 data）。
    let value = config.to_value();
    assert_eq!(value["maxTurnSteps"], 42);
    assert!(
        value.get("max_turn_steps").is_none(),
        "字段名冻结为 camelCase"
    );

    // round-trip：反序列化回同值。
    let back: RuntimeConfig = serde_json::from_value(value.clone()).expect("反序列化");
    assert_eq!(back, config);
    assert_eq!(back.max_turn_steps, 42);

    // 直序列化路径（不经 `to_value`）同样用 camelCase。
    let raw = serde_json::to_value(&config).expect("序列化");
    assert_eq!(raw["maxTurnSteps"], 42);

    // 默认值也走同一路径。
    let default_value = RuntimeConfig::default_v0().to_value();
    assert_eq!(default_value["maxTurnSteps"], DEFAULT_STEPS);

    // 缺字段载荷（老客户端持久化形态）反序列化 ⇒ 回到默认（默认行为不变）。
    let mut bare = value.clone();
    bare.as_object_mut().expect("对象").remove("maxTurnSteps");
    let decoded: RuntimeConfig = serde_json::from_value(bare).expect("缺字段可反序列化");
    assert_eq!(
        decoded.max_turn_steps,
        RuntimeConfig::default_v0().max_turn_steps
    );
}

/// 用例 7：Runtime 侧确实用配置值（不是常量）：同一脚本下上限 2 与上限 3 的
/// provider 调用次数不同，`maxSteps` 随配置变化。
#[test]
fn runtime_uses_the_configured_value_not_the_constant() {
    for limit in [2usize, 3, 4] {
        let p = BudgetProvider::new(script_of(limit, text_step("到限总结")));
        let rt = runtime_with(p.clone(), RuntimeConfig::default_v0());
        let s = ask_session(&rt);
        rt.config_set(&json!({ "maxTurnSteps": limit }))
            .expect("set");
        let done = done_payload(&rt.send_text(s.id, "读文件").expect("turn"));
        assert_eq!(
            p.calls(),
            limit + 1,
            "上限 {limit}：{limit} 步工具 + 1 缴械步"
        );
        assert_eq!(done["maxSteps"], limit);
        assert_eq!(done["steps"], limit + 1);
    }
}

/// 用例 8：缴械步后会话历史不落缴械提示（既有语义在新入口下不变），且工具调用不执行。
#[test]
fn configured_ceiling_keeps_the_surrender_semantics() {
    use agent_core::r#loop::MAX_STEPS_PROMPT;
    let p = BudgetProvider::new(script_of(
        1,
        tool_step("fs.write", json!({ "path": "z.txt" })),
    ));
    let rt = runtime_with(p.clone(), RuntimeConfig::default_v0());
    let s = ask_session(&rt);
    rt.config_set(&json!({ "maxTurnSteps": 1 })).expect("set");

    let receipt = rt.send_text(s.id, "读一个文件然后写").expect("turn");
    assert_eq!(p.calls(), 2);
    let messages = rt.get_session(s.id).expect("session").messages;
    assert!(
        messages.iter().all(|m| m.content != MAX_STEPS_PROMPT),
        "缴械提示不得落进会话历史"
    );
    let calls = messages.iter().filter(|m| m.role == Role::ToolCall).count();
    assert_eq!(calls, 1, "缴械步的工具请求不执行、不落 ToolCall");
    let done = done_payload(&receipt);
    assert_eq!(done["status"], "max_steps_reached");
    assert_eq!(done["maxSteps"], 1);
}
