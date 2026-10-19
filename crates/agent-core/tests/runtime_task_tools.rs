//! `task.*` 工具族接线测试（W1 打通卡）：白名单里的四个逻辑工具名
//! （`task.list` / `task.get` / `task.create` / `task.update`）经
//! [`agent_core::TaskToolExecutor`] 真实落到 Runtime 的会话作用域任务入口，
//! 并**驱动一次真实回合**（端到端一例）。
//!
//! 语义出处：参考件 21 册「工具宇宙」TaskCreate/TaskGet/TaskUpdate/TaskList
//! （工具名与语义不改写）、18 册 §7 任务管理纪律（建 → 进行中 → 完成，标记完成
//! 不得攒批——本卡只保证工具层可表达，纪律属提示词层）。
//!
//! 覆盖矩阵（卡面必做二）：
//! 1. `task.create` 真落在会话上（`get_session` 可见 / `list_tasks` 返回）；
//! 2. `task.list` 只返回本会话任务；
//! 3. `task.get` 取他会话任务 ⇒ 既有 `NotFound` 文案且零副作用；
//! 4. `task.update` 非法迁移 ⇒ 报错且状态不变；
//! 5. 参数缺失 / 类型错误 / 未知工具名 ⇒ 可读错误、不 panic；
//! 6. 端到端：脚本化 provider 请求 `task.create`，经 `Runtime::send_text` 真实
//!    执行后任务存在**且** `task.created` 事件已落；
//! 7. 语境归属：`with_task_runtime` 之外直接调用执行器 ⇒ 可读错误（不串热态）。

use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::permission::PermissionGate;
use agent_core::provider::{EchoProvider, Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime, Task, TaskStatus};
use agent_core::schema::{Isolation, Mode, PermissionMode, Role};
use agent_core::tool::{ToolKind, ToolRegistry, ToolSpec};

// 线程本地受控时钟（Unix 毫秒）：时间戳断言确定。每个测试跑在独立线程。
thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

fn clock() -> u64 {
    CLOCK.with(|c| c.get())
}

fn set_clock(ms: u64) {
    CLOCK.with(|c| c.set(ms));
}

// ---------------- harness ----------------

fn runtime(tag: &str) -> Runtime {
    let dir = std::env::temp_dir().join(format!("task-tools-{tag}-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .task_board(agent_tasks::TaskBoard::with_clock(clock))
        .build()
        .expect("runtime")
}

fn session(rt: &Runtime, title: &str) -> Uuid {
    rt.create_session(&CreateSession::plain(title, Isolation::local_default()))
        .expect("create session")
        .id
}

/// 已装配好本会话任务工具的 Runtime（宿主的一行装配路径）。
fn rt_with_tools(tag: &str, session: Uuid) -> Runtime {
    let rt = runtime(tag);
    rt.install_task_tools(session);
    rt
}

/// 调执行器：模拟宿主按需调用工具（如审批通过后回填）。
///
/// `ctx = Some(rt)` 表示宿主自持 Runtime、以 [`Runtime::with_task_runtime`]
/// 显式带入语境（等价于回合内那次调用）；`ctx = None` 表示**裸调用**——
/// 契约上必须返回可读错误而不是作用到别的热态（覆盖项 7）。
fn call_in(ctx: Option<&Runtime>, rt: &Runtime, name: &str, args: Value) -> Result<Value, String> {
    let executor = rt.tool_executor().expect("已装配执行器");
    match ctx {
        Some(rt) => rt
            .with_task_runtime(|| executor.execute(name, &args))
            .unwrap_or_else(|e| Err(e.to_string())),
        None => executor.execute(name, &args),
    }
}

/// 语境内的工具调用（本文件绝大多数用例的口径）。
fn call(rt: &Runtime, name: &str, args: Value) -> Result<Value, String> {
    call_in(Some(rt), rt, name, args)
}

/// 本会话的任务事件（kind 前缀 `task.`），按日志 seq 升序。
fn task_events(rt: &Runtime, session: Uuid) -> Vec<agent_core::schema::Event> {
    rt.event_log()
        .replay(session, 0, 500)
        .expect("replay")
        .into_iter()
        .filter(|event| event.kind.starts_with("task."))
        .collect()
}

// ---------------- 1. task.create 真落在会话上 ----------------

#[test]
fn task_create_lands_on_the_session_board() {
    set_clock(1_000);
    let rt = runtime("create");
    let s = session(&rt, "S");
    rt.install_task_tools(s);

    let value = call(
        &rt,
        "task.create",
        json!({ "subject": "跑构建", "description": "cargo test 全绿" }),
    )
    .expect("task.create 应当成功");

    // 结果形状 = 域层 `Task` 的 serde（不另造字段名）。
    let task: Task = serde_json::from_value(value.clone()).expect("结果是 Task 形状");
    assert_eq!(task.subject, "跑构建");
    assert_eq!(task.description, "cargo test 全绿");
    assert_eq!(task.status, TaskStatus::Pending, "初始状态 pending");
    assert_eq!(task.session_id, s);
    assert_eq!(task.created_at_ms, 1_000, "受控时钟语义保留");

    // 会话热态可见：`get_task` / `list_tasks` 都能读到同一任务。
    let fetched = rt.get_task(s, task.id).expect("会话上可见");
    assert_eq!(fetched, task);
    let listed = rt.list_tasks(s);
    assert_eq!(listed, vec![task], "create 的产物就在本会话列表里");

    // 会话本身也是既有对象（不是落到空气上）。
    assert!(rt.get_session(s).is_ok());
}

// ---------------- 2. task.list 只返回本会话任务 ----------------

#[test]
fn task_list_is_scoped_to_this_session_only() {
    set_clock(2_000);
    let rt = runtime("list-scope");
    let a = session(&rt, "A");
    let b = session(&rt, "B");

    rt.install_task_tools(a);
    let mine = call(&rt, "task.create", json!({ "subject": "我的任务" })).expect("create A");
    let mine: Task = serde_json::from_value(mine).expect("Task");
    // 另一会话由它自己的执行器建任务（执行器按会话绑定）。
    rt.install_task_tools(b);
    call(&rt, "task.create", json!({ "subject": "别家任务" })).expect("create B");

    // 回到 A 的语境（A 的执行器）读列表。
    rt.install_task_tools(a);
    let listed = call(&rt, "task.list", json!({})).expect("list A");

    assert_eq!(listed["count"], 1, "只看到本会话的 1 个任务");
    let tasks: Vec<Task> = serde_json::from_value(listed["tasks"].clone()).expect("Task 列表");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, mine.id);
    assert_eq!(tasks[0].session_id, a);
    assert!(
        tasks.iter().all(|t| t.session_id == a),
        "B 的任务不得出现在 A 的 list 里"
    );

    // A 的 read 面同样看不到 B 的任务（越权 = 不存在）。
    let b_task_id = rt.list_tasks(b)[0].id;
    let err = call(&rt, "task.get", json!({ "id": b_task_id })).expect_err("跨会话应失败");
    assert_eq!(err, "任务不存在");
}

// ---------------- 3. task.get 跨会话 ⇒ NotFound 且零副作用 ----------------

#[test]
fn task_get_cross_session_is_not_found_without_side_effect() {
    set_clock(3_000);
    let rt = runtime("get-cross");
    let a = session(&rt, "A");
    let b = session(&rt, "B");
    rt.install_task_tools(a);
    let task: Task = serde_json::from_value(
        call(&rt, "task.create", json!({ "subject": "机密" })).expect("create"),
    )
    .expect("Task");

    // B 的执行器去取 A 的任务：文案与「不存在」逐字一致（不泄露存在性）。
    rt.install_task_tools(b);
    let not_found = agent_tasks::TaskError::NotFound.to_string();
    let err = call(&rt, "task.get", json!({ "id": task.id })).expect_err("跨会话应失败");
    assert_eq!(err, not_found);

    // 零副作用：A 侧任务原样（状态与 updated_at_ms 均未变）。
    let after = rt.get_task(a, task.id).expect("owner 仍可读");
    assert_eq!(after, task);
    // 越权尝试也不得在他会话留下事件痕迹。
    assert!(
        task_events(&rt, b).is_empty(),
        "被拒的读不得在他会话落任何任务事件"
    );
}

// ---------------- 4. task.update 非法迁移 ⇒ 报错且状态不变 ----------------

#[test]
fn task_update_invalid_transition_errors_and_keeps_status() {
    set_clock(4_000);
    let rt = rt_with_tools("update-invalid", Uuid::nil());
    let s = session(&rt, "S");
    rt.install_task_tools(s);
    let task: Task = serde_json::from_value(
        call(&rt, "task.create", json!({ "subject": "上线" })).expect("create"),
    )
    .expect("Task");

    // pending → done 跳过 in_progress：域层状态机拒绝（18 册 §7：建 → 进行中 → 完成）。
    let err = call(
        &rt,
        "task.update",
        json!({ "id": task.id, "status": "done" }),
    )
    .expect_err("非法迁移应失败");
    assert!(
        err.contains("非法状态迁移"),
        "错误应透传域层文案，实得: {err}"
    );

    let after = rt.get_task(s, task.id).expect("仍可读");
    assert_eq!(after.status, TaskStatus::Pending, "拒绝路径状态不变");
    assert_eq!(after, task, "拒绝路径零副作用（含时间戳）");
    assert!(
        task_events(&rt, s).iter().all(|e| e.kind == "task.created"),
        "被拒的更新不得落 task.updated"
    );

    // 合法链走通：pending → in_progress → done（先进行中再完成，不攒批）。
    let wip: Task = serde_json::from_value(
        call(
            &rt,
            "task.update",
            json!({ "id": task.id, "status": "in_progress" }),
        )
        .expect("合法"),
    )
    .expect("Task");
    assert_eq!(wip.status, TaskStatus::InProgress);
    let done: Task = serde_json::from_value(
        call(
            &rt,
            "task.update",
            json!({ "id": task.id, "status": "done" }),
        )
        .expect("合法"),
    )
    .expect("Task");
    assert_eq!(done.status, TaskStatus::Done);
    assert!(done.updated_at_ms >= wip.updated_at_ms, "时间戳只前进");
}

// ---------------- 5. 参数缺失 / 类型错误 / 未知工具名 ----------------

#[test]
fn bad_arguments_and_unknown_tool_are_readable_errors() {
    set_clock(5_000);
    let rt = runtime("bad-args");
    let s = session(&rt, "S");
    rt.install_task_tools(s);
    let task: Task = serde_json::from_value(
        call(&rt, "task.create", json!({ "subject": "T" })).expect("create"),
    )
    .expect("Task");

    // 参数缺失：subject 必填。
    let err = call(&rt, "task.create", json!({ "description": "没有 subject" }))
        .expect_err("缺必填参数应失败");
    assert!(err.starts_with("task.create 参数非法:"), "可读前缀: {err}");

    // 类型错误：id 不是 UUID。
    let err = call(&rt, "task.get", json!({ "id": 42 })).expect_err("类型错误应失败");
    assert!(err.starts_with("task.get 参数非法:"), "可读前缀: {err}");

    // 未知字段：不静默忽略（deny_unknown_fields）。
    let err =
        call(&rt, "task.get", json!({ "id": task.id, "extra": 1 })).expect_err("未知字段应失败");
    assert!(err.starts_with("task.get 参数非法:"), "可读前缀: {err}");

    // 参数整体不是对象。
    let err = call(&rt, "task.update", json!("not-an-object")).expect_err("形状错误应失败");
    assert!(err.starts_with("task.update 参数非法:"), "可读前缀: {err}");

    // `task.update` 三个可写字段一个都没给 ⇒ 可读错误（不静默成功）。
    let err = call(&rt, "task.update", json!({ "id": task.id })).expect_err("空更新应失败");
    assert!(err.contains("至少需要"), "可读错误: {err}");

    // `task.list` 不接受具名参数。
    let err = call(&rt, "task.list", json!({ "limit": 10 })).expect_err("list 无参数应失败");
    assert!(err.contains("不接受参数"), "可读错误: {err}");
    // 但省略/null/空对象都可接受。
    assert!(call(&rt, "task.list", Value::Null).is_ok());
    assert!(call(&rt, "task.list", json!({})).is_ok());

    // 未知工具名：本执行器只认领四个名字，报可读错误（不 panic）。
    let err = call(&rt, "task.delete", json!({})).expect_err("未知工具应失败");
    assert!(err.contains("未知任务工具: task.delete"), "可读错误: {err}");

    // 全部失败路径都不许污染看板。
    assert_eq!(rt.list_tasks(s), vec![task], "失败调用零副作用");
}

// ---------------- 6. 端到端：模型请求 ⇒ 任务真的建成 ----------------

/// 按队列出步的脚本化 provider（复用 `runtime_step_primitive` 的观测面口径）。
struct ScriptedProvider {
    steps: Mutex<VecDeque<Vec<ProviderStep>>>,
    seen: Mutex<Vec<TurnContext>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Vec<ProviderStep>>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.seen.lock().expect("锁").len()
    }
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported("端到端用例只走带上下文接口"))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("锁").push(context.clone());
        self.steps
            .lock()
            .expect("锁")
            .pop_front()
            .ok_or(CoreError::Unsupported(
                "脚本已耗尽：出现计划外的第 n+1 次调用",
            ))
    }
}

/// 注册 + 白名单 + auto 放行 ⇒ `Decision::Allow`（回合会真实调执行器）。
fn allowing_gate(name: &str) -> PermissionGate {
    let mut registry = ToolRegistry::new();
    registry.register(ToolSpec {
        name: name.to_string(),
        description: "任务工具".to_string(),
        kind: ToolKind::Write,
        sensitive_arguments: false,
        timeout_ms: 1_000,
    });
    PermissionGate::new(registry)
        .with_whitelist(Mode::Craft, BTreeSet::from([name.to_string()]))
        .with_auto_allowed(name)
}

fn craft_session(rt: &Runtime) -> Uuid {
    rt.create_session(&CreateSession {
        title: "任务工具端到端".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Craft,
        permission_mode: PermissionMode::Full,
        env_binding: None,
        isolation: Isolation {
            user_id: "u".into(),
            tenant_id: "t".into(),
            env_id: None,
        },
    })
    .expect("create session")
    .id
}

#[test]
fn model_request_executes_task_create_end_to_end() {
    set_clock(6_000);
    let p = ScriptedProvider::new(vec![
        vec![ProviderStep::ToolCall {
            name: "task.create".into(),
            args: json!({ "subject": "跑构建", "description": "修 10 个类型错误" }),
        }],
        vec![
            ProviderStep::Text("已建任务".into()),
            ProviderStep::Finished,
        ],
    ]);
    let dir = std::env::temp_dir().join(format!("task-tools-e2e-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(p.clone())
        .gate(allowing_gate("task.create"))
        .config(RuntimeConfig::default_v0())
        .task_board(agent_tasks::TaskBoard::with_clock(clock))
        .build()
        .expect("runtime");
    let s = craft_session(&rt);
    // 宿主一行装配：本会话的 `task.*` 接进去。
    rt.install_task_tools(s);

    let receipt = rt.send_text(s, "帮我建个任务").expect("回合");

    // 模型真的走完了工具步（两次 provider 调用 = 工具步 + 收敛步）。
    assert_eq!(p.calls(), 2, "工具步结算后续步开新调用");

    // 1) 任务真的落在会话上。
    let tasks = rt.list_tasks(s);
    assert_eq!(tasks.len(), 1, "模型请求的任务真的建成了");
    assert_eq!(tasks[0].subject, "跑构建");
    assert_eq!(tasks[0].description, "修 10 个类型错误");
    assert_eq!(tasks[0].status, TaskStatus::Pending);
    assert_eq!(tasks[0].session_id, s);

    // 2) `task.created` 事件已落（durable 事件先于读侧投影）。
    let events = task_events(&rt, s);
    assert_eq!(events.len(), 1, "恰好一条任务事件");
    assert_eq!(events[0].kind, "task.created");
    assert_eq!(events[0].payload["type"], "task.created");
    assert_eq!(events[0].payload["task_id"], json!(tasks[0].id));

    // 3) 历史上 ToolResult 是**真实结果**（不是 pending 占位、不是 error）。
    let tr = rt
        .get_session(s)
        .expect("session")
        .messages
        .into_iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("ToolResult 消息在历史里");
    let settled: Value = serde_json::from_str(&tr.content).expect("真实结果应是 JSON");
    assert_eq!(settled["type"], Value::Null, "不是 error 形状");
    assert_eq!(settled["note"], Value::Null, "不是 pending 占位形状");
    assert_eq!(
        settled["id"],
        json!(tasks[0].id),
        "工具返回的就是这个任务（真实结果，非占位）"
    );
    assert_eq!(
        settled["status"], "pending",
        "这里的 pending 是**任务自己的状态**（TaskStatus::Pending），\
         不是 status=pending 的占位——以 id/note 字段区分"
    );

    // 4) 回合级 ToolResult 事件成功结算（isError 缺席 = 成功）。
    let tool_result = receipt
        .events
        .iter()
        .find(|e| e.kind == "tool_result")
        .expect("成功执行必须发 tool_result 事件");
    assert_ne!(
        tool_result.payload["isError"],
        json!(true),
        "成功执行不得标 isError"
    );
}

// ---------------- 7. 语境归属 ----------------

#[test]
fn executor_outside_runtime_context_is_readable_error() {
    set_clock(7_000);
    let rt = runtime("no-context");
    let s = session(&rt, "S");
    // 故意**不**经 Runtime 发起：直接拿执行器按需调用（`call_in(None)`）。
    rt.install_task_tools(s);

    // 写：无语境 ⇒ 可读错误，且**不得**落到看板上。
    let err = call_in(None, &rt, "task.create", json!({ "subject": "幽灵任务" }))
        .expect_err("无语境应失败");
    assert!(err.contains("无执行语境"), "可读错误: {err}");
    assert!(
        rt.list_tasks(s).is_empty(),
        "无语境的执行不得作用到任何热态（拒绝路径零副作用）"
    );

    // 读：无语境 ⇒ 空列表（「看不到任务」比 panic 更真实），不串到别的热态。
    let listed = call_in(None, &rt, "task.list", json!({})).expect("list 无语境为空");
    assert_eq!(listed["count"], 0);

    // 单任务读取/更新同样报可读错误。
    let err =
        call_in(None, &rt, "task.get", json!({ "id": Uuid::new_v4() })).expect_err("无语境应失败");
    assert!(err.contains("无执行语境"), "可读错误: {err}");

    // 语境是**栈式**的：Runtime 发起时可读、返回后即刻失效（不残留、不串味）。
    let created: Task = serde_json::from_value(
        call(&rt, "task.create", json!({ "subject": "正常任务" })).expect("ok"),
    )
    .expect("Task");
    let after = call_in(None, &rt, "task.list", json!({})).expect("list");
    assert_eq!(
        after["count"], 0,
        "回合外的裸执行器不得看到上一个语境里的任务"
    );
    assert_eq!(rt.list_tasks(s), vec![created]);
}

/// 嵌套不同 Runtime ⇒ 语境归属冲突被拒（不悄悄用别人的热态）。
#[test]
fn nested_foreign_runtime_context_is_rejected() {
    set_clock(8_000);
    let a = runtime("nest-a");
    let b = runtime("nest-b");
    let sa = session(&a, "A");
    let sb = session(&b, "B");
    a.install_task_tools(sa);
    b.install_task_tools(sb);
    let a_task: Task = serde_json::from_value(
        call(&a, "task.create", json!({ "subject": "A 的任务" })).expect("ok"),
    )
    .expect("Task");

    // 在 B 的语境里，A 必须能识别出「此刻服务的不是我」——这正是归属断言的取数依据：
    // 归属不符时 A 不会悄悄读 B 的热态，而是 `is_serving_this_runtime() == false`，
    // 由 A 自己的入口把「无归属」转成可读错误（见 `current_task_runtime`）。
    b.with_task_runtime(|| {
        assert!(
            !a.is_serving_this_runtime(),
            "B 的语境里 A 不得认为自己在服务"
        );
        assert!(b.is_serving_this_runtime(), "B 的语境里 B 应当认领");
        // A 试图在 B 的语境里入栈 ⇒ 归属断言拒绝（不 panic、可读文案）。
        let err = a
            .with_task_runtime(|| a.is_serving_this_runtime())
            .expect_err("A 嵌进 B 的语境应被拒");
        let message = err.to_string();
        assert!(message.contains("归属冲突"), "可读错误: {message}");
        assert!(
            message.contains("不同 Runtime 嵌套执行"),
            "文案要说清是什么被拒: {message}"
        );
    })
    .expect("B 自身入栈应当放行");
    // 被拒的嵌套**没有把 A 压进栈**：B 出栈后没有任何残留语境。
    assert!(!a.is_serving_this_runtime(), "被拒的嵌套不得留下语境");
    assert!(!b.is_serving_this_runtime(), "出栈后无残留");

    // 同归属（自己嵌自己）放行，且读到的就是自己的热态。
    let listed = b
        .with_task_runtime(|| call(&b, "task.list", json!({})))
        .expect("同归属应当放行")
        .expect("list ok");
    assert_eq!(listed["count"], 0, "B 的语境只服务 B 的热态");

    // A 的任务仍在 A 的热态里，B 的跨读一律 NotFound。
    assert_eq!(a.list_tasks(sa), vec![a_task.clone()]);
    assert_eq!(
        call(&b, "task.get", json!({ "id": a_task.id })).expect_err("跨会话读"),
        agent_tasks::TaskError::NotFound.to_string()
    );
}

// ---------------- 8. 装配面的运行期语义 ----------------

/// `install_task_tools` 与 `RuntimeBuilder::tool_executor` 是**同一槽位**，
/// 后写胜出；清空后回到「未注入执行器」的 pending 占位语义。
#[test]
fn install_replaces_slot_and_clear_restores_pending() {
    set_clock(9_000);
    let rt = runtime("slot");
    let s = session(&rt, "S");
    assert!(rt.tool_executor().is_none(), "未装配时槽位为空");

    rt.install_task_tools(s);
    assert!(rt.tool_executor().is_some(), "安装后槽位有执行器");
    let task: Task =
        serde_json::from_value(call(&rt, "task.create", json!({ "subject": "T" })).expect("ok"))
            .expect("Task");

    // 后写胜出：换一个别的会话的执行器，槽位只有一个。
    let other = session(&rt, "Other");
    rt.install_task_tools(other);
    let err = call(&rt, "task.get", json!({ "id": task.id })).expect_err("已换会话上下文");
    assert_eq!(err, "任务不存在", "新执行器绑定新会话（后写胜出）");

    rt.clear_tool_executor();
    assert!(
        rt.tool_executor().is_none(),
        "清空后槽位为空（恢复 pending 占位）"
    );
    assert_eq!(rt.list_tasks(s), vec![task], "清空执行器不影响已落库的任务");
}

/// 未装配执行器 ⇒ 工具步落 pending 占位（既有兼容语义不被本卡改写）。
#[test]
fn without_install_tool_step_keeps_pending_placeholder() {
    set_clock(10_000);
    let p = ScriptedProvider::new(vec![
        vec![ProviderStep::ToolCall {
            name: "task.create".into(),
            args: json!({ "subject": "不该建成" }),
        }],
        vec![ProviderStep::Finished],
    ]);
    let dir = std::env::temp_dir().join(format!("task-tools-pending-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(p)
        .gate(allowing_gate("task.create"))
        .config(RuntimeConfig::default_v0())
        .task_board(agent_tasks::TaskBoard::with_clock(clock))
        .build()
        .expect("runtime");
    let s = craft_session(&rt);

    let receipt = rt.send_text(s, "建个任务").expect("回合");

    assert!(rt.list_tasks(s).is_empty(), "未装配 ⇒ 运行时不执行工具");
    assert!(task_events(&rt, s).is_empty(), "占位结算不落任务事件");
    let tr = rt
        .get_session(s)
        .expect("session")
        .messages
        .into_iter()
        .find(|m| m.role == Role::ToolResult)
        .expect("占位 ToolResult 在历史里");
    assert!(
        tr.content.contains("pending"),
        "占位形状不变: {}",
        tr.content
    );
    assert!(
        !receipt.events.iter().any(|e| e.kind == "tool_result"),
        "占位不发 tool_result 事件"
    );
}

// ---------------- 9. 测试与自检用的归属观测面 ----------------

/// `is_serving_this_runtime` 在装配自检路径上被真实使用：语境内外判定一致。
#[test]
fn is_serving_this_runtime_tracks_the_context_stack() {
    set_clock(11_000);
    let rt = rt_with_tools("serving", Uuid::nil());
    assert!(!rt.is_serving_this_runtime(), "栈空时不算在服务");

    let inside = rt
        .with_task_runtime(|| rt.is_serving_this_runtime())
        .expect("入语境");
    assert!(inside, "语境栈顶是本 Runtime");
    assert!(
        !rt.is_serving_this_runtime(),
        "出栈后判定恢复为否（不残留）"
    );

    // 全仓装配自检面：无语境时 `has_task_runtime_context` 为否——宿主据此可
    // 提示「忘了 install_task_tools」，而不是让工具步静默落 pending 占位。
    assert!(!Runtime::has_task_runtime_context());
    let inside_any = rt
        .with_task_runtime(Runtime::has_task_runtime_context)
        .expect("入语境");
    assert!(inside_any);
    assert!(!Runtime::has_task_runtime_context());
}
