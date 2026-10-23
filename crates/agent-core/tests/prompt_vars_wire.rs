//! 提示词变量接线（x-prompt-vars-wire 卡）。
//!
//! 验收对象是**真实 Runtime 回合链**：`build_turn_context` 组装出的
//! `system_prompt`。变量取值必须来自运行时可观测的事实（身份目录 / product.json），
//! 不是独立调用 Composer 的单元测试。
//!
//! 本轮（第 1 轮）只写测试；实现见 `runtime/sessions/product_vars.rs`。

use std::sync::{Arc, Mutex};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode};

/// 捕获型 provider：每回合把 Runtime 实际组装的上下文留下。
struct Capturing {
    seen: Mutex<Vec<TurnContext>>,
}

impl Capturing {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
        })
    }

    fn take(&self) -> Vec<TurnContext> {
        std::mem::take(&mut *self.seen.lock().expect("捕获锁"))
    }
}

impl Provider for Capturing {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "Runtime 必须走带上下文的回合接口，不得回落到旧 next_steps",
        ))
    }

    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("捕获锁").push(context.clone());
        Ok(vec![
            ProviderStep::Text("ok".into()),
            ProviderStep::Finished,
        ])
    }
}

fn isolation() -> Isolation {
    Isolation {
        user_id: "u".into(),
        tenant_id: "t".into(),
        env_id: None,
    }
}

/// 起一个真实 Runtime（独立临时工作区，互不污染）。
fn runtime(tag: &str) -> (Arc<Capturing>, Runtime) {
    let dir = std::env::temp_dir().join(format!("prompt-vars-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let capture = Capturing::new();
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(capture.clone())
        .config(RuntimeConfig::default_v0())
        .build()
        .expect("runtime");
    (capture, rt)
}

fn session(rt: &Runtime) -> agent_core::schema::Session {
    rt.create_session(&CreateSession {
        title: "prompt-vars".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Craft,
        permission_mode: PermissionMode::ReadOnly,
        env_binding: None,
        isolation: isolation(),
    })
    .expect("create session")
}

/// 跑一回合，取 Runtime 实际送给 Provider 的 system prompt。
fn prompt_after_turn(rt: &Runtime, capture: &Capturing, text: &str) -> String {
    let s = session(rt);
    rt.send_text(s.id, text).expect("turn");
    capture.take().remove(0).system_prompt
}

/// 写产品配置：`<home>/product.json`（home = `<workdir>/.lengsuo`）。
fn write_product(rt: &Runtime, body: &str) {
    let home = rt.workdir().join(".lengsuo");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(home.join("product.json"), body).expect("product.json");
}

/// 写身份文件：`<home>/identity/<name>`（复用 agent-memory 的身份目录口径）。
fn write_identity_file(rt: &Runtime, name: &str, body: &str) {
    let dir = rt.workdir().join(".lengsuo").join("identity");
    std::fs::create_dir_all(&dir).expect("identity dir");
    std::fs::write(dir.join(name), body).expect("identity file");
}

/// 1. 字段全覆盖：`prompt_vars()` 里 `PromptVars` 的**每个字段**都必须显式赋值。
///
/// 用源码文本扫描而非锁字段总数（仓内共识：只守关系，不锁数量）：
/// - 字段清单从 `agent-prompts` 的**结构体定义**里抽（不是 `PROMPT_VAR_KEYS`——
///   键表与字段表是两回事，新增字段漏挂键时键表不会变）；
/// - 匹配按「整行字段名 + 冒号」，避免 `user:` 被 `memory_user:` 顶替；
/// - 并以「不得有 `..PromptVars::default()`」兜住「漏字段被默认值掩盖」。
#[test]
fn prompt_vars_explicitly_assigns_every_field() {
    const CONTEXT_SRC: &str = include_str!("../src/runtime/context.rs");
    const MODEL_SRC: &str = include_str!("../../agent-prompts/src/model.rs");

    let fields = prompt_vars_fields(MODEL_SRC);
    assert!(
        !fields.is_empty(),
        "必须能从 model.rs 抽出 PromptVars 的字段清单"
    );

    let start = CONTEXT_SRC
        .find("fn prompt_vars(")
        .expect("context.rs 必须保留 prompt_vars()");
    let rest = &CONTEXT_SRC[start..];
    let end = rest
        .find("fn expert_persona(")
        .expect("prompt_vars() 之后必须紧跟 expert_persona()");
    let body = &rest[..end];

    assert!(
        !body.contains("..PromptVars::default()"),
        "prompt_vars() 不得再用 ..PromptVars::default() 掩盖未接线字段"
    );
    for field in &fields {
        assert!(
            body.contains(&format!("\n            {field}:")),
            "PromptVars 字段 `{field}` 未在 prompt_vars() 中显式赋值"
        );
    }

    // 键表与字段表必须一致：键表漏挂字段时，模板侧会静默拿不到该变量。
    for key in agent_prompts::PROMPT_VAR_KEYS {
        assert!(
            fields.iter().any(|field| field == key),
            "PROMPT_VAR_KEYS 的 `{key}` 在 PromptVars 里没有对应字段"
        );
    }
}

/// 从 `model.rs` 的 `PromptVars` 结构体定义里抽出字段名。
///
/// 只认「行首缩进 + `pub <名字>:<类型>,`」的字段声明行；文档注释与 `pub struct` 头跳过。
fn prompt_vars_fields(model_src: &str) -> Vec<String> {
    let start = model_src
        .find("pub struct PromptVars {")
        .expect("model.rs 必须定义 PromptVars");
    let rest = &model_src[start..];
    let end = rest.find("\n}").expect("PromptVars 结构体必须闭合");
    rest[..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            let name = line.strip_prefix("pub ")?;
            // `pub name: Type` —— 冒号前是字段名，其后是类型。
            let name = name.split(':').next()?.trim();
            (!name.is_empty() && !name.contains(' ')).then(|| name.to_string())
        })
        .collect()
}

/// 2. `product_name` 有值 ⇒ 渲染「## 产品身份」段。
#[test]
fn product_name_renders_product_identity_section() {
    let (capture, rt) = runtime("product");
    write_product(&rt, r#"{"product_name": "澜枢平台"}"#);

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("## 产品身份"),
        "product_name 非空时产品身份段必须渲染:\n{prompt}"
    );
    assert!(prompt.contains("澜枢平台"), "产品名未注入:\n{prompt}");
}

/// 3. `product_name` 为空 ⇒ 产品身份段整段省略（`if_filled` 既有语义）。
#[test]
fn empty_product_name_omits_product_identity_section() {
    let (capture, rt) = runtime("empty-product");
    write_product(&rt, r#"{"product_name": ""}"#);

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        !prompt.contains("## 产品身份"),
        "product_name 为空时不得留空洞标题:\n{prompt}"
    );
}

/// 4. `is_windows = true` ⇒ 渲染 Windows 条件段；false ⇒ 不渲染。
///
/// 来源是显式配置字段（product.json），**不是** `cfg!(windows)` /
/// `std::env::consts::OS`——研发机是 Linux，编译期判定会让本段永远不渲染。
#[test]
fn is_windows_config_renders_windows_section() {
    let (capture, rt) = runtime("windows");
    write_product(&rt, r#"{"is_windows": true}"#);

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("## Windows 命令安全"),
        "is_windows=true 时 Windows 条件段必须渲染:\n{prompt}"
    );

    let (capture_off, rt_off) = runtime("not-windows");
    write_product(&rt_off, r#"{"is_windows": false}"#);
    let prompt_off = prompt_after_turn(&rt_off, &capture_off, "你好");
    assert!(
        !prompt_off.contains("## Windows 命令安全"),
        "is_windows=false 时 Windows 段不得渲染:\n{prompt_off}"
    );
}

/// 5. 从文件读来的正文含凭据样式串 ⇒ 进变量的明文被脱敏。
#[test]
fn credential_style_text_is_redacted_in_product_vars() {
    let (capture, rt) = runtime("redact");
    write_product(&rt, r#"{"product_name": "澜枢 token=supersecretvalue123"}"#);

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        !prompt.contains("supersecretvalue123"),
        "凭据明文不得进 system prompt:\n{prompt}"
    );
    assert!(prompt.contains("token=***"), "脱敏占位缺失:\n{prompt}");
}

/// 6. 缺 `product.json` ⇒ 显式降级为空串，不 panic、不报错。
#[test]
fn missing_product_json_degrades_to_empty_without_panic() {
    let (capture, rt) = runtime("missing-product");
    let s = session(&rt);
    let receipt = rt
        .send_text(s.id, "你好")
        .expect("缺 product.json 不得阻断回合");
    assert!(receipt.last_seq > 0, "回合必须照常产出事件");

    let prompt = capture.take().remove(0).system_prompt;
    assert!(
        !prompt.contains("## 产品身份"),
        "缺 product.json ⇒ product_name 空 ⇒ 段省略:\n{prompt}"
    );
    assert!(
        !prompt.contains("## Windows 命令安全"),
        "缺 product.json ⇒ is_windows=false ⇒ Windows 段省略:\n{prompt}"
    );
}

/// 7. 身份四文件落在 agent-memory 既有的身份目录口径
///    （`<home>/identity`，`MemoryPaths::identity_dir()`），不另造目录约定。
///
/// 两条断言各有分工：
/// - **目录口径**：四文件写在 `MemoryPaths::identity_dir()` 下 ⇒ 运行时读得到
///   （读不到的话 SOUL 段会是空的，`<identity_context>` 里的正文也是空的）；
/// - **`identity_mode` 由文件事实推导 ⇒ 身份段必渲染**。
///
/// ⚠️ 口径变更（`x-prompt-config-wire` 卡）：本文件原先断言
/// 「`identity_mode` 未声明（空串）⇒ 身份段不渲染」（D3 定死空串）。该口径已被
/// 参考端实物推翻 —— 其 `IdentityCollector` 里
/// `WorkspaceIdentityMode = fileExists(BOOTSTRAP.md) ? "onboarding" : "normal"`，
/// **正常路径永远二选一**，只有抛异常才是空串。故此处改为断言「必渲染」，
/// 并在写了 BOOTSTRAP.md 的前提下断言走 onboarding 分支。
///
/// 身份**正文与路径**本身另由 `product_vars.rs` 的单元测试断言。
#[test]
fn identity_files_live_in_memory_identity_dir() {
    let (capture, rt) = runtime("identity");
    write_identity_file(&rt, "SOUL.md", "人格正文");
    write_identity_file(&rt, "IDENTITY.md", "身份正文");
    write_identity_file(&rt, "USER.md", "用户画像正文");
    write_identity_file(&rt, "BOOTSTRAP.md", "引导正文");

    let paths = agent_memory::MemoryPaths::new(rt.workdir().join(".lengsuo"), rt.workdir());
    for name in ["SOUL.md", "IDENTITY.md", "USER.md", "BOOTSTRAP.md"] {
        assert!(
            paths.identity_dir().join(name).is_file(),
            "身份文件必须落在 MemoryPaths::identity_dir(): {name}"
        );
    }

    // SOUL 正文走既有 `read_identity`（不经 identity_mode）⇒ 必进 prompt。
    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("人格正文"),
        "SOUL.md 正文必须经既有身份层读取进入提示词:\n{prompt}"
    );
    assert!(
        prompt.contains("<identity_context>"),
        "identity_mode 由 BOOTSTRAP.md 存在性推导（onboarding/normal）⇒ 身份段必渲染:\n{prompt}"
    );
    assert!(
        prompt.contains("（onboarding）"),
        "写了 BOOTSTRAP.md ⇒ 必须走 onboarding 分支:\n{prompt}"
    );
}
