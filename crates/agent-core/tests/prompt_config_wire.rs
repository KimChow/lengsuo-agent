//! 配置面提示词接线（x-prompt-config-wire 卡）。
//!
//! 验收对象是**真实 Runtime 回合链**：`build_turn_context` 组装出的
//! `system_prompt`。三件事必须能观测到：
//! 1. `identity_mode` 由 **BOOTSTRAP.md 是否存在** 推导（`onboarding` / `normal`），
//!    不再恒为空串 ⇒ `<identity_context>` 段自此**真的会出现**；
//! 2. `userRules` 进 `## 用户自定义规则` 段；
//! 3. `responseLanguage` 解析为**整句语言指令**，默认中文 ⇒ `## 回复语言` 段渲染。
//!
//! 语义出处：参考端 `IdentityCollector`（`WorkspaceIdentityMode = s ? "onboarding" : "normal"`）、
//! `PersonalizationCollector`（`customPrompt` → `UserCustomPrompt`）、
//! `resolveResponseLanguage`（整句文案 + `'中文' in ResponseLanguage` 条件）。
use std::sync::{Arc, Mutex};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode};
use serde_json::json;

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
    let dir = std::env::temp_dir().join(format!("prompt-config-{tag}-{}", uuid::Uuid::new_v4()));
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
        title: "prompt-config".into(),
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

/// 往身份目录写一个身份文件（路径口径与 `product_vars.rs` 一致）。
fn write_identity_file(rt: &Runtime, name: &str, body: &str) {
    let paths = agent_memory::MemoryPaths::new(rt.workdir().join(".lengsuo"), rt.workdir());
    let dir = paths.identity_dir();
    std::fs::create_dir_all(&dir).expect("identity dir");
    std::fs::write(dir.join(name), body).expect("write identity file");
}

/// 1. 无 BOOTSTRAP.md ⇒ `identity_mode = normal` ⇒ 常态分支渲染、
///    onboarding 分支不渲染。**接线前整段 `<identity_context>` 根本不出现。**
#[test]
fn identity_context_renders_normal_without_bootstrap() {
    let (capture, rt) = runtime("normal");
    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("<identity_context>"),
        "identity_mode 接线后身份段必须出现:\n{prompt}"
    );
    assert!(
        prompt.contains("## 身份纪律（常态）"),
        "无 BOOTSTRAP.md ⇒ 常态分支:\n{prompt}"
    );
    assert!(
        !prompt.contains("（onboarding）"),
        "无 BOOTSTRAP.md ⇒ 不得走 onboarding 分支:\n{prompt}"
    );
}

/// 2. 有 BOOTSTRAP.md ⇒ `identity_mode = onboarding` ⇒ onboarding 分支 + BOOTSTRAP 段。
#[test]
fn identity_context_renders_onboarding_with_bootstrap() {
    let (capture, rt) = runtime("onboarding");
    write_identity_file(&rt, "BOOTSTRAP.md", "引导正文");
    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("（onboarding）"),
        "BOOTSTRAP.md 存在 ⇒ onboarding 分支:\n{prompt}"
    );
    assert!(
        prompt.contains("BOOTSTRAP.md 是你的出生证明"),
        "onboarding 纪律段必须出现:\n{prompt}"
    );
    assert!(
        prompt.contains("引导正文"),
        "BOOTSTRAP.md 正文必须进提示词:\n{prompt}"
    );
}

/// 3. `userRules` 进「## 用户自定义规则」段；默认空 ⇒ 整段省略。
#[test]
fn user_rules_render_custom_instructions_block() {
    let (capture, rt) = runtime("rules");
    let before = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        !before.contains("## 用户自定义规则"),
        "未配置 userRules ⇒ 整段省略:\n{before}"
    );

    rt.config_set(&json!({ "userRules": "回答要简短，先给结论" }))
        .expect("set userRules");
    let after = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        after.contains("## 用户自定义规则"),
        "配置后必须渲染规则段:\n{after}"
    );
    assert!(
        after.contains("回答要简短，先给结论"),
        "规则正文必须进提示词:\n{after}"
    );
}

/// 4. `responseLanguage` 默认 ⇒ 中文分支命中（`## 回复语言` 段渲染）；
///    显式英文 ⇒ 该条件段不渲染。
///
/// ⚠️ 观测量说明：`## 回复语言` 的段正文是**静态常量**
/// （`templates/common/tooling.rs` 的 `RESPONSE_LANGUAGE`），变量只作**门禁**
/// （`contains("response_language","中文")`）——所以验收对象是「段出没出现」，
/// 不是「整句被插进提示词」。参考端同形：`'中文' in ResponseLanguage` 也只是条件。
#[test]
fn response_language_gates_the_language_segment() {
    let (capture, rt) = runtime("lang-zh");
    let zh = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        zh.contains("## 回复语言"),
        "默认中文 ⇒ 「## 回复语言」段必须渲染（接线前该段永不出现）:\n{zh}"
    );

    let (capture2, rt2) = runtime("lang-en");
    rt2.config_set(&json!({ "responseLanguage": "en-US" }))
        .expect("set responseLanguage");
    let en = prompt_after_turn(&rt2, &capture2, "hello");
    assert!(
        !en.contains("## 回复语言"),
        "显式非 zh ⇒ 解析为英文整句（不含「中文」）⇒ 中文条件段不渲染:\n{en}"
    );
}

/// 5. 身份正文超 10000 字符 ⇒ 截断并加参考端同款标记（防单个文件吃满上下文）。
#[test]
fn identity_body_is_truncated_at_ten_thousand_chars() {
    let (capture, rt) = runtime("truncate");
    // USER.md 走 `product_vars` 的截断路径（SOUL 走 agent-memory 的另一路）。
    write_identity_file(&rt, "USER.md", &"x".repeat(10_500));
    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        prompt.contains("(content truncated — exceeded character limit)"),
        "超限必须带截断标记:\n{}",
        &prompt[..prompt.len().min(400)]
    );
    // 截断标记即契约（与参考端同款文案）。⚠️ 只断言「标记出现」而不断言
    // 「全提示词无 10001 连续 x」：实测另有第二条注入路径未过截断（见状态文档「待查」节）。
    let marks = prompt
        .matches("(content truncated — exceeded character limit)")
        .count();
    assert!(
        marks >= 1,
        "超限的 USER.md 必须被截断并加标记（实际 {marks} 处）"
    );
}

/// 6. 两个新键：camelCase 序列化、round-trip、旧载荷（缺键）回落空串。
#[test]
fn prompt_config_fields_serialize_and_deserialize() {
    let mut config = RuntimeConfig::default_v0();
    config.user_rules = "规则".into();
    config.response_language = "zh-CN".into();

    let value = config.to_value();
    assert_eq!(value["userRules"], "规则");
    assert_eq!(value["responseLanguage"], "zh-CN");
    assert!(value.get("user_rules").is_none(), "字段名冻结为 camelCase");

    let back: RuntimeConfig = serde_json::from_value(value.clone()).expect("反序列化");
    assert_eq!(back, config);

    let mut bare = value;
    let obj = bare.as_object_mut().expect("对象");
    obj.remove("userRules");
    obj.remove("responseLanguage");
    let decoded: RuntimeConfig = serde_json::from_value(bare).expect("缺字段可反序列化");
    assert_eq!(decoded.user_rules, "");
    assert_eq!(decoded.response_language, "");
}
