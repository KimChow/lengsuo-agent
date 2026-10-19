//! Agent loop：面向 [`ChatProvider`] 宿主的**回合适配层**——外推步、内管白名单/门禁，
//! 结构对齐参考件 `session/runner/llm.ts`（外层只推进步）+ `runner/step.ts`（一步原语）。
//!
//! 步语义（与 [`Runtime::step`](crate::runtime::Runtime::step) 同一词汇）：
//! 一步 = 一次 provider 调用；`run()` 外层只推进步，每步经 [`run`] 内的 `History`
//! **重新派生**本次请求（工具结算后开新步重读），[`run::StepFlow`] 四走向只报
//! 「本步发生了什么」，回合收口归外层。
//!
//! **与 Runtime 的收敛状态（本卡）**：本层提供两条驱动路径（见 [`Driver`]）——
//! [`AgentLoop::new`] 的本地自驱，与 [`AgentLoop::with_runtime`] 的 `Runtime` 驱动。
//! 后者把「一步」的历史与工具执行都交给 `Runtime`（历史落在**会话**上、每步重读；
//! 工具由 `Runtime` 结算），本层只做回合政策与门禁。仍留在本层的缺口（B2 门禁拒绝
//! 文案、B5 事件载荷缺 id）与建议的最小扩法见 `docs/loop-adapter-status.md`。
//!
//! 回合骨架（对应卡面 8 步）：

//! 1. 理解：Quick/Ask 之外才带工具面；系统消息 = 模式模板 + 全模式硬规则；
//! 2. chat：调 [`ChatProvider::chat`] 取回文本与工具请求（v0 同步整段文本，流式由网关侧切分）；
//! 3. 选工具：按 [`crate::mode::mode_policy`] 白名单过滤，白名单外**不执行**，直接回填 `denied` 结果；
//! 4. 权限门：调用方（网关/宿主）已做，本层不重复判定；
//! 5. 执行：[`ToolExecutor::execute`] → 结果回填消息；
//! 6. 观察：结果进入消息历史后继续下一回合；
//! 7. 完成：某回合**无工具请求**即完成；Craft/Expert 若从未调用任何写/执行工具而任务文本含
//!    创建/写入/修改意图，追加一次提醒回合（整个 run 最多一次）；
//! 8. 最终回复：`LoopOutcome.final_text` + `done` 事件。
//!
//! 门禁与熔断：
//! - `Plan` 模式进入即 `PlanGate { active: true }`；除 `plan.exit` 外写/执行类工具一律拒绝（回填不中断）；
//!   `plan.exit(approved = true)` 切到 Craft 继续，`approved = false` ⇒ [`LoopStatus::Aborted`]；
//! - `plan.enter` 用于非规划模式（Expert）显式进入规划态；
//! - 同一工具在被拒绝后再次请求 ⇒ [`LoopStatus::Denied`]（防对同一被拒工具空转）；
//! - 回合数达 `LoopLimits::max_steps` 仍未收敛 ⇒ 末步缴械：以 `toolChoice=none`
//!   追加一次纯文本总结步（`MAX_STEPS_PROMPT` 进请求、工具定义保留以维持
//!   provider 缓存前缀），随后以 [`LoopStatus::MaxStepsReached`] 收口（金标
//!   opencode `max-steps.ts` + `llm.ts` 的 `stepLimitReached` 分支）。
//!
//! 本卡不接真实工具与真实模型：trait 由宿主实现，`EchoProvider` 之外另有测试用 Mock。
//!
//! 按职责拆分：`types`（公开词汇与 trait 契约）、`run`（外层步循环 + 一步原语 + 历史组装）、
//! `guard`（白名单与规划门禁处理）、`support`（共用小工具）、`runtime_step`（两条驱动
//! 路径的适配面：历史装载、消息落地、provider 调用与工具结算分发）。

mod guard;
mod run;
mod runtime_step;
mod support;
mod types;

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::mode::ModePolicy;
use crate::runtime::Runtime;
use crate::schema::{Event, Mode};

pub use types::{
    ChatProvider, ChatTurn, LoopLimits, LoopOutcome, LoopStatus, PlanGate, ToolCall, ToolExecutor,
    DEFAULT_MAX_STEPS, MAX_STEPS_PROMPT,
};

/// 一步的执行面：要么交给已建好的 [`Runtime`]，要么由本层自驱。
///
/// 这是缺口 B1 的解法：`Runtime` 的注入面是 `Arc<dyn Provider>` /
/// `Arc<dyn ToolExecutor + Send + Sync>`（均隐含 `'static`），把 `AgentLoop::new`
/// 的 `&'a dyn` 借用塞进 `Arc` 会破坏既有调用方（测试即契约）。故不改 `new` 的
/// 签名，改用 [`AgentLoop::with_runtime`] 由宿主把**已建好的** `Runtime` 接入；
/// 两条路径的差异只在于「谁持有历史、谁执行工具」。
pub(super) enum Driver<'a> {
    /// 本地自驱（[`AgentLoop::new`] 路径）：本层持有历史、本层执行工具。
    Local {
        provider: &'a dyn ChatProvider,
        executor: &'a dyn ToolExecutor,
    },
    /// Runtime 驱动（[`AgentLoop::with_runtime`] 路径）：`Runtime` 持有历史并执行
    /// 工具（`step_executor` 是宿主经
    /// [`crate::runtime::RuntimeBuilder::tool_executor`] 注入给 Runtime 的那一个，
    /// 本层只借它判「当前哪些工具可见」，不自己执行）。
    Runtime {
        runtime: Runtime,
        session: Uuid,
        step_executor: Option<&'a dyn ToolExecutor>,
    },
}

/// Agent loop。
pub struct AgentLoop<'a> {
    driver: Driver<'a>,
    mode: Mode,
    limits: LoopLimits,
    plan_gate: PlanGate,
    emit: &'a dyn Fn(Event),
    session_id: Uuid,
    seq: u64,
    policy: ModePolicy,
    /// 已成功执行过的写/执行类工具。
    write_called: bool,
    /// 提醒回合是否已用过（整个 run 最多一次）。
    reminder_used: bool,
}

impl<'a> AgentLoop<'a> {
    /// 组装：provider/executor 由宿主提供；`Plan` 模式进入即规划态。
    ///
    /// 签名与行为（自 `new` 引入以来）**一字不变**：内部退化为
    /// [`Driver::Local`]（本层持历史、本层执行工具）。
    pub fn new(
        provider: &'a dyn ChatProvider,
        executor: &'a dyn ToolExecutor,
        mode: Mode,
        limits: LoopLimits,
        plan_gate: PlanGate,
        emit: &'a dyn Fn(Event),
    ) -> Self {
        let gate_active = mode == Mode::Plan || plan_gate.active;
        Self {
            driver: Driver::Local { provider, executor },
            mode,
            limits,
            plan_gate: PlanGate {
                active: gate_active,
                approved: plan_gate.approved,
            },
            emit,
            session_id: Uuid::nil(),
            seq: 0,
            policy: ModePolicy::of(mode),
            write_called: false,
            reminder_used: false,
        }
    }

    /// 组装（缺口 B1 的解法）：把**已建好的** [`Runtime`] 接入本层，一步的历史与
    /// 工具执行都落在 `Runtime` 上——每条消息经 [`Runtime::append_message`] 写进
    /// 会话、每步请求由 `runtime.get_session(id).messages` **重读**派生、工具经
    /// [`Runtime::settle_tool_call`] 真实执行并结算成会话消息。`new` 路径（本地
    /// [`ChatProvider`] / [`ToolExecutor`] 借用）行为不变。
    ///
    /// 三点如实说明（详见 `docs/loop-adapter-status.md`）：
    /// - **会话就是历史**：不另设历史容器，历史即 `session`；本构造器把
    ///   `session_id` 一并锚定。系统提示由 `Runtime` 组上下文时按会话模式生成
    ///   （[`Runtime::chat_turn`] → `build_turn_context`），不按字符串拼进请求。
    /// - **`step_executor` 只是工具面来源**：本层需要「当前可见工具有哪些」才能
    ///   构造 provider 请求；本地路径的工具面由 `ModePolicy` 直接给出，`Runtime`
    ///   路径则借宿主注入 Runtime 的同一个执行器、按 [`Runtime::check_tool`] 判定
    ///   可见性（口径与 [`Runtime::step`] 一致）。传 `None` 只影响工具面（请求里
    ///   无工具定义），不影响工具执行。
    /// - **门禁与事件仍留本层**：白名单 / 规划态判定、以及事件发射都还是本层的
    ///   （缺口 B2/B5 未解，跨窗口签名变更另行派卡）。
    pub fn with_runtime(
        runtime: Runtime,
        session: Uuid,
        step_executor: Option<&'a dyn ToolExecutor>,
        mode: Mode,
        limits: LoopLimits,
        plan_gate: PlanGate,
        emit: &'a dyn Fn(Event),
    ) -> Self {
        let gate_active = mode == Mode::Plan || plan_gate.active;
        Self {
            driver: Driver::Runtime {
                runtime,
                session,
                step_executor,
            },
            mode,
            limits,
            plan_gate: PlanGate {
                active: gate_active,
                approved: plan_gate.approved,
            },
            emit,
            session_id: session,
            seq: 0,
            policy: ModePolicy::of(mode),
            write_called: false,
            reminder_used: false,
        }
    }

    /// 设置事件归属的会话 id。
    #[must_use]
    pub fn session_id(mut self, id: Uuid) -> Self {
        self.session_id = id;
        self
    }

    /// 起始 seq（会话内事件续接锚点）。
    #[must_use]
    pub fn seq_base(mut self, seq: u64) -> Self {
        self.seq = seq;
        self
    }

    /// 当前白名单。
    pub fn tools(&self) -> &BTreeSet<String> {
        &self.policy.tools
    }

    /// 当前门禁状态。
    pub fn plan_gate(&self) -> PlanGate {
        self.plan_gate
    }

    /// 当前模式（`plan.exit` 批准后切 Craft）。
    pub fn mode(&self) -> Mode {
        self.mode
    }
}
