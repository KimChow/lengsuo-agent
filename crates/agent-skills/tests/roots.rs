//! `SkillRoots` 行为测试：三级顺序、环境覆盖、缺失跳过、不可读显式报错。
//!
//! 环境变量是进程级状态，本文件内所有用例共用一把锁并各自在开头清空，
//! 避免 Cargo 的并行测试线程互相污染。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use agent_skills::{SkillRoots, SkillsError, PROJECT_DIR_ENV, USER_DIR_ENV};
use tempfile::TempDir;

/// 本文件内串行化环境变量的读写。
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 每个用例开头调用：把两个覆盖变量恢复为「未设置」。
fn clear_env() {
    env::remove_var(USER_DIR_ENV);
    env::remove_var(PROJECT_DIR_ENV);
}

/// 默认数据目录下的技能根：`<base>/.lengsuo/skills/`。
fn skills_dir(base: &Path) -> PathBuf {
    base.join(".lengsuo").join("skills")
}

fn make_dir(path: &Path) {
    fs::create_dir_all(path).expect("创建目录");
}

#[test]
fn all_three_levels_are_ordered_builtin_user_project() {
    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let builtin = TempDir::new().unwrap();
    let user = skills_dir(home.path());
    let project = skills_dir(workspace.path());
    make_dir(&user);
    make_dir(&project);

    let roots =
        SkillRoots::resolve_default(home.path(), workspace.path()).with_builtin(builtin.path());

    // 低优先级 → 高优先级：registry 的后注册者优先，故项目级必须最后。
    assert_eq!(
        roots.as_dirs().unwrap(),
        vec![builtin.path().to_path_buf(), user, project]
    );
}

#[test]
fn only_existing_project_level_is_returned() {
    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let project = skills_dir(workspace.path());
    make_dir(&project);

    let roots = SkillRoots::resolve_default(home.path(), workspace.path());

    // 内置级缺省为 None；用户级目录不存在 ⇒ 跳过。
    assert_eq!(roots.builtin(), None);
    assert_eq!(roots.user(), Some(skills_dir(home.path()).as_path()));
    assert_eq!(roots.as_dirs().unwrap(), vec![project]);
}

#[test]
fn missing_directories_are_skipped_without_error() {
    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let builtin_base = TempDir::new().unwrap();
    let builtin = builtin_base.path().join("not-installed");

    let roots = SkillRoots::resolve_default(home.path(), workspace.path()).with_builtin(builtin);

    assert!(roots.as_dirs().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn unreadable_user_directory_is_an_explicit_error() {
    use std::os::unix::fs::PermissionsExt;

    /// 断言先于清理：即使 panic，也要恢复权限让 `TempDir` 能删掉目录。
    struct RestoreMode(PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let user = skills_dir(home.path());
    let project = skills_dir(workspace.path());
    make_dir(&user);
    make_dir(&project);

    fs::set_permissions(&user, fs::Permissions::from_mode(0o000)).unwrap();
    let _restore = RestoreMode(user.clone());

    // root 不受权限位限制；此时无法验证「不可读」，明确跳过而不是假绿。
    if fs::read_dir(&user).is_ok() {
        eprintln!("跳过：当前用户可读取 0o000 目录（可能以 root 运行）");
        return;
    }

    let roots = SkillRoots::resolve_default(home.path(), workspace.path());
    match roots.as_dirs() {
        Err(SkillsError::Io { path, source }) => {
            assert_eq!(path, user);
            assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        }
        other => panic!("不可读目录必须显式报错，实得 {other:?}"),
    }
}

#[test]
fn environment_overrides_win_over_defaults() {
    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let custom_user = TempDir::new().unwrap();
    let custom_project = TempDir::new().unwrap();
    // 默认目录也建出来，证明「覆盖」而非「回退」。
    make_dir(&skills_dir(home.path()));
    make_dir(&skills_dir(workspace.path()));

    env::set_var(USER_DIR_ENV, custom_user.path());
    env::set_var(PROJECT_DIR_ENV, custom_project.path());

    let roots = SkillRoots::resolve_default(home.path(), workspace.path());

    assert_eq!(roots.user(), Some(custom_user.path()));
    assert_eq!(roots.project(), Some(custom_project.path()));
    assert_eq!(
        roots.as_dirs().unwrap(),
        vec![
            custom_user.path().to_path_buf(),
            custom_project.path().to_path_buf()
        ]
    );
}

#[test]
fn empty_environment_value_means_unset() {
    let _lock = env_lock();
    clear_env();

    let home = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let user = skills_dir(home.path());
    let project = skills_dir(workspace.path());
    make_dir(&user);
    make_dir(&project);

    env::set_var(USER_DIR_ENV, "");
    env::set_var(PROJECT_DIR_ENV, "");

    let roots = SkillRoots::resolve_default(home.path(), workspace.path());

    // 空串不得变成空路径（否则扫描根会退化成 `/`）。
    assert_eq!(roots.user(), Some(user.as_path()));
    assert_eq!(roots.project(), Some(project.as_path()));
    assert_eq!(roots.as_dirs().unwrap(), vec![user, project]);
}
