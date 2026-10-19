//! `file:write / writeBatch / remove / rename`：原子落盘、防呆、组回滚、回收站、跨盘退化。

use std::fs;
#[cfg(unix)]
use std::path::Path;

use agent_tools::{FsToolbox, ToolError, WriteOp};

mod support;

#[cfg(unix)]
use support::trashed_named;
use support::Sandbox;

#[test]
fn write_新建与覆盖成功_不留临时文件() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);

    tb.write("new.txt", "v1", false).unwrap();
    assert_eq!(sb.get("new.txt"), "v1");
    tb.write("new.txt", "v2", false).unwrap();
    assert_eq!(sb.get("new.txt"), "v2");
    // 嵌套路径需要父目录已存在。
    fs::create_dir_all(sb.ws("deep")).unwrap();
    tb.write("deep/in.txt", "nested", false).unwrap();
    assert_eq!(sb.get("deep/in.txt"), "nested");

    let leftovers: Vec<String> = fs::read_dir(sb.ws("deep"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "原子写后不应残留临时文件：{leftovers:?}"
    );
}

#[test]
#[cfg(unix)]
fn write_覆盖时保留原文件权限位() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    use std::os::unix::fs::PermissionsExt;
    sb.put("script.sh", "#!/bin/sh\n");
    fs::set_permissions(sb.ws("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();

    tb.write("script.sh", "#!/bin/sh\necho hi\n", false)
        .unwrap();
    let mode = fs::metadata(sb.ws("script.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "原子写必须继承原权限");
}

#[test]
fn write_错误路径_parent缺失与目标是目录() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    assert!(matches!(
        tb.write("nope/x.txt", "x", false).unwrap_err(),
        ToolError::NotFound(ref p) if *p == sb.ws("nope")
    ));
    fs::create_dir_all(sb.ws("adir")).unwrap();
    assert!(matches!(
        tb.write("adir", "x", false).unwrap_err(),
        ToolError::AlreadyExists(_)
    ));
    assert!(matches!(
        tb.write("../out.txt", "x", false).unwrap_err(),
        ToolError::Denied(_)
    ));
}

#[test]
fn write_expect_read_未读过则拒绝_读过之后放行() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("doc.md", "old");

    // 旧内容存在但本次未读 ⇒ 防呆拒绝，且原内容未被改动。
    let err = tb.write("doc.md", "new", true).unwrap_err();
    assert!(
        matches!(err, ToolError::Denied(ref p) if *p == sb.ws("doc.md")),
        "实际 {err:?}"
    );
    assert_eq!(sb.get("doc.md"), "old");

    // 读过后放行。
    assert_eq!(tb.read("doc.md", 4096).unwrap(), "old");
    tb.write("doc.md", "new", true).unwrap();
    assert_eq!(sb.get("doc.md"), "new");
}

#[test]
fn write_expect_read_新文件无需先读() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    tb.write("brand-new.txt", "created", true).unwrap();
    assert_eq!(sb.get("brand-new.txt"), "created");

    // 自己写过也算见过。
    tb.write("brand-new.txt", "updated", true).unwrap();
    assert_eq!(sb.get("brand-new.txt"), "updated");

    // 另一个实例（独立「已见」集）仍需先读。
    let tb2 = FsToolbox::new(&sb.workspace);
    assert!(matches!(
        tb2.write("brand-new.txt", "blind", true).unwrap_err(),
        ToolError::Denied(_)
    ));
    assert_eq!(sb.get("brand-new.txt"), "updated");
}

#[test]
fn write_batch_全部成功() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("keep.txt", "old");
    let ops = vec![
        WriteOp::new("keep.txt", "new"),
        WriteOp::new("made.txt", "created"),
        WriteOp::new("keep.txt", "final"),
    ];
    tb.write_batch(&ops).unwrap();
    assert_eq!(sb.get("keep.txt"), "final");
    assert_eq!(sb.get("made.txt"), "created");

    // 空批次直接成功。
    tb.write_batch(&[]).unwrap();
}

#[test]
fn write_batch_第三个失败_前两个已存在文件恢复原内容() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("first.txt", "OLD-1");
    sb.put("second.txt", "OLD-2");

    let ops = vec![
        WriteOp::new("first.txt", "NEW-1"),
        WriteOp::new("second.txt", "NEW-2"),
        WriteOp::new("missing-parent/third.txt", "NEW-3"), // 父目录不存在 ⇒ 失败点
    ];
    let err = tb.write_batch(&ops).unwrap_err();
    match &err {
        ToolError::BatchRolledBack { failed_index, .. } => assert_eq!(*failed_index, 2),
        other => panic!("期望 BatchRolledBack(failed_index=2)，实际 {other:?}"),
    }
    assert_eq!(sb.get("first.txt"), "OLD-1", "回滚必须恢复原内容");
    assert_eq!(sb.get("second.txt"), "OLD-2", "回滚必须恢复原内容");
    assert!(!sb.ws("missing-parent").join("third.txt").exists());
    // 错误文本带上失败原因，便于协议层展示。
    assert!(err.to_string().contains("回滚"), "{err}");
}

#[test]
fn write_batch_失败时删除本组新建的文件() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("old.txt", "OLD");

    let ops = vec![
        WriteOp::new("old.txt", "NEW"),
        WriteOp::new("brand-new.txt", "created"),
        WriteOp::new("../escape.txt", "evil"), // 第 3 个越界 ⇒ 失败点
    ];
    let err = tb.write_batch(&ops).unwrap_err();
    assert!(
        matches!(
            err,
            ToolError::BatchRolledBack {
                failed_index: 2,
                ..
            }
        ),
        "实际 {err:?}"
    );
    assert_eq!(sb.get("old.txt"), "OLD");
    assert!(!sb.ws("brand-new.txt").exists(), "组内新建文件必须一并撤销");
    assert!(!sb.workspace.join("../escape.txt").exists());
}

#[test]
fn write_batch_组内重复写同一路径_回滚回到批量前内容() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("loop.txt", "ORIGINAL");

    let ops = vec![
        WriteOp::new("loop.txt", "STEP-1"),
        WriteOp::new("loop.txt", "STEP-2"),
        WriteOp::new("no-parent/x.txt", "boom"),
    ];
    assert!(matches!(
        tb.write_batch(&ops).unwrap_err(),
        ToolError::BatchRolledBack {
            failed_index: 2,
            ..
        }
    ));
    assert_eq!(sb.get("loop.txt"), "ORIGINAL");
}

// 回收站两条用例依赖 freedesktop 布局（XDG_DATA_HOME + files/ + info/*.trashinfo，
// 见 support/mod.rs），Windows 的 trash 实现不使用该布局 ⇒ 仅 Unix 运行。
#[cfg(unix)]
#[test]
fn remove_落回收站而非直删() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    // 触发 XDG_DATA_HOME 隔离设置（必须在删除前）。
    let trash_files = support::trash_files();
    fs::create_dir_all(&trash_files).unwrap();
    let before = support::trash_entry_count();

    sb.put("trash-me.txt", "content");
    tb.remove("trash-me.txt").unwrap();
    assert!(!sb.ws("trash-me.txt").exists(), "工作区内文件应已移走");
    assert!(
        trashed_named("trash-me.txt"),
        "回收站 files/ 或 info/ 应保留原名条目，实际目录 {trash_files:?}"
    );
    assert!(support::trash_entry_count() > before, "回收站条目数应增加");
}

#[cfg(unix)]
#[test]
fn remove_目录同样进回收站_缺失与越界分别报错() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    support::trash_files();
    fs::create_dir_all(sb.ws("dir-to-trash/sub")).unwrap();
    sb.put("dir-to-trash/sub/inner.txt", "x");
    tb.remove("dir-to-trash").unwrap();
    assert!(!sb.ws("dir-to-trash").exists());
    assert!(trashed_named("dir-to-trash"));

    assert!(matches!(
        tb.remove("already-gone.txt").unwrap_err(),
        ToolError::NotFound(_)
    ));
    assert!(matches!(
        tb.remove("../outside/secret.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    // 工作区自身不允许删除。
    assert!(matches!(tb.remove(".").unwrap_err(), ToolError::Denied(_)));
}

#[test]
fn rename_同盘成功且不覆盖目标() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    sb.put("from.txt", "payload");
    tb.rename("from.txt", "to.txt").unwrap();
    assert!(!sb.ws("from.txt").exists());
    assert_eq!(sb.get("to.txt"), "payload");

    // 目标已存在 ⇒ TargetExists，源保留。
    sb.put("taken.txt", "other");
    let err = tb.rename("to.txt", "taken.txt").unwrap_err();
    assert!(
        matches!(err, ToolError::TargetExists(ref p) if *p == sb.ws("taken.txt")),
        "实际 {err:?}"
    );
    assert_eq!(sb.get("to.txt"), "payload");

    // 源不存在 ⇒ NotFound；目录改名可用。
    assert!(matches!(
        tb.rename("ghost.txt", "x.txt").unwrap_err(),
        ToolError::NotFound(_)
    ));
    fs::create_dir_all(sb.ws("dsrc")).unwrap();
    tb.rename("dsrc", "ddst").unwrap();
    assert!(sb.ws("ddst").is_dir());
    // 目标父目录不存在 ⇒ NotFound。
    assert!(matches!(
        tb.rename("to.txt", "no-dir/x.txt").unwrap_err(),
        ToolError::NotFound(_)
    ));
    // 越界 ⇒ Denied。
    assert!(matches!(
        tb.rename("to.txt", "../out.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    assert!(matches!(
        tb.rename("../out.txt", "to.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
}

/// 经符号链接把文件改名/写出到工作区外必须 fail-closed。
/// （解析闸上线前，该写法会跟随链接落到区外完成 EXDEV 退化复制。）
/// 跨盘退化机械（copy_tree / remove_tree / EXDEV 判定）由 fs_ops 内单测覆盖。
#[test]
#[cfg(unix)]
fn rename_拒绝经符号链接越出工作区() {
    let sb = Sandbox::new();
    let tb = FsToolbox::new(&sb.workspace);
    let external = sb.outside.join("exdev-mount");
    fs::create_dir_all(&external).expect("创建区外目录失败");
    fs::symlink_metadata(&external).unwrap();
    let link = sb.ws("mnt");
    std::os::unix::fs::symlink(&external, &link).expect("创建符号链接失败");

    sb.put("cross.txt", "cross-device payload");
    assert!(matches!(
        tb.rename("cross.txt", "mnt/cross.txt").unwrap_err(),
        ToolError::Denied(_)
    ));
    // 源保留、区外目标未出现。
    assert_eq!(
        fs::read_to_string(sb.ws("cross.txt")).unwrap(),
        "cross-device payload"
    );
    assert!(!Path::new(&external).join("cross.txt").exists());
}
