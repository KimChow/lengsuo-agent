//! 专家技能挂载测试（专家体系 · 卡 7；规格 §16.2）。
//!
//! 样本事实来自 本地对照资料（不入库）：
//! - `pandaai-ai-quant-research-team`：6 个技能目录，**目录名无统一前缀**
//!   （`skill-report-replication` 相对专家 id 无前缀）；
//! - `senior-developer`：3 个技能目录（`browser-use` / `frontend-dev` / `fullstack-dev`），
//!   无一与专家名相关。
//!
//! 这里的 fixture 用**临时目录**现搭技能包（只放 `SKILL.md`），因为真实样本
//! 不在 CI 里；`plugin.json.skills` 的取值与样本逐字一致
//! （见 `fixtures/*.plugin.json`）。

use std::fs;
use std::path::{Path, PathBuf};

use agent_expert::skills::{
    catalog_roots, mount_expert_skills, resolve_mounted, visible_skills, MountedSkill, VisibleSkill,
};
use agent_expert::ExpertError;
use agent_skills::SkillCatalog;

const PANDAI_NAME: &str = "pandaai-ai-quant-research-team";
const PANDAI_SKILLS: [&str; 6] = [
    "ai-quant-team-runtime",
    "skill-report-replication",
    "skill-factor-mining-pandaai",
    "skill-pandaai-factor-online",
    "skill-backtest-overfit",
    "skill-strategy-tearsheet-report",
];
const SENIOR_NAME: &str = "senior-developer";
const SENIOR_SKILLS: [&str; 3] = ["browser-use", "frontend-dev", "fullstack-dev"];

fn declared(skills: &[&str]) -> Vec<String> {
    skills.iter().map(|s| format!("./skills/{s}")).collect()
}

/// 在工作区根的临时目录下搭一个专家包：`<root>/<expert>/skills/<dir>/SKILL.md`。
fn expert_pkg(root: &Path, expert_name: &str, skill_dirs: &[&str]) -> PathBuf {
    let dir = root.join(expert_name);
    for name in skill_dirs {
        let skill_dir = dir.join("skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 测试技能 {name}\n---\n正文\n# 标题\n"),
        )
        .unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 新建一个空的工作区根：测试各自独立，互不影响。
fn workspace(tag: &str) -> PathBuf {
    let base =
        std::env::temp_dir().join(format!("lengsuo-expert-mount-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base
}

fn names(skills: &[MountedSkill]) -> Vec<&str> {
    skills.iter().map(|s| s.mounted_name.as_str()).collect()
}

fn names_of(skills: &[VisibleSkill]) -> Vec<&str> {
    skills.iter().map(|s| s.mounted_name.as_str()).collect()
}

#[test]
fn pandaai_six_skills_get_expert_prefix_and_keep_dir_name() {
    let ws = workspace("pandaai");
    let pkg = expert_pkg(&ws, PANDAI_NAME, &PANDAI_SKILLS);
    let mounted = mount_expert_skills(PANDAI_NAME, &pkg, &declared(&PANDAI_SKILLS)).unwrap();

    assert_eq!(mounted.len(), 6);
    // 对外名：<expert>__<源目录名>——前缀是装载时加的，源目录名保持原样。
    let expected: Vec<String> = PANDAI_SKILLS
        .iter()
        .map(|s| format!("{PANDAI_NAME}__{s}"))
        .collect();
    assert_eq!(
        names(&mounted),
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for (m, dir) in mounted.iter().zip(PANDAI_SKILLS) {
        assert_eq!(m.expert_name, PANDAI_NAME);
        assert_eq!(m.skill_name, dir);
        assert_eq!(m.source, format!("./skills/{dir}"));
        assert_eq!(m.skill_dir, pkg.join("skills").join(dir));
        // 源目录名不带专家前缀（样本事实），本仓不强制。
        assert!(!m.skill_dir.ends_with(format!("/{PANDAI_NAME}__{dir}")));
    }
}

#[test]
fn senior_three_skills_get_expert_prefix() {
    let ws = workspace("senior");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &SENIOR_SKILLS);
    let mounted = mount_expert_skills(SENIOR_NAME, &pkg, &declared(&SENIOR_SKILLS)).unwrap();

    assert_eq!(
        names(&mounted),
        vec![
            "senior-developer__browser-use",
            "senior-developer__frontend-dev",
            "senior-developer__fullstack-dev",
        ]
    );
    // 源目录名（非暴露名）仍是无前缀的 `browser-use` 等。
    let dir_names: Vec<_> = mounted
        .iter()
        .map(|m| m.skill_dir.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(dir_names, SENIOR_SKILLS.to_vec());
}

#[test]
fn prefix_uses_double_underscore_not_single_dash() {
    let ws = workspace("sep");
    let pkg = expert_pkg(&ws, "demo-expert", &["a-b"]);
    let mounted = mount_expert_skills("demo-expert", &pkg, &declared(&["a-b"])).unwrap();
    assert_eq!(
        names(&mounted),
        vec!["demo-expert__a-b"],
        "双下划线连接：kebab-case 里的 `-` 不参与分隔"
    );
}

#[test]
fn declared_skill_dir_without_skill_md_is_an_explicit_error() {
    let ws = workspace("nofile");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &["browser-use"]);
    fs::create_dir_all(pkg.join("skills/frontend-dev")).unwrap(); // 只有空目录
    let err = mount_expert_skills(
        SENIOR_NAME,
        &pkg,
        &declared(&["browser-use", "frontend-dev"]),
    )
    .unwrap_err();
    assert!(
        matches!(&err, ExpertError::SkillPathMissing { path } if path == "./skills/frontend-dev"),
        "缺 SKILL.md 必须显式报 SkillPathMissing，实际 {err:?}"
    );
    assert_eq!(err.code(), "SKILL_PATH_MISSING");
}

#[test]
fn declared_missing_directory_is_an_explicit_error_not_skipped() {
    let ws = workspace("missing");
    let pkg = expert_pkg(&ws, PANDAI_NAME, &["ai-quant-team-runtime"]);
    let err = mount_expert_skills(
        PANDAI_NAME,
        &pkg,
        &declared(&["ai-quant-team-runtime", "skill-report-replication"]),
    )
    .unwrap_err();
    // 声明了不存在的目录 ⇒ 显式错误（不静默跳过）。
    assert!(
        matches!(&err, ExpertError::SkillPathMissing { path } if path == "./skills/skill-report-replication"),
        "实际 {err:?}"
    );
}

#[test]
fn parent_dir_escape_is_rejected() {
    let ws = workspace("escape");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &["browser-use"]);
    // 包外放一个"像技能"的目录：`../outside-skill`
    let outside = pkg
        .join("skills")
        .join("..")
        .join("..")
        .join("outside-skill");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("SKILL.md"),
        "---\nname: outside\n---\n正文\n# 标题\n",
    )
    .unwrap();

    for bad in [
        "./skills/../../outside-skill",
        "../outside-skill",
        "./SKILL/../x",
    ] {
        let err = mount_expert_skills(SENIOR_NAME, &pkg, &[bad.to_string()]).unwrap_err();
        assert!(
            matches!(&err, ExpertError::PathEscape { path } if path == bad),
            "{bad} 应被拒，实际 {err:?}"
        );
        assert_eq!(err.code(), "PATH_ESCAPE");
    }
}

#[cfg(unix)]
#[test]
fn symlink_escaping_expert_dir_is_rejected() {
    let ws = workspace("symlink");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &[]);
    let outside = ws.join("outside-skill");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("SKILL.md"),
        "---\nname: outside\n---\n正文\n# 标题\n",
    )
    .unwrap();
    fs::create_dir_all(pkg.join("skills")).unwrap();
    std::os::unix::fs::symlink(&outside, pkg.join("skills/escape")).unwrap();
    let err = mount_expert_skills(SENIOR_NAME, &pkg, &declared(&["escape"])).unwrap_err();
    // 与卡 3 的 `ensure_within` 同口径：symlink 落到包外 ⇒ PATH_ESCAPE。
    assert_eq!(err.code(), "PATH_ESCAPE");
}

#[test]
fn absolute_path_is_rejected() {
    let ws = workspace("abs");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &["browser-use"]);
    let outside = ws.join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("SKILL.md"),
        "---\nname: elsewhere\n---\n正文\n# 标题\n",
    )
    .unwrap();
    let err = mount_expert_skills(SENIOR_NAME, &pkg, &[outside.to_string_lossy().into_owned()])
        .unwrap_err();
    assert!(
        matches!(err, ExpertError::PathEscape { .. }),
        "实际 {err:?}"
    );
}

#[test]
fn catalog_roots_cover_only_active_expert_and_keep_global() {
    let ws = workspace("roots");
    let global = ws.join("global-skills");
    let pkg_a = expert_pkg(&ws, "expert-a", &["alpha"]);
    let pkg_b = expert_pkg(&ws, "expert-b", &["beta"]);

    let mounted_a = mount_expert_skills("expert-a", &pkg_a, &declared(&["alpha"])).unwrap();
    let roots = catalog_roots(std::slice::from_ref(&global), &mounted_a);
    // 全局根在前、激活专家的技能容器在后（`SkillCatalog` 的根 = 技能目录的父目录）。
    let container_a = pkg_a.join("skills");
    assert_eq!(roots, vec![global, container_a]);
    // 未激活专家 B 的技能不在列表里（私有）。
    assert!(!roots.iter().any(|r| r.starts_with(&pkg_b)));

    let mounted_b = mount_expert_skills("expert-b", &pkg_b, &declared(&["beta"])).unwrap();
    let roots_b = catalog_roots(&[], &mounted_b);
    assert_eq!(roots_b, vec![pkg_b.join("skills")]);
    assert!(!roots_b.iter().any(|r| r.starts_with(&pkg_a)));
    // 同一专家的多个技能共用容器时只出现一次。
    let pkg_c = expert_pkg(&ws, "expert-c", &["one", "two"]);
    let mounted_c = mount_expert_skills("expert-c", &pkg_c, &declared(&["one", "two"])).unwrap();
    assert_eq!(catalog_roots(&[], &mounted_c), vec![pkg_c.join("skills")]);
}

#[test]
fn empty_declaration_mounts_nothing() {
    let ws = workspace("empty");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &[]);
    let mounted = mount_expert_skills(SENIOR_NAME, &pkg, &[]).unwrap();
    assert!(mounted.is_empty());
}

// —— 以下对接 `agent-skills::SkillCatalog`：超出本卡文件范围的依赖门由收口/基建卡补齐 ——
// `agent-expert` 的 Cargo.toml / lib.rs 本卡无独占权（见 docs 未做项），收口卡补上
// `agent-skills` 依赖 + `pub mod skills;` 后，本组用例随 `cargo test -p agent-expert` 变绿。

#[test]
fn skill_catalog_sees_expert_skills_with_global_root() {
    let ws = workspace("catalog");
    let global = ws.join("global-skills");
    fs::create_dir_all(global.join("pdf-forms")).unwrap();
    fs::write(
        global.join("pdf-forms/SKILL.md"),
        "---\nname: pdf-forms\ndescription: 全局技能\n---\n正文\n# 标题\n",
    )
    .unwrap();
    let pkg = expert_pkg(&ws, "expert-a", &["alpha", "beta"]);
    let pkg_b = expert_pkg(&ws, "expert-b", &["gamma"]);
    let mounted = mount_expert_skills("expert-a", &pkg, &declared(&["alpha", "beta"])).unwrap();

    let listed = SkillCatalog::new(
        catalog_roots(std::slice::from_ref(&global), &mounted),
        vec![],
    )
    .unwrap()
    .list()
    .unwrap();
    let names: Vec<_> = listed.iter().map(|s| s.name.as_str()).collect();
    // 全局技能与专家技能共存；专家技能按源名（SKILL.md 的 name）进目录。
    assert_eq!(names, vec!["alpha", "beta", "pdf-forms"]);
    // 可见集带对外名前缀；未激活专家的技能不在其中。
    let catalog = SkillCatalog::new(catalog_roots(&[global], &mounted), vec![]).unwrap();
    let visible = visible_skills(&catalog, &mounted).unwrap();
    assert_eq!(
        names_of(&visible),
        vec!["expert-a__alpha", "expert-a__beta"]
    );
    assert!(!visible.iter().any(|v| v.mounted_name.contains("gamma")));

    // 激活专家 B：只带 B 的容器 + 全局根；A 的技能消失。
    let mounted_b = mount_expert_skills("expert-b", &pkg_b, &declared(&["gamma"])).unwrap();
    let catalog_b = SkillCatalog::new(catalog_roots(&[], &mounted_b), vec![]).unwrap();
    let names: Vec<_> = catalog_b
        .list()
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, vec!["gamma"]);
    let visible_b = visible_skills(&catalog_b, &mounted_b).unwrap();
    assert_eq!(names_of(&visible_b), vec!["expert-b__gamma"]);
    assert!(catalog_b
        .authorized_roots()
        .iter()
        .all(|r| !r.starts_with(&pkg)));
}

#[test]
fn resolve_mounted_maps_external_name_to_dir() {
    let ws = workspace("alias");
    let pkg = expert_pkg(&ws, SENIOR_NAME, &SENIOR_SKILLS);
    let mounted = mount_expert_skills(SENIOR_NAME, &pkg, &declared(&SENIOR_SKILLS)).unwrap();
    let hit = resolve_mounted(&mounted, "senior-developer__frontend-dev").unwrap();
    assert_eq!(hit.source, "./skills/frontend-dev");
    // 源名（无前缀）不是对外名：必须用 `<expert>__<dir>` 查找。
    assert!(resolve_mounted(&mounted, "frontend-dev").is_none());
}

#[test]
fn undeclared_sibling_dir_is_not_visible() {
    let ws = workspace("undeclared");
    // 包内有 `beta` 目录但未在 plugin.json.skills 声明：不得进入可见集。
    let pkg = expert_pkg(&ws, "expert-a", &["alpha", "beta"]);
    let mounted = mount_expert_skills("expert-a", &pkg, &declared(&["alpha"])).unwrap();
    let catalog = SkillCatalog::new(catalog_roots(&[], &mounted), vec![]).unwrap();
    let visible = visible_skills(&catalog, &mounted).unwrap();
    assert_eq!(names_of(&visible), vec!["expert-a__alpha"]);
}
