//! `fs.*` 接线自检：spec 清单 + 适配层（**不经过 registry**）。
//!
//! 分工：本卡（`x-fs-wire`）只产出两个源文件——`tool_fs.rs`（spec 清单）与
//! `fs_adapter.rs`（参数 → `agent-tools` 的翻译）。「把工具注册进 registry」由
//! 汇总卡 `x-tool-registry` 负责（它独占 `tool.rs` / `executor.rs`）。
//! ⇒ 本文件**不**断言 `default_registry()` 含 `fs.*`：那一条由汇总卡守，
//! 本卡若也断言就会在汇总卡合入前必然变红。
//!
//! 挂载方式：`fs_adapter.rs` **已在 crate 本体**（`lib.rs` 的 `pub mod
//! fs_adapter;`）⇒ 这里直接引 `agent_core::fs_adapter::…`，验的是**本体**而不是
//! 影子副本。`tool_fs.rs` 仍未进 crate（属 `x-fs-wire` 的落点，本卡不改），故
//! 仍用 `#[path]` 就地挂载；`crate::tool` 由 `pub use` 转发到 `agent_core`，
//! 与库内构建解析到**同一**模块与 trait（不是另造一份定义）。

pub use agent_core::tool;

#[path = "../src/tool_fs.rs"]
pub mod tool_fs;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;
use tempfile::TempDir;

use agent_core::fs_adapter::FsToolExecutor;
use agent_core::tool::ToolKind;
use tool_fs::tool_specs;

/// 全部六个文件能力名（含 `fs.delete`）——注册面覆盖读/写两类。
const FS_ALL_NAMES: [&str; 6] = [
    "fs.read",
    "fs.write",
    "fs.list",
    "fs.glob",
    "fs.search",
    "fs.delete",
];

/// 汇总卡要把本适配器装进 `DispatchToolExecutor`（槽位类型是
/// `Arc<dyn ToolExecutor + Send + Sync>`）⇒ 适配器必须满足该自动 trait 组合。
/// 这里用编译期断言守：将来若给适配器加了 `Rc`/`RefCell` 之类非 `Send` 字段，
/// 断言会在编译期炸掉，而不是等汇总卡合入时才暴露。
#[test]
fn 适配器满足汇总卡槽位要求的_send_sync() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<FsToolExecutor>();
    assert_send_sync::<dyn agent_core::r#loop::ToolExecutor + Send + Sync>();
}

/// 临时工作区 + 适配器。`TempDir` 必须与适配器同活（删除即回收目录）。
fn workspace() -> (TempDir, FsToolExecutor) {
    let dir = TempDir::new().expect("临时目录");
    let exec = FsToolExecutor::new(dir.path());
    (dir, exec)
}

/// 把回收站重定向到临时目录（freedesktop 布局：`<XDG_DATA_HOME>/Trash/{files,info}`）。
///
/// `trash` 的 freedesktop 实现在**删除时**才读 `XDG_DATA_HOME`（见 trash 5 的
/// `home_trash()`），因此进程内单点设置即可生效 ⇒ 删除用例不会把文件丢进
/// 开发机真实的 `~/.local/share/Trash`。
///
/// 与 `crates/agent-tools/tests/support/mod.rs` 同一手法（那边是既有实现）。
fn isolated_trash_home() -> &'static Path {
    static TRASH: OnceLock<PathBuf> = OnceLock::new();
    TRASH.get_or_init(|| {
        let dir = TempDir::new().expect("创建回收站临时目录失败").keep();
        std::env::set_var("XDG_DATA_HOME", &dir);
        dir
    })
}

// ---------------------------------------------------------------------------
// 1) spec 清单
// ---------------------------------------------------------------------------

/// spec 清单含五个（六个能力）`fs.*` 工具名，元数据与契约表逐字一致。
#[test]
fn tool_specs_列出fs工具且元数据符合契约() {
    let specs = tool_specs();
    let names: BTreeSet<&str> = specs.iter().map(|(name, ..)| *name).collect();
    let expected: BTreeSet<&str> = FS_ALL_NAMES.into_iter().collect();
    assert_eq!(names, expected, "spec 清单的工具名集合");

    for (name, description, kind, sensitive, timeout) in &specs {
        let (want_kind, want_sensitive, want_timeout) = match *name {
            "fs.read" | "fs.list" | "fs.glob" | "fs.search" => (ToolKind::Read, false, 5_000u64),
            "fs.write" | "fs.delete" => (ToolKind::Write, true, 10_000u64),
            other => panic!("spec 清单出现意外工具名: {other}"),
        };
        assert_eq!(*kind, want_kind, "{name} 的 kind");
        assert_eq!(*sensitive, want_sensitive, "{name} 的 sensitive_arguments");
        assert_eq!(*timeout, want_timeout, "{name} 的 timeout_ms");
        assert!(
            !description.trim().is_empty(),
            "{name} 的描述不得为空（要写清「做什么 + 失败怎么表现」）"
        );
        // 描述必须写明失败形态：越界/不存在一律显式错误，不得静默放行。
        assert!(
            description.contains("错误"),
            "{name} 的描述必须写明失败时返回显式错误: {description}"
        );
    }
}

/// 汇总卡按名字核对 ⇒ 名单必须与契约表逐字相等（含 `fs.delete`）。
#[test]
fn tool_specs_工具名与契约表逐字相等() {
    let names: Vec<&str> = tool_specs().iter().map(|(name, ..)| *name).collect();
    for expected in FS_ALL_NAMES {
        assert!(
            names.contains(&expected),
            "spec 清单缺少 {expected}，实得 {names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2) fs.read 读真实文件
// ---------------------------------------------------------------------------

#[test]
fn fs_read_读真实文件返回内容() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("notes.txt"), "hello fs").expect("写临时文件");

    let out = exec
        .dispatch("fs.read", &json!({"path": "notes.txt"}))
        .expect("读应成功");
    assert_eq!(out["content"], json!("hello fs"));
    assert_eq!(out["path"], json!("notes.txt"));
}

// ---------------------------------------------------------------------------
// 3) fs.read 读不存在文件 ⇒ 显式错误
// ---------------------------------------------------------------------------

#[test]
fn fs_read_读不存在文件返回显式错误() {
    let (_dir, exec) = workspace();
    let err = exec
        .dispatch("fs.read", &json!({"path": "missing.txt"}))
        .expect_err("读不存在文件必须失败");
    assert!(err.contains("不存在"), "错误文本应说明路径不存在: {err}");
}

// ---------------------------------------------------------------------------
// 4) fs.write ⇒ 随后 fs.read 读回
// ---------------------------------------------------------------------------

#[test]
fn fs_write_写文件后fs_read能读回() {
    let (dir, exec) = workspace();
    exec.dispatch(
        "fs.write",
        &json!({"path": "out.txt", "content": "payload"}),
    )
    .expect("写应成功");
    assert!(dir.path().join("out.txt").exists(), "写盘应落盘");

    let out = exec
        .dispatch("fs.read", &json!({"path": "out.txt"}))
        .expect("读回应成功");
    assert_eq!(out["content"], json!("payload"));
}

/// 覆盖已有文件前必须先读过（防盲覆盖）：未读 ⇒ 显式错误；读过 ⇒ 允许。
#[test]
fn fs_write_覆盖已有文件前必须先读过() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("e.txt"), "old").expect("写临时文件");

    let err = exec
        .dispatch("fs.write", &json!({"path": "e.txt", "content": "new"}))
        .expect_err("未读先覆盖必须被拒");
    assert!(err.contains("越权"), "错误文本应说明被防呆拒绝: {err}");
    assert_eq!(
        fs::read_to_string(dir.path().join("e.txt")).expect("读回"),
        "old",
        "被拒的写入不得改动原文件"
    );

    exec.dispatch("fs.read", &json!({"path": "e.txt"}))
        .expect("先读");
    exec.dispatch("fs.write", &json!({"path": "e.txt", "content": "new"}))
        .expect("读过之后允许覆盖");
    assert_eq!(
        fs::read_to_string(dir.path().join("e.txt")).expect("读回"),
        "new"
    );
}

// ---------------------------------------------------------------------------
// 5) 越界路径 ⇒ 显式错误（走 agent-tools 的既有路径守卫）
// ---------------------------------------------------------------------------

#[test]
fn fs_read_越界路径返回显式错误() {
    let (_dir, exec) = workspace();
    let err = exec
        .dispatch("fs.read", &json!({"path": "../escape.txt"}))
        .expect_err("越界路径必须被拒");
    assert!(err.contains("越权"), "错误文本应说明越权被拒: {err}");

    let err = exec
        .dispatch("fs.read", &json!({"path": "/etc/passwd"}))
        .expect_err("工作区外绝对路径必须被拒");
    assert!(err.contains("越权"), "错误文本应说明越权被拒: {err}");
}

/// 越界的写同样被拒（不是只有读受保护）。
#[test]
fn fs_write_越界路径返回显式错误() {
    let (_dir, exec) = workspace();
    let err = exec
        .dispatch(
            "fs.write",
            &json!({"path": "../escape.txt", "content": "x"}),
        )
        .expect_err("越界写入必须被拒");
    assert!(err.contains("越权"), "错误文本应说明越权被拒: {err}");
}

// ---------------------------------------------------------------------------
// 6) fs.list / fs.glob / fs.search / fs.delete
// ---------------------------------------------------------------------------

#[test]
fn fs_list_列出目录条目() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("a.txt"), "x").expect("写临时文件");
    fs::create_dir(dir.path().join("sub")).expect("建目录");

    let out = exec
        .dispatch("fs.list", &json!({"path": "."}))
        .expect("列目录");
    assert_eq!(out["total"], json!(2));
    let entries = out["entries"].as_array().expect("entries 是数组");
    assert_eq!(entries.len(), 2);
}

#[test]
fn fs_glob_按路径模式匹配() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("a.txt"), "x").expect("写临时文件");
    fs::write(dir.path().join("b.md"), "x").expect("写临时文件");

    let out = exec
        .dispatch("fs.glob", &json!({"pattern": "**/*.txt"}))
        .expect("glob");
    let paths = out["paths"].as_array().expect("paths 是数组");
    assert_eq!(paths.len(), 1, "只应匹配 .txt: {paths:?}");
    assert!(paths[0].as_str().expect("字符串").ends_with("a.txt"));
}

#[test]
fn fs_search_按文件名模式匹配() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("a.txt"), "x").expect("写临时文件");

    let out = exec
        .dispatch("fs.search", &json!({"pattern": "*.txt"}))
        .expect("search");
    assert_eq!(out["total"], json!(1));
    let hits = out["hits"].as_array().expect("hits 是数组");
    assert_eq!(hits.len(), 1);
}

#[test]
fn fs_delete_删除文件后工作区内不再存在() {
    let (dir, exec) = workspace();
    fs::write(dir.path().join("d.txt"), "x").expect("写临时文件");

    // 先触发回收站隔离（必须在删除之前设置 XDG_DATA_HOME）。
    let trash = isolated_trash_home().join("Trash/files");
    fs::create_dir_all(&trash).expect("创建回收站 files 目录失败");

    exec.dispatch("fs.delete", &json!({"path": "d.txt"}))
        .expect("删除应成功");
    assert!(!dir.path().join("d.txt").exists());
    // 删除是「移入回收站」而非抹除 ⇒ 回收站里应留有原名条目。
    assert!(
        trash
            .read_dir()
            .expect("读回收站")
            .flatten()
            .any(|e| { e.file_name().to_string_lossy() == "d.txt" }),
        "回收站 files/ 应保留原名条目，实际目录 {trash:?}"
    );
}

/// 删除工作区根目录必须被拒（避免一次调用清掉整个工作区）。
#[test]
fn fs_delete_删除工作区根被拒() {
    let (dir, exec) = workspace();
    isolated_trash_home();
    let err = exec
        .dispatch("fs.delete", &json!({"path": "."}))
        .expect_err("删除工作区根必须被拒");
    assert!(err.contains("越权"), "错误文本应说明越权被拒: {err}");
    assert!(dir.path().exists(), "工作区根不得被删");
}

// ---------------------------------------------------------------------------
// 7) 参数形状错误 ⇒ 显式错误（不 panic、不静默忽略）
// ---------------------------------------------------------------------------

#[test]
fn 参数形状错误返回显式错误() {
    let (_dir, exec) = workspace();
    // fs.read 缺 path
    let err = exec
        .dispatch("fs.read", &json!({}))
        .expect_err("缺 path 必须失败");
    assert!(err.contains("path"), "错误文本应点名缺失参数: {err}");
    // fs.read 的 path 不是字符串
    let err = exec
        .dispatch("fs.read", &json!({"path": 42}))
        .expect_err("path 非字符串必须失败");
    assert!(err.contains("path"), "错误文本应点名错误参数: {err}");
    // 未认领的工具名
    let err = exec
        .dispatch("fs.nuke", &json!({"path": "x"}))
        .expect_err("未认领的工具名必须失败");
    assert!(err.contains("fs.nuke"), "错误文本应含工具名: {err}");
}
