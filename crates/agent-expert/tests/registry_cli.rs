//! 专家注册表对外操作测试（卡 8：init / register / unregister / export）。
//!
//! 临时目录用 `std::env::temp_dir()` + 进程/计数唯一名（本 crate 不在依赖白名单
//! 内加 `tempfile`，与 `tests/registry.rs` 一致）。
//!
//! zip 校验**不调外部 `unzip`、不解压到磁盘**：测试内直接读字节，解析 EOCD +
//! 中央目录 + 本地文件头，并独立实现 CRC-32 复核（避免与被测实现同源）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use agent_expert::paths;
use agent_expert::{
    export_expert, init_expert, register_expert, unregister_expert, CategoryId, ExpertDraft,
    ExpertError, ExpertManifest, ExpertRegistry, ExpertType, LocalizedText, Validator,
};

/// 临时根目录（Drop 时清理）。
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-expert-registry-cli-{}-{}-{n}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// 项目级专家根（`<tmp>/project/.lensuo/experts`）。
    fn experts_root(&self) -> PathBuf {
        self.0.join("project").join(paths::PROJECT_EXPERTS_DIR)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(zh: &str, en: &str) -> LocalizedText {
    LocalizedText {
        zh: zh.to_string(),
        en: en.to_string(),
    }
}

fn draft(name: &str) -> ExpertDraft {
    ExpertDraft {
        name: name.to_string(),
        display_name: text("演示专家", "Demo Expert"),
        version: "1.0.0".to_string(),
        expert_type: ExpertType::Agent,
        category_id: CategoryId::Engineering,
        description: "Demo expert used by the registry CLI tests".to_string(),
    }
}

/// 一份能通过校验器的最小单专家 manifest。
fn valid_manifest(name: &str, version: &str) -> ExpertManifest {
    let prompt = text(
        "帮我演示一下专家注册流程。",
        "Walk me through expert registration.",
    );
    ExpertManifest {
        name: name.to_string(),
        version: version.to_string(),
        description: "Demo expert used by the registry CLI tests".to_string(),
        author: None,
        homepage: None,
        license: None,
        keywords: Vec::new(),
        expert_type: ExpertType::Agent,
        agent_name: name.to_string(),
        team_info: None,
        agents: vec![format!("./{}/{name}.md", paths::AGENTS_DIR)],
        skills: Vec::new(),
        display_name: text("演示专家", "Demo Expert"),
        profession: text("演示工程师", "Demo Engineer"),
        display_description: text(
            "这是一个用于注册表操作测试的演示专家，覆盖初始化与校验注册导出全流程。",
            "A demo expert for registry operation tests.",
        ),
        avatar: None,
        category_id: CategoryId::Engineering,
        default_init_prompt: prompt.clone(),
        tags: vec![
            text("演示", "Demo"),
            text("注册表", "Registry"),
            text("导出", "Export"),
        ],
        quick_prompts: vec![
            prompt,
            text("演示第二条提示。", "Second demo prompt."),
            text("演示第三条提示。", "Third demo prompt."),
        ],
        plugin: None,
        members: Vec::new(),
        dependencies: None,
        persona: None,
        prompt_pair: None,
    }
}

/// 写 `plugin.json`（覆盖已有文件）。
fn write_manifest(dir: &Path, manifest: &ExpertManifest) {
    let plugin_dir = dir.join(paths::PLUGIN_DIR);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join(paths::MANIFEST_FILE),
        manifest.to_json_string().unwrap(),
    )
    .unwrap();
}

/// 写主 Agent 人格文件（frontmatter 无 tools、name == stem）。
fn write_agent(dir: &Path, name: &str) {
    let agents = dir.join(paths::AGENTS_DIR);
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(format!("{name}.md")),
        format!("---\nname: {name}\ndescription: 演示\n---\n\n正文\n"),
    )
    .unwrap();
}

/// 把包补成可校验通过的完整包（工作流第 3 步「生成内容」）。
fn complete_package(dir: &Path, name: &str, version: &str) {
    write_manifest(dir, &valid_manifest(name, version));
    write_agent(dir, name);
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[test]
fn init_creates_skeleton_and_serializes_manifest() {
    let tmp = TempRoot::new("init");
    let dir = tmp.experts_root().join("demo-expert");

    let summary = init_expert(&dir, &draft("demo-expert")).unwrap();
    assert_eq!(summary.id, "demo-expert");
    assert_eq!(summary.dir, dir);
    assert_eq!(summary.version, "1.0.0");
    assert_eq!(summary.expert_type, ExpertType::Agent);
    assert_eq!(summary.category_id, CategoryId::Engineering);

    for sub in [
        paths::PLUGIN_DIR,
        paths::AGENTS_DIR,
        paths::SKILLS_DIR,
        paths::AVATARS_DIR,
    ] {
        assert!(dir.join(sub).is_dir(), "缺少骨架目录 {sub}");
    }

    // `plugin.json` 可反序列化回 `ExpertManifest`，且由草稿派生主 Agent 路径。
    let raw = std::fs::read_to_string(dir.join(paths::manifest_rel_path())).unwrap();
    let manifest = ExpertManifest::from_json_str(&raw).unwrap();
    assert_eq!(manifest.name, "demo-expert");
    assert_eq!(manifest.agent_name, "demo-expert");
    assert_eq!(manifest.agents, vec!["./agents/demo-expert.md"]);
    assert_eq!(manifest.display_name.zh, "演示专家");
}

#[test]
fn init_refuses_existing_dir_without_touching_content() {
    let tmp = TempRoot::new("init-exists");
    let dir = tmp.path().join("keep-me");
    std::fs::create_dir_all(&dir).unwrap();
    let original = b"\x00original bytes, do not touch\xff".to_vec();
    std::fs::write(dir.join("existing.bin"), &original).unwrap();

    let err = init_expert(&dir, &draft("demo-expert")).unwrap_err();
    assert!(
        matches!(err, ExpertError::Io { .. }),
        "应显式报已存在: {err}"
    );
    assert_eq!(err.code(), "IO_ERROR");
    assert_eq!(read(&dir.join("existing.bin")), original);
    assert!(!dir.join(paths::PLUGIN_DIR).exists());
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|item| item.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("existing.bin")]);
}

#[test]
fn init_then_content_validate_register_round_trip() {
    let tmp = TempRoot::new("round-trip");
    let root = tmp.experts_root();
    let dir = root.join("demo-expert");

    init_expert(&dir, &draft("demo-expert")).unwrap();
    complete_package(&dir, "demo-expert", "1.0.0");

    let manifest = ExpertManifest::from_json_str(
        &std::fs::read_to_string(dir.join(paths::manifest_rel_path())).unwrap(),
    )
    .unwrap();
    assert!(Validator::new()
        .validate_dir(&dir, &manifest)
        .unwrap()
        .is_ok());

    let mut registry = ExpertRegistry::default();
    let entry = register_expert(&mut registry, &dir).unwrap();
    assert_eq!(entry.id, "demo-expert");
    assert_eq!(entry.path, "experts/demo-expert");
    assert_eq!(registry.find("demo-expert").unwrap().version, "1.0.0");

    // 注册表已落盘到专家根，且能按磁盘真相读回。
    let registry_path = root.join(paths::REGISTRY_FILE);
    assert!(registry_path.is_file());
    let loaded = ExpertRegistry::load_registry(&registry_path).unwrap();
    assert_eq!(loaded.entries().len(), 1);
    assert_eq!(loaded.find("demo-expert").unwrap().version, "1.0.0");
}

#[test]
fn register_invalid_package_writes_nothing() {
    let tmp = TempRoot::new("register-invalid");
    let root = tmp.experts_root();
    let dir = root.join("broken-expert");
    complete_package(&dir, "broken-expert", "1.0.0");
    // tags 只有 2 项 ⇒ §17 #10 TAG_COUNT（Error）。
    let mut broken = valid_manifest("broken-expert", "1.0.0");
    broken.tags.truncate(2);
    write_manifest(&dir, &broken);

    let mut registry = ExpertRegistry::default();
    let registry_path = root.join(paths::REGISTRY_FILE);
    registry.save_registry(&registry_path).unwrap();
    let before = read(&registry_path);

    let err = register_expert(&mut registry, &dir).unwrap_err();
    assert_eq!(err.code(), "TAG_COUNT");
    assert_eq!(read(&registry_path), before, "校验失败不得写 registry.json");
    assert!(registry.entries().is_empty(), "内存注册表也应保持原样");
}

#[test]
fn same_name_reregistration_validates_then_updates() {
    let tmp = TempRoot::new("reregister");
    let root = tmp.experts_root();
    let dir = root.join("demo-expert");
    complete_package(&dir, "demo-expert", "1.0.0");
    let mut registry = ExpertRegistry::default();
    register_expert(&mut registry, &dir).unwrap();

    // 同名再次注册：走「校验 + 注册」，版本更新（不静默跳过）。
    complete_package(&dir, "demo-expert", "1.1.0");
    let entry = register_expert(&mut registry, &dir).unwrap();
    assert_eq!(entry.version, "1.1.0");
    assert_eq!(registry.entries().len(), 1);
    assert_eq!(registry.find("demo-expert").unwrap().version, "1.1.0");

    // 同名但校验不过：报错，注册表（磁盘 + 内存）保持上一次成功状态。
    let registry_path = root.join(paths::REGISTRY_FILE);
    let before = read(&registry_path);
    let mut broken = valid_manifest("demo-expert", "2.0.0");
    broken.quick_prompts.truncate(2);
    write_manifest(&dir, &broken);
    let err = register_expert(&mut registry, &dir).unwrap_err();
    assert_eq!(err.code(), "QUICK_PROMPT_COUNT");
    assert_eq!(read(&registry_path), before);
    assert_eq!(registry.find("demo-expert").unwrap().version, "1.1.0");
}

#[test]
fn unregister_removes_index_but_keeps_package() {
    let tmp = TempRoot::new("unregister");
    let root = tmp.experts_root();
    let dir = root.join("demo-expert");
    complete_package(&dir, "demo-expert", "1.0.0");
    let mut registry = ExpertRegistry::default();
    register_expert(&mut registry, &dir).unwrap();

    let removed = unregister_expert(&mut registry, "demo-expert").unwrap();
    assert_eq!(removed.id, "demo-expert");
    assert!(registry.find("demo-expert").is_none());
    assert!(registry.entries().is_empty());
    // 只删索引，不删包目录（破坏性动作不在本卡语义里）。
    assert!(dir.join(paths::manifest_rel_path()).is_file());
    assert!(dir.join(paths::AGENTS_DIR).join("demo-expert.md").is_file());

    let err = unregister_expert(&mut registry, "no-such-expert").unwrap_err();
    assert_eq!(err.code(), "IO_ERROR");
}

#[path = "registry_cli/main/mod.rs"]
mod main;
