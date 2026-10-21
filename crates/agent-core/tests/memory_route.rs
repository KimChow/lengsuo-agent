//! 记忆检索三工具走真实 Runtime 路由（x-memory-route）。
//!
//! 验收点：`execute_memory_tool` 不再对 `memory.list/search/read` 报「未知记忆工具」；
//! 越界路径显式报错；查询串走写路径同款脱敏。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::config::RuntimeConfig;
use agent_core::error::CoreError;
use agent_core::provider::EchoProvider;
use agent_core::runtime::{CreateSession, Runtime};
use agent_core::schema::{Isolation, Mode, PermissionMode, Session};
use serde_json::{json, Value};

fn rt(tag: &str) -> (Runtime, PathBuf) {
    let dir = std::env::temp_dir().join(format!("x-memory-route-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workdir");
    let mut cfg = RuntimeConfig::default_v0();
    cfg.memory_enabled = true;
    let rt = Runtime::builder()
        .workdir(&dir)
        .provider(Arc::new(EchoProvider))
        .config(cfg)
        .build()
        .expect("runtime");
    (rt, dir)
}

fn mk_session(rt: &Runtime, ws: &Path) -> Session {
    rt.create_session(&CreateSession {
        title: "t".into(),
        workspace_root: Some(ws.display().to_string()),
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
}

/// 在项目记忆根（`<ws>/.lengsuo/memory`）落一个文件。
fn seed(ws: &Path, name: &str, content: &str) {
    let root = ws.join(".lengsuo").join("memory");
    std::fs::create_dir_all(&root).expect("memory root");
    std::fs::write(root.join(name), content).expect("seed");
}

fn exec(rt: &Runtime, session: &Session, name: &str, args: Value) -> Result<Value, CoreError> {
    rt.execute_memory_tool(session.id, name, &args)
}

#[test]
fn list_returns_entries_from_project_root() {
    let (rt, ws) = rt("list");
    seed(&ws, "MEMORY.md", "alpha\n");
    let s = mk_session(&rt, &ws);

    let result = exec(&rt, &s, "memory.list", json!({})).expect("memory.list 必须走通");
    let entries = result["entries"].as_array().expect("entries 数组");
    assert_eq!(entries.len(), 1, "项目根下应只有一个记忆文件: {result}");
    assert_eq!(entries[0]["path"], "project:MEMORY.md");
    assert_eq!(entries[0]["entry_type"], "file");
    assert_eq!(result["truncated"], false);
}

#[test]
fn search_hits_seeded_content() {
    let (rt, ws) = rt("search");
    seed(&ws, "MEMORY.md", "first line\nneedle in haystack\n");
    let s = mk_session(&rt, &ws);

    let result = exec(&rt, &s, "memory.search", json!({ "queries": ["needle"] }))
        .expect("memory.search 必须走通");
    let matches = result["matches"].as_array().expect("matches 数组");
    assert_eq!(matches.len(), 1, "应命中一行: {result}");
    assert!(matches[0]["content"].as_str().unwrap().contains("needle"));
    assert_eq!(matches[0]["path"], "project:MEMORY.md");
    assert_eq!(matches[0]["matched_queries"][0], "needle");
}

#[test]
fn read_returns_file_content() {
    let (rt, ws) = rt("read");
    seed(&ws, "MEMORY.md", "line one\nline two\n");
    let s = mk_session(&rt, &ws);

    let result =
        exec(&rt, &s, "memory.read", json!({ "path": "MEMORY.md" })).expect("memory.read 必须走通");
    assert_eq!(result["path"], "project:MEMORY.md");
    assert_eq!(result["start_line_number"], 1);
    assert!(result["content"].as_str().unwrap().contains("line two"));
    assert_eq!(result["truncated"], false);
}

#[test]
fn read_escape_path_is_explicit_error() {
    let (rt, ws) = rt("escape");
    seed(&ws, "MEMORY.md", "alpha\n");
    let s = mk_session(&rt, &ws);

    for path in ["../outside.md", "user:../../outside.md", "/etc/passwd"] {
        let err = exec(&rt, &s, "memory.read", json!({ "path": path }))
            .expect_err("越界路径必须显式报错");
        match err {
            CoreError::InvalidConfig(msg) => {
                assert!(msg.contains("越出"), "越界错误须可辨识: {msg}");
            }
            other => panic!("越界应为 InvalidConfig(路径越出): {other:?}"),
        }
    }
}

#[test]
fn credential_style_query_is_redacted_in_response() {
    let (rt, ws) = rt("redact");
    seed(&ws, "MEMORY.md", "alpha\n");
    let s = mk_session(&rt, &ws);

    let secret = "sk-abcdefghijklmnopqrstuvwx";
    let result = exec(
        &rt,
        &s,
        "memory.search",
        json!({ "queries": [secret, "alpha"] }),
    )
    .expect("脱敏后仍应可检索");
    let rendered = serde_json::to_string(&result).expect("serialize");
    assert!(!rendered.contains(secret), "查询串明文不得回显: {rendered}");
    assert_eq!(result["queries"][0], "***", "查询串必须走 self.redact");
}
