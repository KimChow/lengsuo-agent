//! 共享夹具：最小合法专家包（可改坏）、捕获型 provider、建会话 / 走回合。
//!
//! 未在某一组用到的辅助项允许 `dead_code`（各组是同一 crate 的不同用例组）。

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::{Provider, ProviderStep, TurnContext};
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode, Session};
use uuid::Uuid;

/// 最小合法专家包（manifest 字段齐全；`name` 与目录名一致）。
fn plugin_json(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "version": "1.0.0",
  "description": "Test expert package",
  "agents": ["./agents/{name}.md"],
  "expertType": "agent",
  "agentName": "{name}",
  "displayName": {{ "en": "{name}", "zh": "{name}" }},
  "profession": {{ "en": "Tester", "zh": "测试员" }},
  "displayDescription": {{
    "en": "A deterministic expert package used by session binding tests only",
    "zh": "仅供会话绑定测试使用的一份确定性专家包，人格正文固定，不依赖外部资源文件"
  }},
  "categoryId": "02-Engineering",
  "defaultInitPrompt": {{ "zh": "开始", "en": "Start" }},
  "tags": [
    {{ "zh": "标签一", "en": "Tag one" }},
    {{ "zh": "标签二", "en": "Tag two" }},
    {{ "zh": "标签三", "en": "Tag three" }}
  ],
  "quickPrompts": [
    {{ "zh": "开始", "en": "Start" }},
    {{ "zh": "继续", "en": "Continue" }},
    {{ "zh": "结束", "en": "Stop" }}
  ]
}}"#
    )
}

/// 用例侧需要直接改 manifest 时取到的原始 JSON（与 [`write_expert`] 写盘内容一致）。
pub fn plugin_json_for_test(name: &str) -> String {
    plugin_json(name)
}

/// 写一个专家包到 `<root>/<name>/`，人格正文为 `persona`。
pub fn write_expert(root: &Path, name: &str, persona: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(".lengsuo-plugin")).expect("plugin dir");
    fs::create_dir_all(dir.join("agents")).expect("agents dir");
    fs::write(dir.join(".lengsuo-plugin/plugin.json"), plugin_json(name)).expect("manifest");
    fs::write(
        dir.join(format!("agents/{name}.md")),
        format!("---\nname: {name}\n---\n{persona}\n"),
    )
    .expect("persona");
    dir
}

/// 测试工作区：此时**用户级根已重定向**到临时 home（见 `main.rs` 的 `HOME` 隔离）。
pub fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("expert-bind-{tag}-{}", Uuid::new_v4()));
    fs::create_dir_all(dir.join(".lengsuo/experts")).expect("experts root");
    dir
}

/// 计数工具：断言「人格注入段只有一处」时数正文出现次数。
pub fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// 写一个**损坏**的包（manifest 不是合法 JSON）到 `<root>/<name>/`。
pub fn write_broken_expert(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(dir.join(".lengsuo-plugin")).expect("plugin dir");
    fs::write(dir.join(".lengsuo-plugin/plugin.json"), "{ not json").expect("坏 manifest");
    dir
}

pub fn projects_root(ws: &Path) -> PathBuf {
    ws.join(".lengsuo/experts")
}

/// 捕获型 provider：两轮回显，记录每轮回合上下文。
#[derive(Default)]
pub struct Capture {
    seen: Mutex<Vec<TurnContext>>,
}

impl Capture {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 供注入 Runtime 用的 trait 对象形态。
    pub fn dyn_capture(self: &Arc<Self>) -> Arc<dyn Provider> {
        self.clone()
    }

    pub fn take(&self) -> Vec<TurnContext> {
        std::mem::take(&mut *self.seen.lock().expect("捕获锁"))
    }
}

impl Provider for Capture {
    fn next_steps(&self, _id: Uuid, user_text: &str) -> Result<Vec<ProviderStep>, CoreError> {
        Ok(vec![
            ProviderStep::Text(format!("收到：{user_text}")),
            ProviderStep::Finished,
        ])
    }

    fn next_steps_with_context(&self, ctx: &TurnContext) -> Result<Vec<ProviderStep>, CoreError> {
        self.seen.lock().expect("捕获锁").push(ctx.clone());
        self.next_steps(ctx.session_id, &ctx.user_text)
    }
}

/// 建 Runtime（用户级专家根已隔离到临时 home，见 `main.rs::isolate_home`）。
pub fn runtime(ws: &Path, provider: Arc<dyn Provider>) -> Runtime {
    crate::isolate_home();
    let mut config = RuntimeConfig::default_v0();
    config.memory_enabled = false;
    Runtime::builder()
        .workdir(ws)
        .provider(provider)
        .config(config)
        .build()
        .expect("runtime")
}

fn isolation() -> Isolation {
    Isolation {
        user_id: "u".into(),
        tenant_id: "t".into(),
        env_id: None,
    }
}

pub fn session(rt: &Runtime) -> Session {
    rt.create_session(&CreateSession {
        title: "召唤卡".into(),
        workspace_root: Some(rt.workdir().display().to_string()),
        mode: Mode::Craft,
        permission_mode: PermissionMode::ReadOnly,
        env_binding: None,
        isolation: isolation(),
    })
    .expect("create session")
}

/// 走一轮真实回合，返回该轮组装出的 system prompt。
pub fn turn(rt: &Runtime, cap: &Capture, id: Uuid, text: &str) -> String {
    rt.send_text(id, text).expect("turn");
    let seen = cap.take();
    assert_eq!(seen.len(), 1, "一轮 = 一次上下文组装");
    seen[0].system_prompt.clone()
}
