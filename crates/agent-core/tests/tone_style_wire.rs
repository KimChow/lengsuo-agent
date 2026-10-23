//! 语气风格接线（x-tone-style-wire 卡）。
//!
//! 验收对象是**真实 Runtime 回合链**：`build_turn_context` 组装出的
//! `system_prompt`。`toneStyle` 从 `config:set` 进入配置，再经
//! `style::body_of` 归一化为正文，由段模板包上 `<tone_and_style>` 标签。

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
    let dir = std::env::temp_dir().join(format!("tone-style-{tag}-{}", uuid::Uuid::new_v4()));
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
        title: "tone-style".into(),
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

/// a. 默认配置 ⇒ 风格段整段省略。
#[test]
fn default_config_omits_tone_style_block() {
    let (capture, rt) = runtime("default");
    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(
        !prompt.contains("<tone_and_style>"),
        "默认配置下不得渲染风格段:\n{prompt}"
    );
    assert!(!prompt.contains("</tone_and_style>"));
}

/// b. 枚举名 ⇒ 渲染正文，且只有一层标签（防 `<tone_and_style>` 双层包裹）。
#[test]
fn professional_style_renders_single_block() {
    let (capture, rt) = runtime("professional");
    rt.config_set(&json!({ "toneStyle": "professional" }))
        .expect("set toneStyle");

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(prompt.contains("<tone_and_style>"), "缺段头:\n{prompt}");
    assert!(
        prompt.contains("用语准确、结构清晰"),
        "枚举名未归一化为正文:\n{prompt}"
    );
    assert!(prompt.contains("</tone_and_style>"), "缺段尾:\n{prompt}");
    assert_eq!(
        prompt.matches("<tone_and_style>").count(),
        1,
        "段模板已自带标签，正文不得再含标签（双层包裹）:\n{prompt}"
    );
}

/// c. 枚举名大小写不敏感。
#[test]
fn enum_name_is_case_insensitive() {
    let (capture, rt) = runtime("efficient");
    rt.config_set(&json!({ "toneStyle": "Efficient" }))
        .expect("set toneStyle");

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(prompt.contains("直入主题"), "大小写归一化失效:\n{prompt}");
}

/// d. 自由文本原样作正文（不校验成枚举）。
#[test]
fn free_text_is_used_verbatim() {
    let (capture, rt) = runtime("free-text");
    rt.config_set(&json!({ "toneStyle": "说话简短" }))
        .expect("set toneStyle");

    let prompt = prompt_after_turn(&rt, &capture, "你好");
    assert!(prompt.contains("说话简短"), "自由文本未进提示词:\n{prompt}");
}

/// e. 非法值 ⇒ `InvalidConfig`，且失败整体不改现值。
#[test]
fn invalid_tone_style_is_rejected_and_preserves_previous_value() {
    let (_capture, rt) = runtime("invalid");
    rt.config_set(&json!({ "toneStyle": "professional" }))
        .expect("set toneStyle");

    let err = rt
        .config_set(&json!({ "toneStyle": 123 }))
        .expect_err("非字符串拒绝");
    assert!(matches!(err, CoreError::InvalidConfig(_)));
    assert_eq!(
        rt.config_get().tone_style,
        "professional",
        "失败的 set 不得改动现值"
    );
}

/// f. 序列化往返：`toValue` 含 `toneStyle`，反序列化回同值；缺键回落空串。
#[test]
fn tone_style_serializes_and_deserializes() {
    let mut config = RuntimeConfig::default_v0();
    config.tone_style = "professional".into();

    let value = config.to_value();
    assert_eq!(value["toneStyle"], "professional");
    assert!(value.get("tone_style").is_none(), "字段名冻结为 camelCase");

    let back: RuntimeConfig = serde_json::from_value(value.clone()).expect("反序列化");
    assert_eq!(back, config);
    assert_eq!(back.tone_style, "professional");

    // 旧载荷（无该键）⇒ 回落默认空串。
    let mut bare = value;
    bare.as_object_mut().expect("对象").remove("toneStyle");
    let decoded: RuntimeConfig = serde_json::from_value(bare).expect("缺字段可反序列化");
    assert_eq!(decoded.tone_style, "");
}
