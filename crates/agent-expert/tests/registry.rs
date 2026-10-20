//! 专家双根发现与注册表读写测试（专家体系 · 卡 3）。
//!
//! 样本来源同 `tests/manifest.rs`：真实样本逐字复制进 `tests/fixtures/`；
//! 临时目录用 `std::env::temp_dir()` + 进程/计数唯一名（本 crate 不在依赖白名单
//! 内加 `tempfile`，见 `docs/expert-discovery-status.md` 未做项）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use agent_expert::paths;
use agent_expert::{
    ExpertError, ExpertOrigin, ExpertRegistry, ExpertRoot, ExpertScope, RegistryEntry, RegistryFile,
};

const SENIOR: &str = include_str!("fixtures/senior-developer.plugin.json");
const PANDAI: &str = include_str!("fixtures/pandaai-team.plugin.json");

/// 临时根目录（Drop 时清理）。
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-expert-registry-{}-{}-{n}",
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
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 写一个专家包：`<root>/<id>/.lengsuo-plugin/plugin.json`（含 agents 目录）。
fn write_package(root: &Path, id: &str, manifest: &str) -> PathBuf {
    let dir = root.join(id);
    std::fs::create_dir_all(dir.join(paths::PLUGIN_DIR)).unwrap();
    std::fs::create_dir_all(dir.join(paths::AGENTS_DIR)).unwrap();
    std::fs::write(dir.join(paths::manifest_rel_path()), manifest).unwrap();
    dir
}

#[test]
fn same_name_project_root_overrides_user_root() {
    let tmp = TempRoot::new("override");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    let project = paths::project_experts_root(&tmp.path().join("ws"));
    write_package(&user, "senior-developer", SENIOR);
    // 项目级同名但版本不同：用于区分哪一份生效。
    let bumped = SENIOR.replace("\"1.1.1\"", "\"2.0.0\"");
    write_package(&project, "senior-developer", &bumped);

    // 后注册者优先：顺序 [用户级, 项目级] ⇒ 项目级生效。
    let packages = ExpertRegistry::discover(&[user.clone(), project.clone()]).unwrap();
    assert_eq!(packages.len(), 1);
    let pkg = &packages[0];
    assert_eq!(pkg.id, "senior-developer");
    assert_eq!(pkg.manifest.version, "2.0.0");
    assert_eq!(pkg.dir, project.join("senior-developer"));
    assert_eq!(pkg.overrides.len(), 1);
    assert_eq!(pkg.overrides[0].dir, user.join("senior-developer"));

    // 覆盖关系可查：注册表条目记录生效层级 + 被覆盖者。
    let registry = ExpertRegistry::from_packages(&packages);
    assert_eq!(
        registry.scope_of("senior-developer"),
        Some(ExpertScope::Project)
    );
    let (entry, shadowed) = registry.overrides().next().expect("覆盖关系应可查");
    assert_eq!(entry.id, "senior-developer");
    assert_eq!(shadowed.scope, ExpertScope::Project); // 裸路径推断：不在 home 下
    assert_eq!(shadowed.path, "experts/senior-developer");

    // 显式层级：项目级生效，被覆盖者是用户级。
    let scoped = ExpertRegistry::discover_scoped(&[
        ExpertRoot::new(user.clone(), ExpertScope::User),
        ExpertRoot::new(project.clone(), ExpertScope::Project),
    ])
    .unwrap();
    assert_eq!(scoped[0].scope, ExpertScope::Project);
    assert_eq!(scoped[0].overrides[0].scope, ExpertScope::User);
    assert_eq!(scoped[0].overrides[0].dir, user.join("senior-developer"));
    let registry = ExpertRegistry::from_packages(&scoped);
    let (entry, shadowed) = registry.overrides().next().unwrap();
    assert_eq!(entry.scope, ExpertScope::Project);
    assert_eq!(entry.path, "experts/senior-developer");
    assert_eq!(shadowed.scope, ExpertScope::User);
    assert_eq!(shadowed.path, "experts/senior-developer");

    // 反过来注册 ⇒ 用户级后到、生效（后注册者优先，与 SkillRegistry 一致）。
    let reversed = ExpertRegistry::discover_scoped(&[
        ExpertRoot::new(project, ExpertScope::Project),
        ExpertRoot::new(user, ExpertScope::User),
    ])
    .unwrap();
    assert_eq!(reversed[0].manifest.version, "1.1.1");
    assert_eq!(reversed[0].scope, ExpertScope::User);
    assert_eq!(reversed[0].overrides[0].scope, ExpertScope::Project);
}

#[test]
fn distinct_names_merge_from_both_roots() {
    let tmp = TempRoot::new("merge");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    let project = paths::project_experts_root(&tmp.path().join("ws"));
    write_package(&user, "senior-developer", SENIOR);
    write_package(&project, "pandaai-ai-quant-research-team", PANDAI);

    let packages = ExpertRegistry::discover(&[user, project]).unwrap();
    let ids: Vec<_> = packages.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["pandaai-ai-quant-research-team", "senior-developer"]);
    assert!(packages.iter().all(|p| p.overrides.is_empty()));
}

#[test]
fn registry_missing_is_rebuilt_without_error() {
    let tmp = TempRoot::new("rebuild");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    write_package(&user, "senior-developer", SENIOR);
    write_package(&user, "pandaai-ai-quant-research-team", PANDAI);
    let registry_path = user.join(paths::REGISTRY_FILE);
    assert!(!registry_path.exists());

    let registry = ExpertRegistry::load_registry(&registry_path).unwrap();
    assert_eq!(registry.entries().len(), 2);
    let entry = registry.find("senior-developer").unwrap();
    assert_eq!(entry.version, "1.1.1");
    assert_eq!(entry.path, "experts/senior-developer");
    assert_eq!(entry.source, ExpertOrigin::Local);
    assert!(entry.registered_at.as_deref().unwrap().ends_with('Z'));

    // 落盘 → 读回：格式逐字段（§4.2）。
    registry.save_registry(&registry_path).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(raw["schemaVersion"], 1);
    let senior = &raw["experts"][1];
    assert_eq!(senior["id"], "senior-developer");
    assert_eq!(senior["version"], "1.1.1");
    assert_eq!(senior["expertType"], "agent");
    assert_eq!(senior["categoryId"], "02-Engineering");
    assert_eq!(senior["path"], "experts/senior-developer");
    assert_eq!(senior["source"], "local");
    assert_eq!(senior["scope"], "project");
    let typed: RegistryFile =
        serde_json::from_str(&std::fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(typed.experts.len(), 2);
    assert_eq!(typed.experts[0].expert_type, agent_expert::ExpertType::Team);

    // 已存在且一致 ⇒ 原样读回（保留磁盘上的 registeredAt）。
    let loaded = ExpertRegistry::load_registry(&registry_path).unwrap();
    assert_eq!(loaded, registry);
}

#[test]
fn drifted_registry_follows_disk() {
    let tmp = TempRoot::new("drift");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    write_package(&user, "senior-developer", SENIOR);
    let registry_path = user.join(paths::REGISTRY_FILE);
    // 手改的注册表：版本与磁盘漂移 ⇒ 以目录为准（§4.1 第 3 条）。
    let forged = r#"{"schemaVersion":1,"experts":[{"id":"senior-developer","version":"9.9.9","expertType":"agent","categoryId":"02-Engineering","path":"experts/senior-developer","source":"imported","scope":"user"}]}"#;
    std::fs::write(&registry_path, forged).unwrap();

    let registry = ExpertRegistry::load_registry(&registry_path).unwrap();
    let entry: &RegistryEntry = registry.find("senior-developer").unwrap();
    assert_eq!(entry.version, "1.1.1");
    assert_eq!(entry.source, ExpertOrigin::Local);
}

#[test]
fn missing_root_is_skipped() {
    let tmp = TempRoot::new("missing-root");
    let user = tmp.path().join("does-not-exist");
    let project = paths::project_experts_root(&tmp.path().join("ws"));
    write_package(&project, "senior-developer", SENIOR);

    let packages = ExpertRegistry::discover(&[user, project]).unwrap();
    assert_eq!(packages.len(), 1);
    // 两个根都不存在 ⇒ 空表，不报错。
    assert!(
        ExpertRegistry::discover(&[tmp.path().join("a"), tmp.path().join("b")])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn plain_directory_without_manifest_marker_is_ignored() {
    let tmp = TempRoot::new("ignore");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    std::fs::create_dir_all(user.join("not-an-expert").join("docs")).unwrap();
    std::fs::write(user.join("loose-file.txt"), "x").unwrap();
    write_package(&user, "senior-developer", SENIOR);
    let packages = ExpertRegistry::discover(&[user]).unwrap();
    assert_eq!(packages.len(), 1);
}

#[test]
fn expert_dir_with_corrupt_manifest_is_explicit_error() {
    let tmp = TempRoot::new("corrupt");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    let dir = user.join("broken");
    std::fs::create_dir_all(dir.join(paths::PLUGIN_DIR)).unwrap();
    std::fs::write(dir.join(paths::manifest_rel_path()), "{ not json").unwrap();
    let err = ExpertRegistry::discover(std::slice::from_ref(&user)).unwrap_err();
    assert_eq!(err.code(), "MANIFEST_PARSE_FAILED");
    assert_eq!(err.severity(), agent_expert::Severity::Error);

    // 缺必填字段同样是显式错误。
    std::fs::write(dir.join(paths::manifest_rel_path()), r#"{"name":"broken"}"#).unwrap();
    let err = ExpertRegistry::discover(std::slice::from_ref(&user)).unwrap_err();
    assert_eq!(err.code(), "MISSING_FIELD");

    // 增量重写注册表时，损坏包也必须显式失败（不能「以目录为准」把专家抹掉）。
    let registry_path = user.join(paths::REGISTRY_FILE);
    ExpertRegistry::load_registry(&registry_path).unwrap_err();
}

#[test]
fn expert_dir_without_manifest_is_explicit_error() {
    let tmp = TempRoot::new("no-manifest");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    // 有 agents/（包标记）但没有 plugin.json。
    std::fs::create_dir_all(user.join("half-baked").join(paths::AGENTS_DIR)).unwrap();
    let err = ExpertRegistry::discover(std::slice::from_ref(&user)).unwrap_err();
    assert_eq!(err.code(), "PLUGIN_MANIFEST_MISSING");
    match err {
        ExpertError::PluginManifestMissing { path } => {
            assert_eq!(
                path,
                user.join("half-baked").join(paths::manifest_rel_path())
            )
        }
        other => panic!("期望 PluginManifestMissing，实际 {other:?}"),
    }
}

#[test]
fn dot_parent_escape_is_rejected() {
    let tmp = TempRoot::new("dotdot");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    write_package(&tmp.path().join("elsewhere"), "outside", SENIOR);
    std::fs::create_dir_all(&user).unwrap();
    // 根内一个 symlink：../elsewhere/outside（词法合法、真实路径越界）。
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        tmp.path().join("elsewhere").join("outside"),
        user.join("outside"),
    )
    .unwrap();
    let err = ExpertRegistry::discover(&[user]).unwrap_err();
    assert_eq!(err.code(), "PATH_ESCAPE");

    // 纯词法 `..` 穿越（不依赖文件系统）。
    let base = tmp.path().join("base");
    let escaped = base.join("..").join("ghost");
    assert!(paths::ensure_within(&base, &escaped).is_err());
    let inside = base.join("a").join(".").join("b");
    assert!(paths::ensure_within(&base, &inside).is_ok());

    // 公开的相对路径还原入口同样拒绝 `..`。
    let err = agent_expert::registry::resolve_within(&base, Path::new("../secret")).unwrap_err();
    assert_eq!(err.code(), "PATH_ESCAPE");
}

#[cfg(unix)]
#[test]
fn symlink_escaping_expert_dir_is_rejected() {
    let tmp = TempRoot::new("symlink");
    let user = paths::user_experts_root(&tmp.path().join("home"));
    let outside = write_package(&tmp.path().join("outside"), "senior-developer", SENIOR);
    std::fs::create_dir_all(&user).unwrap();
    std::os::unix::fs::symlink(&outside, user.join("senior-developer")).unwrap();
    let err = ExpertRegistry::discover(std::slice::from_ref(&user)).unwrap_err();
    assert_eq!(err.code(), "PATH_ESCAPE");
}
