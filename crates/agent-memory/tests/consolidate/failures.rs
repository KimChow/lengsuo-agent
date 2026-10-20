//! 失败语义：读/写失败 ⇒ Err 且不删除任何日文件（先写后删，不可交换）。

use std::fs;

use agent_memory::consolidate;
use chrono::Duration;

use crate::support::{now, TempDir, NOW_DATE};

fn old_date(n: i64) -> chrono::NaiveDate {
    NOW_DATE - Duration::days(n)
}

#[test]
fn write_failure_returns_err_and_deletes_nothing() {
    let tmp = TempDir::new("fail-write");
    let old = old_date(40);
    let recent = old_date(5);
    tmp.write_daily(old, "- 09:00 旧摘要\n");
    tmp.write_daily(recent, "- 09:00 近期摘要\n");
    tmp.write_consolidated(&format!("# 项目记忆蒸馏\n\n## {old}\n- 09:00 旧摘要\n"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let memory_dir = tmp.memory_dir();
        fs::set_permissions(&memory_dir, fs::Permissions::from_mode(0o500)).unwrap();

        let result = consolidate(&tmp.workspace(), now());
        if result.is_ok() {
            // uid 0 等环境可写 0o500：解除权限后跳过，避免环境相关假失败。
            fs::set_permissions(&memory_dir, fs::Permissions::from_mode(0o700)).unwrap();
            eprintln!("跳过只读写入断言：当前环境可写 0o500 目录");
            return;
        }
        assert!(result.is_err(), "写入被拒必须显式上抛: {result:?}");
        fs::set_permissions(&memory_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }

    // 写失败 ⇒ 日文件一个都不能少（尤其已入册的 40 天旧文件）。
    assert!(tmp.exists(old), "写失败不得删除已入册旧日文件");
    assert!(tmp.exists(recent), "写失败不得删除任何日文件");
}

#[test]
fn unreadable_daily_file_returns_err_and_deletes_nothing() {
    let tmp = TempDir::new("fail-read");
    let old = old_date(41);
    let unreadable = old_date(3);
    tmp.write_daily(old, "- 09:00 旧摘要\n");
    tmp.write_daily(unreadable, "- 09:00 不可读\n");
    tmp.write_consolidated(&format!("# 项目记忆蒸馏\n\n## {old}\n- 09:00 旧摘要\n"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = tmp.daily_file(unreadable);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_to_string(&file).is_ok() {
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            eprintln!("跳过只读读取断言：当前环境可读 0o000 文件");
            return;
        }

        let result = consolidate(&tmp.workspace(), now());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(result.is_err(), "读失败必须显式上抛: {result:?}");
    }

    assert!(tmp.exists(old), "读失败不得删除已入册旧日文件");
    assert!(tmp.exists(unreadable), "读失败不得删除任何日文件");
}
