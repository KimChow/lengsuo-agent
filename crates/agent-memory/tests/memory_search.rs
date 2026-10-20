//! 记忆检索三工具后端验收（h2-memory-search）。
//!
//! 覆盖越界拒绝（`..` / 绝对路径 / symlink 逃逸）、上限夹取、空查询拒绝、
//! `case_sensitive` 默认值、游标翻页、`read` 截断，以及 `user:` / `project:`
//! 双根前缀语义。
//!
//! schema 层用例（`deny_unknown_fields`、`max_results = 0`、`queries` 空数组）
//! 放在 `crates/agent-core/tests/memory_search_tools.rs`：本 crate 的 Cargo.toml
//! 属共享区不可改，无法为集成测试引入 `serde_json`。

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_memory::list::{list, ListMemoriesRequest};
use agent_memory::read::{read, ReadMemoryRequest};
use agent_memory::search::{search, SearchMatchMode, SearchMemoriesRequest};
use agent_memory::{MemoryError, MemoryPaths, MAX_SEARCH_RESULTS};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lengsuo-agent-memory-search-{pid}-{id}-{tag}",
            pid = std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("临时目录可创建");
        Self { path }
    }

    fn paths(&self) -> MemoryPaths {
        MemoryPaths::new(self.path.join("home"), self.path.join("ws"))
    }

    /// 写入项目记忆根下的相对路径（自动建父目录）。
    fn write_project(&self, relative: &str, text: &str) {
        let file = self.paths().project_memory_dir().join(relative);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).expect("父目录可创建");
        }
        fs::write(file, text).expect("测试文件可写");
    }

    /// 写入用户记忆根下的相对路径（自动建父目录）。
    fn write_user(&self, relative: &str, text: &str) {
        let file = self.paths().user_memory_root().join(relative);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).expect("父目录可创建");
        }
        fs::write(file, text).expect("测试文件可写");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn search_request(
    queries: &[&str],
    path: Option<&str>,
    max_results: usize,
) -> SearchMemoriesRequest {
    SearchMemoriesRequest {
        queries: queries.iter().map(|query| (*query).to_string()).collect(),
        match_mode: SearchMatchMode::Any,
        path: path.map(str::to_string),
        cursor: None,
        context_lines: 0,
        case_sensitive: true,
        normalized: false,
        max_results,
    }
}

fn list_request(path: Option<&str>) -> ListMemoriesRequest {
    ListMemoriesRequest {
        path: path.map(str::to_string),
        cursor: None,
        max_results: 50,
    }
}

fn read_request(path: &str) -> ReadMemoryRequest {
    ReadMemoryRequest {
        path: path.to_string(),
        line_offset: 1,
        max_lines: None,
        max_tokens: 20_000,
    }
}

// ---------------------------------------------------------------- 越界拒绝

#[test]
fn traversing_and_absolute_paths_are_rejected() {
    let dir = TempDir::new("lexical-escape");
    fs::create_dir_all(dir.path.join("outside")).expect("外部目录");
    fs::write(dir.path.join("outside").join("secret.md"), "外部机密").expect("外部文件");
    let paths = dir.paths();

    for bad in [
        "../outside",
        "project:../outside",
        "user:../../outside",
        // 归一化后仍在根内的 `..` 也按 codex 词法预检拒绝。
        "nested/../MEMORY.md",
        "/etc/passwd",
        "project:/etc/passwd",
    ] {
        let listed = list(&paths, list_request(Some(bad)));
        assert!(
            matches!(listed, Err(MemoryError::PathEscape)),
            "list({bad}) 必须 PathEscape，实得 {listed:?}"
        );

        let searched = search(&paths, search_request(&["外部机密"], Some(bad), 10));
        assert!(
            matches!(searched, Err(MemoryError::PathEscape)),
            "search({bad}) 必须 PathEscape，实得 {searched:?}"
        );

        let read_back = read(&paths, read_request(bad));
        assert!(
            matches!(read_back, Err(MemoryError::PathEscape)),
            "read({bad}) 必须 PathEscape，实得 {read_back:?}"
        );
    }
}

/// symlink 指向记忆根外 ⇒ 起点路径本身被 `ensure_within` 拒绝；
/// 即便从根目录枚举/检索，也不得跟随 symlink 泄露外部内容。
#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_and_never_traversed() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new("symlink-escape");
    let outside = dir.path.join("outside");
    fs::create_dir_all(&outside).expect("外部目录");
    fs::write(outside.join("secret.md"), "外部机密 needle").expect("外部文件");
    fs::create_dir_all(dir.paths().project_memory_dir()).expect("记忆根");
    symlink(&outside, dir.paths().project_memory_dir().join("link")).expect("建立 symlink");
    let paths = dir.paths();

    let listed = list(&paths, list_request(Some("link")));
    assert!(
        matches!(listed, Err(MemoryError::PathEscape)),
        "list 穿过逃逸 symlink 必须 PathEscape，实得 {listed:?}"
    );

    let read_back = read(&paths, read_request("link/secret.md"));
    assert!(
        matches!(read_back, Err(MemoryError::PathEscape)),
        "read 穿过逃逸 symlink 必须 PathEscape，实得 {read_back:?}"
    );

    let searched = search(&paths, search_request(&["外部机密"], Some("link"), 10));
    assert!(
        matches!(searched, Err(MemoryError::PathEscape)),
        "search 穿过逃逸 symlink 必须 PathEscape，实得 {searched:?}"
    );

    // 从根枚举/检索时按 codex 口径跳过 symlink 子项，不泄露外部内容。
    let root_list = list(&paths, list_request(None)).expect("根目录可列");
    assert!(
        !root_list
            .entries
            .iter()
            .any(|entry| entry.path.contains("link")),
        "根枚举不得返回 symlink 子项：{:?}",
        root_list.entries
    );
    let root_search = search(&paths, search_request(&["外部机密"], None, 10)).expect("根检索");
    assert!(
        root_search.matches.is_empty(),
        "根检索不得跟随 symlink 读到外部内容：{:?}",
        root_search.matches
    );
}

// ---------------------------------------------------------------- 上限夹取

#[test]
fn max_results_is_clamped_to_max_search_results() {
    let dir = TempDir::new("clamp-max");
    let total = MAX_SEARCH_RESULTS + 7;
    let mut content = String::new();
    for _ in 0..total {
        content.push_str("needle\n");
    }
    dir.write_project("MEMORY.md", &content);

    let response =
        search(&dir.paths(), search_request(&["needle"], None, 99_999)).expect("检索成功");

    assert_eq!(response.matches.len(), MAX_SEARCH_RESULTS);
    assert!(response.truncated);
    assert_eq!(
        response.next_cursor.as_deref(),
        Some(MAX_SEARCH_RESULTS.to_string().as_str())
    );
}

// ---------------------------------------------------------------- 空查询

#[test]
fn empty_or_blank_queries_are_rejected() {
    let dir = TempDir::new("empty-queries");
    dir.write_project("MEMORY.md", "needle\n");

    for queries in [vec![], vec![""], vec!["  "], vec!["needle", "\t"]] {
        let mut request = search_request(&["needle"], None, 10);
        request.queries = queries.iter().map(|query| (*query).to_string()).collect();
        let result = search(&dir.paths(), request);
        assert!(
            matches!(result, Err(MemoryError::EmptyQuery)),
            "queries={queries:?} 必须 EmptyQuery，实得 {result:?}"
        );
    }
}

// ---------------------------------------------------------------- 大小写默认

#[test]
fn case_sensitive_defaults_to_true_and_can_be_disabled() {
    let dir = TempDir::new("case-sensitive");
    dir.write_project("MEMORY.md", "Rust 网络面\n");
    let paths = dir.paths();

    let sensitive = search(&paths, search_request(&["rust"], None, 10)).expect("检索成功");
    assert!(
        sensitive.matches.is_empty(),
        "默认 case_sensitive=true，rust 不应命中 Rust：{:?}",
        sensitive.matches
    );

    let mut insensitive = search_request(&["rust"], None, 10);
    insensitive.case_sensitive = false;
    let matched = search(&paths, insensitive).expect("检索成功");
    assert_eq!(matched.matches.len(), 1);
    assert_eq!(matched.matches[0].path, "project:MEMORY.md");
    assert_eq!(matched.matches[0].match_line_number, 1);
}

// ---------------------------------------------------------------- 游标翻页

#[test]
fn cursor_pagination_has_no_overlap_and_preserves_total() {
    let dir = TempDir::new("cursor");
    let total = MAX_SEARCH_RESULTS + 23;
    let mut content = String::new();
    for _ in 0..total {
        content.push_str("needle\n");
    }
    dir.write_project("MEMORY.md", &content);
    let paths = dir.paths();

    let first = search(
        &paths,
        search_request(&["needle"], None, MAX_SEARCH_RESULTS),
    )
    .expect("第一页");
    assert_eq!(first.matches.len(), MAX_SEARCH_RESULTS);
    let cursor = first.next_cursor.clone().expect("应给出下一页游标");

    let mut second_request = search_request(&["needle"], None, MAX_SEARCH_RESULTS);
    second_request.cursor = Some(cursor);
    let second = search(&paths, second_request).expect("第二页");
    assert_eq!(second.matches.len(), total - MAX_SEARCH_RESULTS);
    assert!(second.next_cursor.is_none(), "第二页应为末页");

    let first_lines: BTreeSet<usize> = first
        .matches
        .iter()
        .map(|matched| matched.match_line_number)
        .collect();
    let second_lines: BTreeSet<usize> = second
        .matches
        .iter()
        .map(|matched| matched.match_line_number)
        .collect();
    assert!(first_lines.is_disjoint(&second_lines), "两页不得重叠");
    assert_eq!(first_lines.len() + second_lines.len(), total, "总数守恒");
}

// ---------------------------------------------------------------- read 截断

#[test]
fn read_truncates_content_beyond_the_budget() {
    let dir = TempDir::new("read-truncate");
    let content = (0..1_000)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    dir.write_project("MEMORY.md", &content);

    let truncated = read(
        &dir.paths(),
        ReadMemoryRequest {
            path: "project:MEMORY.md".to_string(),
            line_offset: 1,
            max_lines: None,
            max_tokens: 100,
        },
    )
    .expect("读取成功");

    assert!(truncated.truncated, "超过 max_tokens 必须标记 truncated");
    assert!(
        truncated.content.chars().count() <= 100,
        "内容必须被截到预算内，实际 {} 字符",
        truncated.content.chars().count()
    );
    assert_ne!(truncated.content, content);
    assert_eq!(truncated.start_line_number, 1);
    assert_eq!(truncated.path, "project:MEMORY.md");

    let full = read(&dir.paths(), read_request("project:MEMORY.md")).expect("完整读取");
    assert!(!full.truncated);
    assert_eq!(full.content, content);
}

#[test]
fn read_line_offset_and_max_lines_are_one_indexed() {
    let dir = TempDir::new("read-window");
    dir.write_project("MEMORY.md", "l1\nl2\nl3\nl4");

    let response = read(
        &dir.paths(),
        ReadMemoryRequest {
            path: "MEMORY.md".to_string(),
            line_offset: 2,
            max_lines: Some(2),
            max_tokens: 20_000,
        },
    )
    .expect("读取成功");

    assert_eq!(response.start_line_number, 2);
    // 与 codex `line_end_byte_offset` 同口径：窗口含末行自身的换行符。
    assert_eq!(response.content, "l2\nl3\n");
}

// ---------------------------------------------------------------- 双根前缀

#[test]
fn user_and_project_prefixes_select_separate_roots() {
    let dir = TempDir::new("prefixes");
    dir.write_project("MEMORY.md", "项目正文");
    dir.write_user("MEMORY.md", "用户正文");
    let paths = dir.paths();

    // 省略 path ⇒ 默认项目根（codex 单根语义在本仓的落点）。
    let default_list = list(&paths, list_request(None)).expect("默认列目录");
    assert_eq!(default_list.entries.len(), 1);
    assert_eq!(default_list.entries[0].path, "project:MEMORY.md");

    let user_list = list(&paths, list_request(Some("user:"))).expect("用户根列目录");
    assert_eq!(user_list.entries.len(), 1);
    assert_eq!(user_list.entries[0].path, "user:MEMORY.md");

    let user_read = read(&paths, read_request("user:MEMORY.md")).expect("用户记忆可读");
    assert_eq!(user_read.content, "用户正文");
    assert_eq!(user_read.path, "user:MEMORY.md");

    let project_read = read(&paths, read_request("MEMORY.md")).expect("项目记忆可读");
    assert_eq!(project_read.content, "项目正文");
    assert_eq!(project_read.path, "project:MEMORY.md");
}
