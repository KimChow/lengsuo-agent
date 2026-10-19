//! 路径安全闸：受保护分量 deny 的入口一致性、同名前缀不误伤、
//! 递归搜索/列举剪枝、批量回滚无部分副作用。
//! 符号链接面的用例见 `path_links.rs`。
//!
//! 全部凭据与链接都建在临时工作区里（`Sandbox`），不触碰真实家目录。

use std::path::Path;

use agent_tools::{FsToolbox, ToolError, WriteOp};

mod support;

use support::Sandbox;

/// 工作区里的假凭据目录 + 一个同名前缀的合法兄弟目录。
fn creds(sb: &Sandbox) {
    sb.put(".ssh/id_rsa", "FAKE-PRIVATE-KEY");
    sb.put(".ssh/nested/authorized_keys", "FAKE-AUTHORIZED");
    sb.put(".gnupg/secring.gpg", "FAKE-SECRING");
    sb.put(".ssh-backup/notes.txt", "not-a-credential");
    sb.put("normal.txt", "plain");
}

#[test]
fn 受保护目录自身与深层子项在所有入口上一致被拒() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);

    for denied in [
        ".ssh",
        ".ssh/id_rsa",
        ".ssh/nested/authorized_keys",
        ".gnupg",
    ] {
        let abs = sb.ws(denied);
        assert!(
            matches!(tb.read(denied, 4096).unwrap_err(), ToolError::Denied(_)),
            "read {denied}"
        );
        assert!(
            matches!(
                tb.read_chunked(denied, 0, 4).unwrap_err(),
                ToolError::Denied(_)
            ),
            "readChunked {denied}"
        );
        assert!(
            matches!(tb.get_info(denied).unwrap_err(), ToolError::Denied(_)),
            "info {denied}"
        );
        assert!(
            matches!(
                tb.list_dir(denied, 0, 10).unwrap_err(),
                ToolError::Denied(_)
            ),
            "list {denied}"
        );
        assert!(
            matches!(
                tb.write(denied, "x", false).unwrap_err(),
                ToolError::Denied(_)
            ),
            "write {denied}"
        );
        assert!(
            matches!(tb.make_dir(denied).unwrap_err(), ToolError::Denied(_)),
            "makeDir {denied}"
        );
        assert!(
            matches!(tb.remove(denied).unwrap_err(), ToolError::Denied(_)),
            "remove {denied}"
        );
        assert!(
            matches!(
                tb.rename(denied, "moved.txt").unwrap_err(),
                ToolError::Denied(_)
            ),
            "rename src {denied}"
        );
        assert!(
            matches!(
                tb.rename("normal.txt", denied).unwrap_err(),
                ToolError::Denied(_)
            ),
            "rename dst {denied}"
        );
        assert!(
            matches!(
                tb.search(denied, "*", 10).unwrap_err(),
                ToolError::Denied(_)
            ),
            "search {denied}"
        );
        assert!(
            matches!(tb.glob(denied, "**", 10).unwrap_err(), ToolError::Denied(_)),
            "glob {denied}"
        );
        assert!(!tb.exists(denied), "exists {denied} 必须为 false");
        assert!(
            matches!(
                tb.write_batch(&[WriteOp::new(denied, "x")]).unwrap_err(),
                ToolError::BatchRolledBack {
                    failed_index: 0,
                    ..
                }
            ),
            "writeBatch {denied}"
        );
        // 绝对路径写法同样被拒。
        assert!(
            matches!(
                tb.read(abs.to_str().unwrap(), 4096).unwrap_err(),
                ToolError::Denied(_)
            ),
            "read abs {denied}"
        );
    }

    // 凭据内容自始至终未被改动，工作区外的普通文件也未被误伤。
    assert_eq!(sb.get(".ssh/id_rsa"), "FAKE-PRIVATE-KEY");
    assert_eq!(sb.get(".gnupg/secring.gpg"), "FAKE-SECRING");
    assert_eq!(sb.get("normal.txt"), "plain");
}

#[test]
fn 同名前缀与普通路径不被误伤() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);

    // `.ssh-backup` 是不同分量，不得被子串/前缀匹配误伤。
    assert!(tb.exists(".ssh-backup"));
    assert_eq!(
        tb.read(".ssh-backup/notes.txt", 4096).unwrap(),
        "not-a-credential"
    );
    assert!(tb.get_info(".ssh-backup").unwrap().is_dir);
    let (entries, total) = tb.list_dir(".ssh-backup", 0, 10).unwrap();
    assert_eq!(total, 1);
    assert_eq!(entries[0].name, "notes.txt");
    tb.write(".ssh-backup/new.txt", "ok", false).unwrap();
    tb.make_dir(".ssh-backup/sub").unwrap();
    tb.rename(".ssh-backup/new.txt", ".ssh-backup/renamed.txt")
        .unwrap();
    tb.remove(".ssh-backup/renamed.txt").unwrap();
    assert!(!sb.ws(".ssh-backup/renamed.txt").exists());

    // 普通路径照常。
    assert_eq!(tb.read("normal.txt", 4096).unwrap(), "plain");
    let (hits, _) = tb.search(".", "*.txt", 10).unwrap();
    assert!(hits.iter().any(|h| h.rel_path == "normal.txt"));
}

#[test]
fn 递归搜索与列举不进入禁区() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);

    // 从工作区根搜索：禁区内的文件名不得出现，同名前缀兄弟目录照常出现。
    let (hits, total) = tb.search(".", "*", 100).unwrap();
    let rels: Vec<&str> = hits.iter().map(|h| h.rel_path.as_str()).collect();
    assert!(
        !rels.iter().any(|p| p.starts_with(".ssh/")
            || p.starts_with(".gnupg/")
            || *p == ".ssh"
            || *p == ".gnupg"),
        "搜索结果泄漏禁区内容: {rels:?}"
    );
    assert!(
        rels.contains(&".ssh-backup/notes.txt"),
        "同名前缀目录应照常命中: {rels:?}"
    );
    assert_eq!(total, hits.len());

    let globbed = tb.glob(".", "**/*", 100).unwrap();
    assert!(
        !globbed.iter().any(|p| p.starts_with(".ssh/")
            || p.starts_with(".gnupg/")
            || p == ".ssh"
            || p == ".gnupg"),
        "glob 结果泄漏禁区内容: {globbed:?}"
    );
    assert!(globbed.contains(&".ssh-backup/notes.txt".to_string()));

    // 列举工作区根：禁区条目整个不出现（连名字都不泄漏）。
    let (entries, total) = tb.list_dir(".", 0, 100).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(
        !names.contains(&".ssh") && !names.contains(&".gnupg"),
        "列举泄漏禁区名: {names:?}"
    );
    assert!(names.contains(&".ssh-backup"));
    assert_eq!(total, entries.len());
}

#[test]
fn 批量写入撞禁区时整体回滚无部分副作用() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    creds(&sb);
    sb.put("first.txt", "OLD-1");
    sb.put("second.txt", "OLD-2");

    let ops = vec![
        WriteOp::new("first.txt", "NEW-1"),
        WriteOp::new("created.txt", "NEW-FILE"),
        WriteOp::new(".ssh/authorized_keys", "evil"),
        WriteOp::new("second.txt", "NEW-2"),
    ];
    let err = tb.write_batch(&ops).unwrap_err();
    match &err {
        ToolError::BatchRolledBack { failed_index, .. } => assert_eq!(*failed_index, 2),
        other => panic!("期望 BatchRolledBack(failed_index=2)，实际 {other:?}"),
    }
    assert_eq!(sb.get("first.txt"), "OLD-1", "已改文件必须回滚");
    assert!(!sb.ws("created.txt").exists(), "组内新建文件必须撤销");
    assert_eq!(sb.get("second.txt"), "OLD-2", "失败点之后的写入不得发生");
    assert_eq!(
        sb.get(".ssh/nested/authorized_keys"),
        "FAKE-AUTHORIZED",
        "凭据不得被覆盖"
    );
    assert_eq!(sb.get(".ssh/id_rsa"), "FAKE-PRIVATE-KEY");
    assert!(!sb.ws(".ssh/authorized_keys").exists());
}

#[test]
fn 工作区根本身仍是合法目标() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    assert!(tb.exists("."));
    assert!(tb.get_info(".").unwrap().is_dir);
    assert!(tb.list_dir(".", 0, 10).unwrap().1 > 0 || Path::new(".").exists());
    // 工作区根本身仍不允许被删除。
    assert!(matches!(tb.remove(".").unwrap_err(), ToolError::Denied(_)));
}
