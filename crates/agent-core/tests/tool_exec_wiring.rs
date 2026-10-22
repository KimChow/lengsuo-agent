//! `fs.*` 与 `web.fetch` 的执行接线验收（`x-tool-exec-wiring`）。
//!
//! 验收焦点：这 7 个工具**注册了却没人认领**——本卡把执行体接进运行时的分派槽。
//! 覆盖 D8 的九条：`fs.` 前缀路由 / `web.fetch` 精确名路由 / 未认领工具仍报
//! `UNMATCHED` / 四族共存 / 绑定表≡实际装配 / `fs.write` 未授权被门拒 /
//! 工作区根缺失⇒`fs.` 不装 / 精确名优先于前缀 / crate 本体可见性。
//!
//! 抓取用本地 mock 端口（**不触网**）。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::executor::{DispatchToolExecutor, EXEC_TOOL_NAME, TASK_TOOL_PREFIX};
use agent_core::fs_adapter::{FsToolExecutor, FS_TOOL_NAMES};
use agent_core::permission::PermissionGate;
use agent_core::provider::{EchoProvider, Provider, ProviderStep};
use agent_core::r#loop::ToolExecutor;
use agent_core::schema::{Isolation, Mode, PermissionMode, Role};
use agent_core::tool::default_registry;
use agent_core::tool_exec_wiring::{FS_TOOL_PREFIX, TOOL_BINDING_TABLE, WEB_FETCH_BINDING};
use agent_core::web_adapter::{WebFetchAdapter, WebFetchPage, WebFetchPort};
use agent_core::{CoreError, CreateSession, Runtime};
use agent_execpolicy::argv::Policy;
use agent_sandbox::{ExecBounds, SandboxPolicy, SandboxablePreference};
use agent_tools_exec::GuardedExec;
use serde_json::{json, Value};
use tempfile::TempDir;
use uuid::Uuid;

// --- 测试脚手架 ---

/// 本地 mock 抓取端口：只认一个 URL，返回预置页面（**不触网**）。
struct MockPort {
    url: String,
    page: WebFetchPage,
}

impl MockPort {
    fn with_page(url: &str, text: &str) -> Arc<dyn WebFetchPort> {
        Arc::new(Self {
            url: url.to_string(),
            page: WebFetchPage {
                final_url: url.to_string(),
                title: Some("本地测试页".to_string()),
                text: text.to_string(),
            },
        })
    }
}

impl WebFetchPort for MockPort {
    fn fetch(&self, url: &str) -> Result<WebFetchPage, String> {
        if url == self.url {
            Ok(self.page.clone())
        } else {
            Err(format!("IO 失败: 未预置 {url}"))
        }
    }
}

/// 白名单 + auto 放行给定工具名；**其余工具不在白名单 ⇒ deny-by-default**。
fn gate_allowing(names: &[&str]) -> PermissionGate {
    let allowed: BTreeSet<String> = names.iter().map(|n| (*n).to_string()).collect();
    let mut gate = PermissionGate::new(default_registry()).with_whitelist(Mode::Craft, allowed);
    for name in names {
        gate = gate.with_auto_allowed(name);
    }
    gate
}

/// 最小 Runtime（provider 用 [`EchoProvider`]，门可控）。
fn runtime_with_gate(gate: PermissionGate) -> Runtime {
    Runtime::builder()
        .provider(Arc::new(EchoProvider))
        .gate(gate)
        .build()
        .expect("Runtime 必须可构造")
}

/// 建一个 Craft + Full 的会话；`workspace_root` 原样落进会话。
fn session(rt: &Runtime, workspace_root: Option<String>) -> Uuid {
    rt.create_session(&CreateSession {
        title: "执行接线自检".into(),
        workspace_root,
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

/// 临时工作区 + 会话（工作区根 = 临时目录）。
fn workspace_session(rt: &Runtime) -> (TempDir, Uuid) {
    let dir = TempDir::new().expect("临时目录");
    let session = session(rt, Some(dir.path().display().to_string()));
    (dir, session)
}

/// 装配域族工具（`fs.` + `web.fetch`）并取回执行器。
fn install(
    rt: &Runtime,
    s: Uuid,
    web: Option<Arc<dyn WebFetchPort>>,
) -> Arc<dyn ToolExecutor + Send + Sync> {
    rt.install_domain_tools(s, web).expect("装配");
    rt.tool_executor().expect("执行器已装")
}

/// 判定 → 执行器；`Forbid` 偏好（不启沙箱隔离，但仍走 `GuardedExec` 链）。
fn guarded() -> Arc<GuardedExec> {
    Arc::new(GuardedExec {
        policy: Policy::empty(),
        pref: SandboxablePreference::Forbid,
        sandbox_policy: SandboxPolicy::default(),
        bounds: ExecBounds::default(),
    })
}

// --- 1) `fs.` 前缀路由 ---

#[test]
fn fs前缀路由到文件系统适配器() {
    let rt = runtime_with_gate(gate_allowing(&["fs.read"]));
    let (dir, s) = workspace_session(&rt);
    std::fs::write(dir.path().join("notes.txt"), "hello fs").expect("写临时文件");

    let executor = install(&rt, s, None);
    let out = executor
        .execute("fs.read", &json!({ "path": "notes.txt" }))
        .expect("fs.read 必须被认领并成功");
    assert_eq!(out["content"], json!("hello fs"));
    assert_eq!(out["path"], json!("notes.txt"));
}

// --- 2) `web.fetch` 精确名路由 ---

#[test]
fn web_fetch精确名路由到抓取适配器() {
    let url = "https://example.test/page";
    let rt = runtime_with_gate(gate_allowing(&["web.fetch"]));
    let (_dir, s) = workspace_session(&rt);

    let executor = install(&rt, s, Some(MockPort::with_page(url, "正文内容")));
    let out = executor
        .execute("web.fetch", &json!({ "url": url }))
        .expect("web.fetch 必须被认领并成功");
    assert_eq!(out["text"], json!("正文内容"));
    assert_eq!(out["title"], json!("本地测试页"));
}

// --- 3) 负例：未注册工具仍报 UNMATCHED ---

#[test]
fn 未注册工具仍报未认领() {
    let rt = runtime_with_gate(gate_allowing(&["fs.read", "web.fetch"]));
    let (_dir, s) = workspace_session(&rt);
    let executor = install(
        &rt,
        s,
        Some(MockPort::with_page("https://example.test/p", "x")),
    );

    // 没有任何绑定键认领的名字 ⇒ 明确报未认领（分派器没有变成「什么都认领」）。
    for name in [
        "fsx.read",
        "web.other",
        "web.fetchx",
        "taskx.list",
        "shell.exec2",
    ] {
        let err = executor
            .execute(name, &json!({}))
            .expect_err(&format!("{name} 必须未被认领"));
        assert!(
            err.contains(DispatchToolExecutor::UNMATCHED),
            "{name} 应报未认领（不是被某个执行器吞掉）: {err}"
        );
    }

    // 反面：`fs.` 前缀**认领**一切 `fs.` 起首的名字（哪怕没注册过）⇒ 走到门，
    // 由门按「工具未注册」拒掉——而不是落到未认领。
    let err = executor
        .execute("fs.nuke", &json!({}))
        .expect_err("未注册的 fs.* 必须被门拒掉");
    assert!(
        !err.contains(DispatchToolExecutor::UNMATCHED),
        "fs.nuke 已被 fs. 前缀认领，不该报未认领: {err}"
    );
    assert!(
        err.contains("未注册"),
        "fs.nuke 应由权限门按未注册拒绝: {err}"
    );
}

// --- 4) 四族一次装齐后各自可达 ---

#[test]
fn 四族一次装配后各自可达() {
    let url = "https://example.test/page";
    let rt = runtime_with_gate(gate_allowing(&[
        TASK_TOOL_PREFIX,
        EXEC_TOOL_NAME,
        "fs.read",
        "web.fetch",
    ]));
    let (dir, s) = workspace_session(&rt);
    std::fs::write(dir.path().join("a.txt"), "hi").expect("写临时文件");

    rt.install_all_tools(s, guarded(), Some(MockPort::with_page(url, "正文")))
        .expect("四族装配");
    let executor = rt.tool_executor().expect("执行器已装");

    // task.*：运行时语境外的 list 返回空列表（可读结果，不是未认领）。
    let tasks = executor
        .execute("task.list", &json!({}))
        .expect("task.list 必须被 task.* 认领");
    assert_eq!(tasks["count"], json!(0));

    // shell.exec：空策略 ⇒ NeedsApproval（**执行器层错误**，不是未认领）。
    let err = executor
        .execute("shell.exec", &json!({ "command": "echo hi" }))
        .expect_err("空策略必须转审批");
    assert!(
        !err.contains(DispatchToolExecutor::UNMATCHED),
        "shell.exec 必须由 exec 链认领: {err}"
    );
    assert!(err.contains("审批"), "错误应是待审批语义: {err}");

    // fs.*：真实读写。
    let out = executor
        .execute("fs.read", &json!({ "path": "a.txt" }))
        .expect("fs.read 必须被 fs. 认领");
    assert_eq!(out["content"], json!("hi"));

    // web.fetch：精确名命中。
    let out = executor
        .execute("web.fetch", &json!({ "url": url }))
        .expect("web.fetch 必须被精确名认领");
    assert_eq!(out["text"], json!("正文"));
}

// --- 5) 绑定表 ≡ 实际装配 ---

#[test]
fn 绑定顺序表与实际装配逐字一致() {
    let rt = runtime_with_gate(gate_allowing(&[]));
    let (_dir, s) = workspace_session(&rt);

    let dispatcher = rt
        .build_dispatcher(
            s,
            guarded(),
            Some(MockPort::with_page("https://example.test/p", "x")),
        )
        .expect("组装四族分派器");

    let table_keys: Vec<&str> = TOOL_BINDING_TABLE.iter().map(|(key, _)| *key).collect();
    assert_eq!(
        dispatcher.names(),
        table_keys,
        "实际装配的绑定名序列（含顺序）必须与绑定表逐字一致"
    );

    // 旧的两族视图必须与四族表的对应行**同源**（描述文本不能分叉）。
    let legacy = agent_core::executor::builtin_tool_bindings();
    assert_eq!(legacy.len(), 2, "旧视图只描述 install_builtin_tools 的两族");
    for (key, description) in &legacy {
        let row = TOOL_BINDING_TABLE
            .iter()
            .find(|(table_key, _)| table_key == key)
            .expect("旧视图的每一行都必须能在四族表里找到");
        assert_eq!(row.1, *description, "{key} 的描述文本必须与四族表同源");
    }
}

// --- 6) D6：`fs.write` 未授权被权限门拒绝 ---

#[test]
fn 未授权的fs_write被权限门拒绝() {
    let rt = runtime_with_gate(gate_allowing(&["fs.read"]));
    let (dir, s) = workspace_session(&rt);
    let executor = install(&rt, s, None);

    // **绕过运行时工具步**直接调执行器：门若不在这里生效，写入就会落盘。
    let err = executor
        .execute(
            "fs.write",
            &json!({ "path": "denied.txt", "content": "must not land" }),
        )
        .expect_err("未授权的写必须被拒");
    assert!(err.contains("权限门"), "错误必须点名权限门拒绝: {err}");
    assert!(
        !dir.path().join("denied.txt").exists(),
        "被拒的写入不得落盘"
    );
}

/// 门放行后路径越界仍由 `agent-tools` 的路径守卫兜住（放行 ≠ 越过工作区）。
#[test]
fn 门放行后越界写入仍被路径守卫拒绝() {
    let rt = runtime_with_gate(gate_allowing(&["fs.write"]));
    let (_dir, s) = workspace_session(&rt);
    let executor = install(&rt, s, None);

    let err = executor
        .execute(
            "fs.write",
            &json!({ "path": "../escape.txt", "content": "x" }),
        )
        .expect_err("越界写入必须被拒");
    assert!(err.contains("越权"), "错误应说明越权: {err}");
}

// --- 7) D5：工作区根缺失 ⇒ `fs.` 族不装 ---

#[test]
fn 工作区根为空时fs族不装() {
    let rt = runtime_with_gate(gate_allowing(&["fs.read"]));
    // 空工作区根 = 「没有配置安全边界」的那一态（`None` 会被运行时回落到 workdir）。
    let s = session(&rt, Some(String::new()));
    let executor = install(&rt, s, None);

    let err = executor
        .execute("fs.read", &json!({ "path": "a.txt" }))
        .expect_err("fs. 未装 ⇒ 必须未认领");
    assert!(
        err.contains(DispatchToolExecutor::UNMATCHED),
        "缺少工作区根时应明确报未认领: {err}"
    );
}

// --- 8) 精确名优先于前缀（route() 既有语义的回归守） ---

/// 前缀 `fs.` 与精确名 `fs.read` 同时绑定时，精确名那一方命中。
///
/// 用导出常量 `FS_TOOL_PREFIX` 造重叠对；本卡的四族表本身不重叠，这里守的是
/// [`DispatchToolExecutor::route`] 被本卡依赖时的那条既有语义。
#[test]
fn 精确名优先于前缀() {
    let dispatcher = DispatchToolExecutor::new()
        .bind(FS_TOOL_PREFIX, Arc::new(PrefixAwareExecutor("前缀")))
        .bind("fs.read", Arc::new(PrefixAwareExecutor("精确名")));
    let hit = |name: &str| dispatcher.execute(name, &json!({})).expect("必须命中");

    assert_eq!(hit("fs.read"), json!("精确名"), "精确名优先于前缀");
    assert_eq!(hit("fs.other"), json!("前缀"), "无精确名时前缀认领");
}

/// 固定应答执行器：只回报自己的标签。
struct PrefixAwareExecutor(&'static str);

impl ToolExecutor for PrefixAwareExecutor {
    fn execute(&self, _name: &str, _args: &Value) -> Result<Value, String> {
        Ok(json!(self.0))
    }
}

// --- 9) crate 本体可见性（核心验收句） ---

/// 两个适配器从 **crate 外部**直接引用 ⇒ 证明它不是测试影子副本。
#[test]
fn crate本体类型可从外部引用() {
    let dir = TempDir::new().expect("临时目录");
    let fs: Arc<dyn ToolExecutor + Send + Sync> = Arc::new(FsToolExecutor::new(dir.path()));
    let web: Arc<dyn ToolExecutor + Send + Sync> = Arc::new(WebFetchAdapter::new(
        MockPort::with_page("https://example.test/p", "x"),
    ));
    let _ = (fs, web);

    assert_eq!(FS_TOOL_NAMES.len(), 6, "六个 fs.* 共用一个适配器");
    // `tool_web` 也必须在 crate 本体里（`web_adapter` 的编译依赖）。
    assert_eq!(
        agent_core::tool_web::tool_specs()[0].0,
        WEB_FETCH_BINDING,
        "tool_web 与绑定键同源"
    );
}

// --- 端到端：模型请求 fs.write，门不放行 ⇒ 文件不落盘 ---

/// 第一次要 `fs.write`、之后收口的脚本化 provider。
struct ScriptedProvider {
    calls: AtomicUsize,
    path: String,
    content: String,
}

impl Provider for ScriptedProvider {
    fn next_steps(
        &self,
        _session_id: Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(if first {
            vec![ProviderStep::ToolCall {
                name: "fs.write".into(),
                args: json!({ "path": self.path, "content": self.content }),
            }]
        } else {
            vec![ProviderStep::Text("完成".into()), ProviderStep::Finished]
        })
    }
}

/// 跑一回「模型请求 `fs.write`」的完整回合；返回 `(工作区, 会话, 运行时)`。
fn run_turn_writing_fs(
    gate: PermissionGate,
    path: &str,
    content: &str,
) -> (TempDir, Uuid, Runtime) {
    let dir = TempDir::new().expect("临时目录");
    let provider = Arc::new(ScriptedProvider {
        calls: AtomicUsize::new(0),
        path: path.to_string(),
        content: content.to_string(),
    });
    let rt = Runtime::builder()
        .provider(provider)
        .gate(gate)
        .build()
        .expect("Runtime 必须可构造");
    let s = session(&rt, Some(dir.path().display().to_string()));
    install(&rt, s, None);

    rt.send_text(s, "写个文件").expect("回合应正常收口");
    (dir, s, rt)
}

#[test]
fn 端到端未授权的fs_write不落盘() {
    // `fs.write` 不在白名单 ⇒ deny-by-default。
    let (dir, s, rt) = run_turn_writing_fs(gate_allowing(&["fs.read"]), "e2e.txt", "x");
    assert!(
        !dir.path().join("e2e.txt").exists(),
        "权限门拒绝后文件不得落盘"
    );

    // 结算面必须是「denied」，不是执行器的真实结果。
    let denied = rt
        .get_session(s)
        .expect("session")
        .messages
        .into_iter()
        .filter(|m| m.role == Role::ToolResult)
        .find_map(|m| serde_json::from_str::<Value>(&m.content).ok())
        .expect("应有工具结果");
    assert_eq!(denied["status"], json!("denied"), "工具结果口径: {denied}");
}

/// 对照：同一链路里门放行时文件真的落盘（证明上一条不是「路径写错」造成的假阴性）。
#[test]
fn 端到端已授权的fs_write落盘() {
    let (dir, _s, _rt) = run_turn_writing_fs(gate_allowing(&["fs.write"]), "ok.txt", "landed");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ok.txt")).expect("读回"),
        "landed"
    );
}
