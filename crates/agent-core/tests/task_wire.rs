//! `x-task-wire`：任务工具族的 spec 清单与宿主侧适配（**不经过** registry 注册）。
//!
//! 本卡只产出 `src/tool_task.rs`（spec 清单）与 `src/task_adapter.rs`（调用适配）；
//! 注册进 `default_registry()` / 挂 `DispatchToolExecutor` 由汇总卡 `x-tool-registry`
//! 负责。因此本套件直接调用 `tool_specs()` 与 `task_adapter::execute`，守两条口径：
//!
//! 1. spec 表与 `mode` 白名单逐字一致（4 个名字、2 读 2 写、超时 5s、非敏感参数）；
//! 2. 四条流程真实落到 `Runtime` 的会话作用域任务入口：create → list → get →
//!    update，且 create / update 各自在事件日志里留下 `task.created` / `task.updated`。
//!
//! `lib.rs` 属共享区（本卡不得改），故用 `#[path]` 直接包含本卡产出的源文件，并给
//! 文件内的 `crate::...` 路径补最小 shim。汇总卡把它们声明进 `lib.rs` 后，可删掉
//! 这三行 shim 与 `#[path]`，改用 `agent_core::tool_task` / `agent_core::task_adapter`。

mod tool {
    pub use agent_core::tool::ToolKind;
}
mod r#loop {
    pub use agent_core::r#loop::ToolExecutor;
}
mod runtime {
    pub use agent_core::runtime::TaskToolExecutor;
}
pub use agent_core::Runtime;

#[path = "../src/task_adapter.rs"]
mod task_adapter;
#[path = "../src/tool_task.rs"]
mod tool_task;

use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

use agent_core::executor::{DispatchToolExecutor, TASK_TOOL_PREFIX};
use agent_core::mode::{READONLY_TOOLS, WRITE_TOOLS};
use agent_core::permission::{Decision, PermissionGate};
use agent_core::provider::EchoProvider;
use agent_core::r#loop::ToolExecutor;
use agent_core::runtime::{CreateSession, Task, TaskStatus};
use agent_core::schema::{Event, Isolation, Mode, PermissionMode};
use agent_core::tool::{ToolRegistry, ToolSpec};

// 线程本地受控时钟（Unix 毫秒）：事件时间戳确定，每个测试跑在独立线程。
thread_local! {
    static CLOCK: Cell<u64> = const { Cell::new(0) };
}

fn clock() -> u64 {
    CLOCK.with(|c| c.get())
}

fn set_clock(ms: u64) {
    CLOCK.with(|c| c.set(ms));
}

fn runtime(tag: &str) -> Runtime {
    let dir = std::env::temp_dir().join(format!("task-wire-{tag}-{}", Uuid::new_v4()));
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

/// 经本卡适配层调用一次任务工具（会话由宿主绑定，不从参数取）。
fn call(rt: &Runtime, session: Uuid, name: &str, args: Value) -> Result<Value, String> {
    task_adapter::execute(rt, session, name, &args)
}

/// 本会话的事件（kind 前缀 `task.`），按日志 seq 升序。
fn task_events(rt: &Runtime, session: Uuid) -> Vec<Event> {
    rt.event_log()
        .replay(session, 0, 500)
        .expect("replay")
        .into_iter()
        .filter(|event| event.kind.starts_with("task."))
        .collect()
}

// ---------------- 1. spec 清单：4 个名字、2 读 2 写、超时 5s ----------------

#[test]
fn tool_specs_cover_the_four_whitelisted_task_tools() {
    let specs = tool_task::tool_specs();
    let names: Vec<&str> = specs.iter().map(|(name, ..)| *name).collect();
    assert_eq!(
        names,
        vec!["task.list", "task.get", "task.create", "task.update"],
        "spec 表必须逐字覆盖白名单里的四个任务工具名"
    );

    // 与 mode 白名单交叉核对：读/写分段不能与 spec 的 kind 漂移。
    for (name, description, kind, sensitive, timeout_ms) in &specs {
        assert!(
            !description.trim().is_empty(),
            "{name} 的描述必须写清行为与失败语义，不能为空"
        );
        assert!(!*sensitive, "{name} 参数是任务元数据，不含凭据");
        assert_eq!(*timeout_ms, 5_000, "{name} 超时按卡面固定 5s");
        let expected = match *name {
            "task.list" | "task.get" => {
                assert!(READONLY_TOOLS.contains(name), "{name} 应列在只读白名单");
                agent_core::tool::ToolKind::Read
            }
            "task.create" | "task.update" => {
                assert!(WRITE_TOOLS.contains(name), "{name} 应列在写白名单");
                agent_core::tool::ToolKind::Write
            }
            other => panic!("意外的任务工具名: {other}"),
        };
        assert_eq!(*kind, expected, "{name} 的 kind 与 mode 白名单不一致");
    }
}

// ---------------- 2. create → list ----------------

#[test]
fn task_create_then_list_returns_the_created_task() {
    set_clock(1_000);
    let rt = runtime("create-list");
    let s = session(&rt, "S");

    let created: Task = serde_json::from_value(
        call(
            &rt,
            s,
            "task.create",
            json!({ "subject": "跑构建", "description": "cargo test 全绿" }),
        )
        .expect("task.create 应成功"),
    )
    .expect("结果是 Task 形状");

    assert_eq!(created.subject, "跑构建");
    assert_eq!(created.description, "cargo test 全绿");
    assert_eq!(created.status, TaskStatus::Pending, "初始状态 pending");
    assert_eq!(created.session_id, s);

    let listed = call(&rt, s, "task.list", json!({})).expect("task.list 应成功");
    assert_eq!(listed["count"], 1);
    let tasks: Vec<Task> = serde_json::from_value(listed["tasks"].clone()).expect("Task 列表");
    assert_eq!(tasks, vec![created.clone()], "list 必须能看到刚建的任务");

    // D6：创建必须落 `task.created` 事件（快照含完整任务）。
    let events = task_events(&rt, s);
    assert_eq!(events.len(), 1, "create 只落一条事件");
    assert_eq!(events[0].kind, "task.created");
    assert_eq!(events[0].payload["task_id"], created.id.to_string());
    assert_eq!(events[0].payload["snapshot"]["subject"], "跑构建");
}

// ---------------- 3. get by id ----------------

#[test]
fn task_get_by_id_returns_the_task_and_rejects_unknown_id() {
    set_clock(2_000);
    let rt = runtime("get");
    let s = session(&rt, "S");

    let created: Task = serde_json::from_value(
        call(
            &rt,
            s,
            "task.create",
            json!({ "subject": "审计", "description": "看变更" }),
        )
        .expect("create"),
    )
    .expect("Task");

    let fetched: Task = serde_json::from_value(
        call(&rt, s, "task.get", json!({ "id": created.id })).expect("task.get 应成功"),
    )
    .expect("Task");
    assert_eq!(fetched, created, "get 取回的内容必须与 create 一致");

    let err = call(&rt, s, "task.get", json!({ "id": Uuid::new_v4() }))
        .expect_err("不存在的 id 必须显式报错");
    assert_eq!(err, "任务不存在", "透传域层 NotFound 文案");
}

// ---------------- 4. update 状态 + 事件落盘 ----------------

#[test]
fn task_update_status_is_visible_via_get_and_emits_event() {
    set_clock(3_000);
    let rt = runtime("update");
    let s = session(&rt, "S");

    let created: Task = serde_json::from_value(
        call(&rt, s, "task.create", json!({ "subject": "上线" })).expect("create"),
    )
    .expect("Task");

    // 非法迁移：pending → done 跳过 in_progress，显式错误且状态不变。
    let err = call(
        &rt,
        s,
        "task.update",
        json!({ "id": created.id, "status": "done" }),
    )
    .expect_err("非法迁移必须报错");
    assert!(err.contains("非法状态迁移"), "应透传域层文案: {err}");
    let unchanged: Task =
        serde_json::from_value(call(&rt, s, "task.get", json!({ "id": created.id })).expect("get"))
            .expect("Task");
    assert_eq!(unchanged.status, TaskStatus::Pending, "拒绝路径零副作用");

    // 合法迁移：pending → in_progress。
    set_clock(4_000);
    let updated: Task = serde_json::from_value(
        call(
            &rt,
            s,
            "task.update",
            json!({ "id": created.id, "status": "in_progress" }),
        )
        .expect("合法迁移应成功"),
    )
    .expect("Task");
    assert_eq!(updated.status, TaskStatus::InProgress);

    let fetched: Task =
        serde_json::from_value(call(&rt, s, "task.get", json!({ "id": created.id })).expect("get"))
            .expect("Task");
    assert_eq!(fetched.status, TaskStatus::InProgress, "get 看到新状态");

    // D6：状态变更必须落 `task.updated` 事件（重复的非法更新不得留痕）。
    let events = task_events(&rt, s);
    assert_eq!(events.len(), 2, "create + update 各一条事件");
    assert_eq!(events[0].kind, "task.created");
    assert_eq!(events[1].kind, "task.updated");
    assert_eq!(events[1].payload["task_id"], created.id.to_string());
    assert_eq!(events[1].payload["status"], "in_progress");
}

// ---------------- 5. spec 表能机械地注册进 registry，并过权限门 ----------------

/// 汇总卡（`x-tool-registry`）的注册方式就是「把 `tool_specs()` 的每条元组
/// 逐字搬进 `ToolSpec`」。本用例照该方式注册一遍，守住两件事：
/// 1. 元组的五个字段**机械可搬**（类型对得上，描述不丢）；
/// 2. 注册后这四个名字在权限门里**不再**撞「工具未注册」那第一门。
///
/// ⛔ 本卡不改 `tool.rs` ⇒ 这里不经过 `default_registry()`，而是自己建
/// 一个 `ToolRegistry` 装 spec（与汇总卡等价的构造路径）。
fn registry_from_specs() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for (name, description, kind, sensitive_arguments, timeout_ms) in tool_task::tool_specs() {
        registry.register(ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            kind,
            sensitive_arguments,
            timeout_ms,
        });
    }
    registry
}

#[test]
fn tool_specs_register_into_a_registry_and_pass_the_permission_gate() {
    let registry = registry_from_specs();

    // 四个名字都在表里（登记口径）。
    for name in tool_task::TASK_TOOLS {
        assert!(registry.get(name).is_some(), "{name} 必须能注册进 registry");
    }
    assert_eq!(registry.len(), 4, "本域只贡献 4 个任务工具");

    // 权限门第 1 门（未注册 ⇒ Deny「工具未注册」）不再命中这两个读工具。
    let gate = PermissionGate::new(registry).with_whitelist(
        Mode::Craft,
        BTreeSet::from(tool_task::TASK_TOOLS.map(str::to_string)),
    );
    let decision = gate.check(Mode::Craft, PermissionMode::Full, "task.list", &json!({}));
    assert!(
        !matches!(&decision, Decision::Deny(reason) if reason.contains("工具未注册")),
        "注册后不得再撞「工具未注册」，实得 {decision:?}"
    );

    // ⛔ 未注册的名字**仍然** Deny「工具未注册」——门禁没被本卡放宽。
    let deny = gate.check(Mode::Craft, PermissionMode::Full, "task.delete", &json!({}));
    match deny {
        Decision::Deny(reason) => assert!(
            reason.contains("工具未注册"),
            "未注册的工具名仍须 Deny「工具未注册」，实得: {reason}"
        ),
        other => panic!("task.delete 必须 Deny，实得 {other:?}"),
    }
}

// ---------------- 6. 汇总卡能把它装进 DispatchToolExecutor 的 `task.` 槽 ----------------

/// 汇总卡（`x-tool-registry`）要把 `task.*` 挂到派发层，绑定的是
/// [`DispatchToolExecutor`] 的 `task.` 前缀槽——而该槽要求
/// `Arc<dyn ToolExecutor + Send + Sync>`。本用例照该形状装一次并**真调一次**，
/// 守住「汇总卡拿到本卡产物即可接线、不必重写适配逻辑」。
#[test]
fn adapter_binds_into_the_dispatch_executor_task_prefix() {
    set_clock(5_000);
    let rt = runtime("dispatch");
    let s = session(&rt, "S");

    let dispatcher = DispatchToolExecutor::new().bind(
        TASK_TOOL_PREFIX,
        Arc::new(task_adapter::TaskAdapter::new(Arc::new(rt.clone()), s)),
    );
    assert!(
        dispatcher.names().contains(&TASK_TOOL_PREFIX),
        "前缀槽已绑定"
    );

    // 经派发层真调一次 `task.create`：绑定正确 ⇒ 任务真的建成。
    let created: Task = serde_json::from_value(
        dispatcher
            .execute("task.create", &json!({ "subject": "派发建任务" }))
            .expect("派发层应能执行 task.create"),
    )
    .expect("Task");
    assert_eq!(created.subject, "派发建任务");
    assert_eq!(created.session_id, s);
    assert_eq!(rt.list_tasks(s), vec![created]);

    // 未被认领的名字 ⇒ 派发层自己报「无可用执行器认领工具」（不 panic）。
    let err = dispatcher
        .execute("fs.read", &json!({ "path": "/etc/hosts" }))
        .expect_err("未认领的名字应失败");
    assert!(
        err.contains(DispatchToolExecutor::UNMATCHED),
        "可读错误: {err}"
    );
}
