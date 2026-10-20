//! `mcp.json` 加载器验收：路径约定、三条强校验、合并语义与
//! `StdioServerCommand -> ClientConfig` 接缝。
//!
//! 参考规格：reference-docs 18 册 §9.2（原文亦见
//! `crates/agent-prompts/assets/safety-rules.md`）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agent_mcp::client::{
    default_config_path, load_from, merge_into, McpConfigError, StdioServerCommand,
};
use agent_mcp::ClientConfig;

/// 每个用例独立的临时目录；测试结束即删除。
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-mcp-json-loader-{}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, text).expect("写入夹具");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cmd(program: &str, args: &[&str], env: &[(&str, &str)]) -> StdioServerCommand {
    let mut command = StdioServerCommand::new(program).with_args(args.iter().copied());
    for (key, value) in env {
        command = command.with_env(*key, *value);
    }
    command
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).expect("读回配置");
    serde_json::from_str(&text).expect("读回内容是合法 JSON")
}

#[test]
fn loads_valid_file_with_all_stdio_fields() {
    let dir = TempDir::new();
    let path = dir.write(
        "mcp.json",
        r#"{
          "mcpServers": {
            "playwright": {
              "command": "npx",
              "args": ["@playwright/mcp@latest", "--headless"],
              "env": {"BROWSER": "chromium"}
            },
            "filesystem": {"command": "mcp-fs", "args": ["/tmp"]}
          }
        }"#,
    );

    let loaded = load_from(&path).expect("合法配置必须加载成功");

    assert_eq!(loaded.servers().len(), 2);
    assert_eq!(
        loaded.servers()["playwright"],
        cmd(
            "npx",
            &["@playwright/mcp@latest", "--headless"],
            &[("BROWSER", "chromium")]
        )
    );
    assert_eq!(
        loaded.servers()["filesystem"],
        cmd("mcp-fs", &["/tmp"], &[])
    );
}

#[test]
fn stdio_command_converts_to_client_config() {
    let command = cmd(
        "npx",
        &["@playwright/mcp@latest"],
        &[("BROWSER", "chromium")],
    );

    let config = ClientConfig::from(command.clone());

    assert_eq!(config.command, command);
    // 接缝只改 command；其余字段保持 ClientConfig::default() 的契约。
    let default = ClientConfig::default();
    assert_eq!(config.framing, default.framing);
    assert_eq!(config.request_timeout, default.request_timeout);
    assert_eq!(config.handshake_timeout, default.handshake_timeout);
    assert_eq!(config.max_frame_bytes, default.max_frame_bytes);
    assert_eq!(config.protocol_versions, default.protocol_versions);
    assert_eq!(config.notification_capacity, default.notification_capacity);
}

#[test]
fn invalid_json_is_an_explicit_error_not_an_empty_table() {
    let dir = TempDir::new();
    let path = dir.write("mcp.json", r#"{"mcpServers": {"broken": }"#);

    let error = load_from(&path).expect_err("非法 JSON 必须报错");

    assert!(matches!(error, McpConfigError::InvalidJson { .. }));
    assert!(!error.to_string().is_empty());
}

#[test]
fn mcp_servers_must_be_an_object() {
    let dir = TempDir::new();
    for body in [
        r#"{"mcpServers": []}"#,
        r#"{"mcpServers": "not-an-object"}"#,
        r#"{"mcpServers": null}"#,
    ] {
        let path = dir.write("mcp.json", body);
        let error = load_from(&path).expect_err("mcpServers 非对象必须报错");
        assert!(
            matches!(error, McpConfigError::McpServersNotObject),
            "body={body} got={error:?}"
        );
    }
}

#[test]
fn root_must_be_an_object() {
    let dir = TempDir::new();
    let path = dir.write("mcp.json", "[]");

    let error = load_from(&path).expect_err("顶层数组不是配置对象");

    assert!(matches!(error, McpConfigError::RootNotObject));
}

#[test]
fn empty_or_whitespace_server_name_is_rejected() {
    let dir = TempDir::new();
    // JSON 字符串里用转义序列表示制表符/换行；解析后仍是全空白名称。
    for name in ["", "   ", "\\t\\n"] {
        let path = dir.write(
            "mcp.json",
            &format!(r#"{{"mcpServers": {{"{name}": {{"command": "x"}}}}}}"#),
        );
        let error = load_from(&path).expect_err("空 server 名必须报错");
        assert!(matches!(error, McpConfigError::EmptyServerName));
    }
}

#[test]
fn non_stdio_server_entry_is_rejected_explicitly() {
    let dir = TempDir::new();
    let path = dir.write(
        "mcp.json",
        r#"{"mcpServers": {"remote": {"url": "https://example.invalid/mcp"}}}"#,
    );

    let error = load_from(&path).expect_err("本卡只覆盖 stdio");

    assert!(matches!(error, McpConfigError::InvalidServer { .. }));
    assert!(!error.to_string().is_empty());
}

#[test]
fn merge_reads_existing_file_and_preserves_other_servers() {
    let dir = TempDir::new();
    let path = dir.write(
        "mcp.json",
        r#"{
          "mcpServers": {
            "keep": {"command": "keep-server", "args": ["--flag"], "disabled": true},
            "replace": {"command": "old"}
          },
          "otherTopLevel": {"keep": 1}
        }"#,
    );

    let mut incoming = BTreeMap::new();
    incoming.insert("replace".to_string(), cmd("new", &["--new"], &[("K", "V")]));
    incoming.insert("added".to_string(), cmd("added-server", &[], &[]));

    merge_into(&path, &incoming).expect("合并必须成功");

    let loaded = load_from(&path).expect("合并后仍是可加载配置");
    assert_eq!(loaded.servers().len(), 3);
    assert_eq!(
        loaded.servers()["keep"],
        cmd("keep-server", &["--flag"], &[])
    );
    assert_eq!(
        loaded.servers()["replace"],
        cmd("new", &["--new"], &[("K", "V")])
    );
    assert_eq!(loaded.servers()["added"], cmd("added-server", &[], &[]));

    // 未触及的 server 保留原始字段，顶层其他键也不丢。
    let raw = read_json(&path);
    assert_eq!(raw["mcpServers"]["keep"]["disabled"], true);
    assert_eq!(raw["mcpServers"]["keep"]["args"][0], "--flag");
    assert_eq!(raw["otherTopLevel"]["keep"], 1);
}

#[test]
fn merge_creates_file_when_absent() {
    let dir = TempDir::new();
    let path = dir.path("mcp.json");

    let mut incoming = BTreeMap::new();
    incoming.insert("first".to_string(), cmd("first-server", &["a"], &[]));

    merge_into(&path, &incoming).expect("文件不存在时应新建");

    let loaded = load_from(&path).expect("新建文件必须可加载");
    assert_eq!(loaded.servers().len(), 1);
    assert_eq!(loaded.servers()["first"], cmd("first-server", &["a"], &[]));
}

#[test]
fn merge_does_not_clobber_invalid_existing_file() {
    let dir = TempDir::new();
    let original = r#"{"mcpServers": {"broken": }"#;
    let path = dir.write("mcp.json", original);

    let mut incoming = BTreeMap::new();
    incoming.insert("new".to_string(), cmd("new-server", &[], &[]));

    let error = merge_into(&path, &incoming).expect_err("现有文件非法时必须报错");
    assert!(matches!(error, McpConfigError::InvalidJson { .. }));
    assert_eq!(
        std::fs::read_to_string(&path).expect("原文件仍在"),
        original,
        "合并失败不得改写原文件"
    );
}

#[test]
fn merge_rejects_empty_server_name_before_touching_disk() {
    let dir = TempDir::new();
    let original = r#"{"mcpServers": {"keep": {"command": "keep"}}}"#;
    let path = dir.write("mcp.json", original);

    let mut incoming = BTreeMap::new();
    incoming.insert("  ".to_string(), cmd("x", &[], &[]));

    let error = merge_into(&path, &incoming).expect_err("空 server 名必须报错");
    assert!(matches!(error, McpConfigError::EmptyServerName));
    assert_eq!(
        std::fs::read_to_string(&path).expect("原文件仍在"),
        original
    );
}

#[test]
fn load_missing_file_is_an_error_not_an_empty_table() {
    let dir = TempDir::new();
    let path = dir.path("missing.json");

    let error = load_from(&path).expect_err("缺失文件必须显式报错");

    assert!(matches!(error, McpConfigError::Io { .. }));
}

#[test]
fn empty_config_object_loads_as_no_servers() {
    let dir = TempDir::new();
    let path = dir.write("mcp.json", "{}");

    let loaded = load_from(&path).expect("合法 JSON 且无 mcpServers 时为空表");

    assert!(loaded.is_empty());
    assert_eq!(loaded.len(), 0);
}

#[test]
fn default_config_path_has_no_dot_prefix() {
    let data_root = Path::new("/var/lib/lengsuo-agent");

    let path = default_config_path(data_root);

    assert_eq!(path, data_root.join("mcp.json"));
    let file_name = path.file_name().expect("有文件名").to_string_lossy();
    assert_eq!(file_name, "mcp.json");
    assert!(!file_name.starts_with('.'), "路径必须无点前缀");
}
