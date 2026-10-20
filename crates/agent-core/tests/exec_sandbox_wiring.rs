//! 执行端接入沙箱（h1-exec-sandbox-wire）端到端接线测试。
//!
//! 验收焦点：**放行路径真的进沙箱**，而不是「名字对上了」而已。分三层取证：
//!
//! 1. **路由**：`shell.exec` 经 [`DispatchToolExecutor`] 落到 [`ExecToolExecutor`]，
//!    陌生名字得到**可读错误**而不是 panic；
//! 2. **真进沙箱**：`GuardedExec` 内承担真实执行的 `agent_sandbox::run_sandboxed_bounded`
//!    被调用 ⇒ 用**旁路标志文件**观察（沙箱层一旦执行命令，标志文件必然出现）。
//!    这比「比对返回值」强：返回值在无沙箱直跑路径上同样会成功；
//! 3. **放行前置**：`GuardedExec` 判 Deny / NeedsApproval ⇒ 工具结果 `Err`
//!    （宿主结算成 `isError=true`），且**无进程产生**（标志文件不出现）。
//!
//! 工具名收敛断言：`EXEC_TOOL_NAME` 必须与 `mode::WRITE_TOOLS` 白名单条目逐字一致
//! ——白名单是逻辑工具名的唯一事实来源。依赖系统程序的用例（`touch`/`printf`）缺失时跳过。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::executor::{DispatchToolExecutor, ExecToolExecutor, EXEC_TOOL_NAME};
use agent_core::mode::WRITE_TOOLS;
use agent_core::runtime::Runtime;
use agent_core::ToolExecutor;
use agent_execpolicy::argv::Policy;
use agent_execpolicy::Decision;
use agent_sandbox::{ExecBounds, SandboxPolicy, SandboxablePreference};
use agent_tools_exec::GuardedExec;
use serde_json::{json, Value};
use uuid::Uuid;

/// 最小 Runtime（本卡不测权限门 / provider，只需能装配执行器）。
fn runtime() -> Runtime {
    Runtime::builder()
        .provider(Arc::new(agent_core::provider::EchoProvider))
        .build()
        .expect("Runtime 必须可构造")
}

/// 测试专用策略构造：逐条加前缀规则。
fn policy_with(rules: &[(&[&str], Decision)]) -> Policy {
    let mut policy = Policy::empty();
    for (prefix, decision) in rules {
        let prefix: Vec<String> = prefix.iter().map(|token| (*token).to_string()).collect();
        policy
            .add_prefix_rule(&prefix, *decision)
            .expect("前缀规则构造必须成功");
    }
    policy
}

/// 判定 → 执行器；`Forbid` 偏好（不启用沙箱隔离，但仍走 `GuardedExec` 链）。
///
/// 本卡断言的是「经不经 `GuardedExec`/沙箱分发层」，`Forbid` 让断言在无 bwrap 的
/// 研究机上也能成立（沙箱层照常被调用、照常执行命令）。
fn guarded(policy: Policy) -> Arc<GuardedExec> {
    Arc::new(GuardedExec {
        policy,
        pref: SandboxablePreference::Forbid,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds::default(),
    })
}

/// `PATH` 中是否存在该程序（缺失时跳过依赖系统程序的用例）。
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 把路径安全地拼进 shell 命令行单引号段（外包单引号、内部单引号转义）。
fn quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// 放行路径执行 `touch <marker>` 的 argv 形状（系统无 `touch` 时用例跳过）。
macro_rules! require_touch {
    () => {
        if which("touch").is_none() {
            eprintln!("跳过：本机 PATH 中没有 touch");
            return;
        }
    };
}

/// 把 `touch <marker>` 拼成工具调用参数（即本卡 `command` 形状）。
fn touch_args(marker: &std::path::Path) -> Value {
    json!({ "command": format!("touch {}", quote(marker)) })
}

// ---- 1. 工具名收敛（断点 2） ----

/// `EXEC_TOOL_NAME` 必须与白名单条目逐字一致：白名单是唯一事实来源。
#[test]
fn exec_tool_name_matches_whitelist_entry() {
    assert_eq!(EXEC_TOOL_NAME, "shell.exec", "执行体认领的名字");
    assert!(
        WRITE_TOOLS.contains(&EXEC_TOOL_NAME),
        "EXEC_TOOL_NAME 必须在 mode::WRITE_TOOLS 白名单里逐字一致；实际白名单 {WRITE_TOOLS:?}"
    );
}

// ---- 2. 路由：shell.exec 经分派器落到 exec 执行体 ----

/// 分派器认领之外的名字 ⇒ **可读错误**（不 panic、不静默成功）。
#[test]
fn dispatch_unknown_tool_name_is_readable_error() {
    let dispatcher = DispatchToolExecutor::new();

    let message = dispatcher
        .execute("fs.write", &json!({ "path": "/tmp/x" }))
        .expect_err("无匹配必须返回 Err");

    assert!(
        message.contains(DispatchToolExecutor::UNMATCHED),
        "错误必须含稳定前缀: {message}"
    );
    assert!(
        message.contains("fs.write"),
        "错误必须回显工具名: {message}"
    );
}

/// 端到端：`shell.exec` 经分派器 **真的执行** 并被 `GuardedExec` 放行。
///
/// 标记文件由被执行的 `touch` 产出 —— 说明路由落到了 `ExecToolExecutor`，
/// 而后者内部只有 `GuardedExec::run` 一条出口。
#[test]
fn dispatch_routes_shell_exec_to_exec_executor() {
    require_touch!();
    let dir = tempfile::tempdir().expect("临时目录");
    let marker = dir.path().join("routed-marker");

    let dispatcher = DispatchToolExecutor::new().bind(
        EXEC_TOOL_NAME,
        Arc::new(ExecToolExecutor::new(guarded(policy_with(&[(
            &["touch"],
            Decision::Allow,
        )])))),
    );

    let result = dispatcher
        .execute(EXEC_TOOL_NAME, &touch_args(&marker))
        .expect("allow 规则下必须执行成功");

    assert_eq!(result["exit_code"].as_i64(), Some(0), "退出码应为 0");
    assert!(marker.exists(), "命令必须真的执行（标记文件应存在）");
}

/// 分派器保留子执行器自身的错误语义（坏参数在 `ExecToolExecutor` 处也 Err）。
#[test]
fn dispatch_preserves_sub_executor_error() {
    let dispatcher = DispatchToolExecutor::new().bind(
        EXEC_TOOL_NAME,
        Arc::new(ExecToolExecutor::new(guarded(policy_with(&[(
            &["echo"],
            Decision::Allow,
        )])))),
    );

    // 形状不合法 ⇒ 子执行器的静态错误原样透传（不被分派层改写/吞掉）。
    let message = dispatcher
        .execute(EXEC_TOOL_NAME, &json!({}))
        .expect_err("坏参数必须 Err");

    assert!(
        message.contains("shell.exec 参数不合法"),
        "子执行器错误必须原样透传: {message}"
    );
}

// ---- 3. 放行路径真的进沙箱（本卡核心） ----

/// **放行 ⇒ 经沙箱分发层执行**：断言方式 = `GuardedExec` 内的
/// `agent_sandbox::run_sandboxed_bounded` 真的跑了命令（旁观标志文件），
/// 且结果带沙箱标签（该字段由沙箱层填充，执行体只做投影）。
///
/// 反证：若执行体绕过 `GuardedExec` 直接 `std::process::Command::spawn`，
/// 标志文件仍会出现，但 [`deny_path_produces_no_process`] 会因「拒绝却留下文件」
/// 而失败 —— 两者合起来才钉住「必须过判定 + 分发层」。
#[test]
fn allow_path_really_goes_through_sandbox_layer() {
    require_touch!();
    let dir = tempfile::tempdir().expect("临时目录");
    let marker = dir.path().join("sandboxed-marker");

    let exec = ExecToolExecutor::new(guarded(policy_with(&[(&["touch"], Decision::Allow)])));

    let result = exec
        .execute(EXEC_TOOL_NAME, &touch_args(&marker))
        .expect("allow 必须真实执行");

    assert_eq!(result["exit_code"].as_i64(), Some(0));
    assert!(
        result["sandbox"].as_str().is_some(),
        "结果必须带沙箱标签（沙箱分发层填充）: {result:?}"
    );
    assert!(marker.exists(), "命令必须真的执行（标记文件应存在）");
}

/// **Deny / 未命中 ⇒ 无进程产生**：`GuardedExec` 判 `Deny`（或未命中转审批）时
/// 工具结果必须是 `Err`（宿主据此结算成 `isError=true`），且命令不得被执行。
#[test]
fn blocked_paths_produce_no_process() {
    // (规则, 标记名, 期望错误文案片段)——两条 fail-closed 分支各一。
    let cases = [
        (
            policy_with(&[(&["touch"], Decision::Deny)]),
            "denied-marker",
            "策略拒绝",
        ),
        // 只放一条不相干的 allow 规则 ⇒ 本命令未命中 ⇒ 走启发式 Ask。
        (
            policy_with(&[(&["git", "status"], Decision::Allow)]),
            "approval-marker",
            "需人工审批",
        ),
    ];

    for (policy, marker_name, expected) in cases {
        let dir = tempfile::tempdir().expect("临时目录");
        let marker = dir.path().join(marker_name);
        let exec = ExecToolExecutor::new(guarded(policy));

        let message = exec
            .execute(EXEC_TOOL_NAME, &touch_args(&marker))
            .expect_err("被拦下的命令必须 Err");

        assert!(
            message.contains(expected),
            "错误必须含 {expected:?}: {message}"
        );
        assert!(!marker.exists(), "被拦下的命令不得执行（{marker_name}）");
    }
}

// ---- 4. Runtime 装配入口（断点 1 / 3） ----

/// 两个装配入口都把 `shell.exec` 接到沙箱链：装上槽位后**确实**可执行。
#[test]
fn install_entries_wire_exec_into_runtime_slot() {
    require_touch!();
    let dir = tempfile::tempdir().expect("临时目录");

    for (tag, install) in [
        ("exec-dispatch", InstallKind::ExecDispatch),
        ("builtin-tools", InstallKind::Builtin),
    ] {
        let marker = dir.path().join(format!("{tag}-marker"));
        let rt = runtime();
        install.apply(&rt, guarded(policy_with(&[(&["touch"], Decision::Allow)])));

        let executor = rt.tool_executor().expect("槽位必须已装配执行器");
        executor
            .execute(EXEC_TOOL_NAME, &touch_args(&marker))
            .unwrap_or_else(|e| panic!("{tag}: shell.exec 必须可执行: {e}"));
        assert!(marker.exists(), "{tag}: 装配后的调用必须真的执行");
    }
}

/// 两种 Runtime exec 装配入口（见 [`Runtime::install_exec_dispatch`] /
/// [`Runtime::install_builtin_tools`]）。
enum InstallKind {
    ExecDispatch,
    Builtin,
}

impl InstallKind {
    fn apply(&self, rt: &Runtime, guarded: Arc<GuardedExec>) {
        match self {
            Self::ExecDispatch => rt.install_exec_dispatch(guarded),
            Self::Builtin => rt.install_builtin_tools(Uuid::new_v4(), guarded),
        }
    }
}

/// 组合装配后 `task.*` 与 `shell.exec` **共存**（同一槽位内的两条路由）。
///
/// 断点 3 的「缓解」验收：`task.create` 打到任务执行器（真实建任务）、`shell.exec`
/// 打到沙箱链，两者不再互相顶掉。会话作用域：`ToolExecutor` 签名不含会话，`task.*`
/// 适配层靠**执行语境**（`Runtime::with_task_runtime`，生产上由 `execute_tool` 的
/// `Decision::Allow` 分支入栈）取 Runtime/看板——此处复刻同一语境。
#[test]
fn install_builtin_tools_allows_task_and_exec_to_coexist() {
    let rt = runtime();
    let session = rt
        .create_session(&agent_core::runtime::CreateSession::plain(
            "wiring 共存用例",
            agent_core::schema::Isolation::local_default(),
        ))
        .expect("会话必须可创建")
        .id;
    rt.install_builtin_tools(session, guarded(policy_with(&[])));

    let executor = rt.tool_executor().expect("槽位必须已装配组合执行器");

    // `task.create` 走任务执行器：真实建出一个任务（而非无匹配错误）。
    let created = rt
        .with_task_runtime(|| executor.execute("task.create", &json!({ "subject": "共存" })))
        .expect("入语境必须成功")
        .expect("task.create 必须由任务执行器认领");
    assert!(created["id"].as_str().is_some(), "应返回任务: {created:?}");

    // `shell.exec` 走沙箱链：坏参数得到 exec 执行体的静态错误（不是「无匹配」）。
    let exec_error = executor
        .execute(EXEC_TOOL_NAME, &json!({}))
        .expect_err("坏参数必须 Err");
    assert!(
        exec_error.contains("shell.exec 参数不合法"),
        "shell.exec 必须路由到 exec 执行体而非落入无匹配: {exec_error}"
    );
}

// ---- 5. 分词边界（`command` → argv）与装配契约自检 ----

/// 引号内的空白不被拆分：`printf %s 'a b'` 中 `a b` 是**单**参数。
#[test]
fn quoted_argument_whitespace_is_not_split() {
    if which("printf").is_none() {
        eprintln!("跳过：本机 PATH 中没有 printf");
        return;
    }
    let exec = ExecToolExecutor::new(guarded(policy_with(&[(&["printf"], Decision::Allow)])));

    // 若被误拆成多参数，printf 会重复/错位输出。
    let result = exec
        .execute(EXEC_TOOL_NAME, &json!({ "command": "printf %s 'a b'" }))
        .expect("printf 必须执行成功");

    assert_eq!(
        result["stdout"].as_str().unwrap_or_default(),
        "a b",
        "引号内的空白必须保留在单一参数里: {result:?}"
    );
}

/// 未闭合引号 ⇒ 静态拒绝（fail-closed，不猜意图），且不执行。
#[test]
fn unterminated_quote_is_rejected() {
    let exec = ExecToolExecutor::new(guarded(policy_with(&[(&["echo"], Decision::Allow)])));
    let result = exec.execute(EXEC_TOOL_NAME, &json!({ "command": "echo 'unterminated" }));
    assert!(result.is_err(), "未闭合引号必须 Err");
}

/// 装配契约自检：绑定表覆盖白名单名字空间，且分派优先级 = 绑定顺序。
#[test]
fn builtin_bindings_cover_policy_namespaces_and_dispatch_order() {
    let bindings = agent_core::executor::builtin_tool_bindings();
    assert!(bindings.contains_key(EXEC_TOOL_NAME));
    assert!(bindings.contains_key(agent_core::executor::TASK_TOOL_PREFIX));
    // 前缀键必须真的以 `.` 结尾，否则会被当成精确名。
    let prefix = *bindings
        .keys()
        .find(|key| key.ends_with('.'))
        .expect("必须有前缀绑定");
    assert!(prefix.ends_with('.'));

    // 交叉自检：`shell.exec` 在写白名单里，`task.` 前缀确实覆盖白名单 `task.*` 条目。
    let known: BTreeSet<&str> = WRITE_TOOLS.iter().copied().collect();
    assert!(
        known.contains(EXEC_TOOL_NAME),
        "shell.exec 必须在写白名单里"
    );
    assert!(
        known.iter().any(|name| name.starts_with(prefix)),
        "prefix {prefix:?} 必须覆盖白名单里的 task.* 条目"
    );
    assert_eq!(bindings.len(), 2, "组合装配恰有两条路由");

    // 分派：前缀只匹配同族（`taskface` 不匹配），精确名优先且**先绑者胜**。
    let dispatcher = DispatchToolExecutor::new()
        .bind("task.", Arc::new(FixedExecutor::new("task")))
        .bind(EXEC_TOOL_NAME, Arc::new(FixedExecutor::new("first")))
        .bind(EXEC_TOOL_NAME, Arc::new(FixedExecutor::new("second")));

    let hit = |name: &str| dispatcher.execute(name, &Value::Null);
    assert_eq!(hit("task.list").unwrap(), json!("task"));
    assert_eq!(hit("task.create").unwrap(), json!("task"));
    assert_eq!(hit(EXEC_TOOL_NAME).unwrap(), json!("first"), "后绑不覆盖");
    assert!(hit("taskface").is_err(), "`task.` 前缀不得吞近邻");
    assert_eq!(dispatcher.len(), 3, "绑定条目不得被静默丢弃");
    assert_eq!(dispatcher.names().len(), 3);
}

/// 固定应答执行器（只用于路由断言，不触达沙箱）。
struct FixedExecutor(&'static str);

impl FixedExecutor {
    fn new(tag: &'static str) -> Self {
        Self(tag)
    }
}

impl ToolExecutor for FixedExecutor {
    fn execute(&self, _name: &str, _args: &Value) -> Result<Value, String> {
        Ok(json!(self.0))
    }
}
