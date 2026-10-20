//! `memory.search` 后端：记忆文件子串检索。
//!
//! 转写自 codex `codex-rs/ext/memories/src/local/search.rs`：`SearchMatcher` /
//! `SearchComparison` 分层、三种匹配模式、窗口去重、结果排序与 cursor 分页一致；
//! 检索只用标准库 `str::contains` / `to_lowercase`，不引入 regex。路径解析复用
//! [`super::list::resolve_memory_path`]（`ensure_within` 是唯一越界防护实现点）。

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::paths::MemoryPaths;

use super::list::{
    display_path, is_hidden_path, metadata_or_none, read_sorted_dir_entries, reject_symlink,
    resolve_memory_path, ResolvedPath,
};

/// `memory.search` 默认返回条数（照抄 codex `lib.rs:13`）。
pub const DEFAULT_SEARCH_MAX_RESULTS: usize = 200;
/// `memory.search` 上限（照抄 codex `lib.rs:14`）。
pub const MAX_SEARCH_RESULTS: usize = 200;

/// 匹配模式（serde 形状照抄 codex：`type` 标签 + snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchMatchMode {
    Any,
    AllOnSameLine,
    AllWithinLines { line_count: usize },
}

/// `memory.search` 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMemoriesRequest {
    pub queries: Vec<String>,
    pub match_mode: SearchMatchMode,
    pub path: Option<String>,
    pub cursor: Option<String>,
    pub context_lines: usize,
    pub case_sensitive: bool,
    pub normalized: bool,
    pub max_results: usize,
}

/// `memory.search` 响应（字段名照抄 codex）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchMemoriesResponse {
    pub queries: Vec<String>,
    pub match_mode: SearchMatchMode,
    pub path: Option<String>,
    pub matches: Vec<MemorySearchMatch>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

/// 单条命中（含上下文行与命中的查询项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemorySearchMatch {
    pub path: String,
    pub match_line_number: usize,
    pub content_start_line_number: usize,
    pub content: String,
    pub matched_queries: Vec<String>,
}

/// `memory.search`：在 `path` 指向的文件或目录（递归）中做子串检索。
pub fn search(
    paths: &MemoryPaths,
    request: SearchMemoriesRequest,
) -> Result<SearchMemoriesResponse, MemoryError> {
    let queries = request
        .queries
        .iter()
        .map(|query| query.trim().to_string())
        .collect::<Vec<_>>();
    if queries.is_empty() || queries.iter().any(String::is_empty) {
        return Err(MemoryError::EmptyQuery);
    }
    if matches!(
        request.match_mode,
        SearchMatchMode::AllWithinLines { line_count: 0 }
    ) {
        return Err(MemoryError::InvalidMatchWindow);
    }

    let max_results = request.max_results.min(MAX_SEARCH_RESULTS);
    let resolved = resolve_memory_path(paths, request.path.as_deref())?;
    let start_index = match request.cursor.as_deref() {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| MemoryError::InvalidCursor {
                cursor: cursor.to_string(),
            })?,
        None => 0,
    };
    let Some(metadata) = metadata_or_none(&resolved.full)? else {
        return Err(MemoryError::NotFound {
            path: request.path.unwrap_or_default(),
        });
    };
    reject_symlink(&resolved.full, &metadata)?;

    let matcher = SearchMatcher::new(
        queries.clone(),
        request.match_mode.clone(),
        request.case_sensitive,
        request.normalized,
    )?;
    let mut matches = Vec::new();
    search_entries(
        &resolved,
        &resolved.full,
        &metadata,
        &matcher,
        request.context_lines,
        &mut matches,
    )?;
    matches.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.match_line_number.cmp(&right.match_line_number))
    });
    if start_index > matches.len() {
        return Err(MemoryError::InvalidCursor {
            cursor: start_index.to_string(),
        });
    }
    let end_index = start_index.saturating_add(max_results).min(matches.len());
    let next_cursor = (end_index < matches.len()).then(|| end_index.to_string());
    let truncated = next_cursor.is_some();
    Ok(SearchMemoriesResponse {
        queries,
        match_mode: request.match_mode,
        path: request.path,
        matches: matches.drain(start_index..end_index).collect(),
        next_cursor,
        truncated,
    })
}

/// 入口分派：文件直接检索；目录用显式栈递归（跳过隐藏名与 symlink）。
fn search_entries(
    resolved: &ResolvedPath,
    current: &Path,
    current_metadata: &fs::Metadata,
    matcher: &SearchMatcher,
    context_lines: usize,
    matches: &mut Vec<MemorySearchMatch>,
) -> Result<(), MemoryError> {
    if current_metadata.is_file() {
        search_file(resolved, current, matcher, context_lines, matches)?;
        return Ok(());
    }
    if !current_metadata.is_dir() {
        return Ok(());
    }

    let mut pending = vec![current.to_path_buf()];
    while let Some(dir_path) = pending.pop() {
        for (entry_path, _) in read_sorted_dir_entries(&dir_path)? {
            if is_hidden_path(&entry_path) {
                continue;
            }
            let Some(metadata) = metadata_or_none(&entry_path)? else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry_path);
            } else if metadata.is_file() {
                search_file(resolved, &entry_path, matcher, context_lines, matches)?;
            }
        }
    }

    Ok(())
}

/// 单文件检索：逐行求查询命中位图，再按匹配模式组装结果。
fn search_file(
    resolved: &ResolvedPath,
    path: &Path,
    matcher: &SearchMatcher,
    context_lines: usize,
    matches: &mut Vec<MemorySearchMatch>,
) -> Result<(), MemoryError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        // 非 UTF-8 记忆文件无法检索 ⇒ 跳过（与 codex 同口径）。
        Err(err) if err.kind() == std::io::ErrorKind::InvalidData => return Ok(()),
        Err(err) => return Err(super::list::io_err(path, err)),
    };
    let lines = content.lines().collect::<Vec<_>>();
    let line_matches = lines
        .iter()
        .map(|line| matcher.matched_query_flags(line))
        .collect::<Vec<_>>();
    match &matcher.match_mode {
        SearchMatchMode::Any => {
            for (index, matched_query_flags) in line_matches.iter().enumerate() {
                if matched_query_flags.iter().any(|matched| *matched) {
                    matches.push(build_search_match(
                        resolved,
                        path,
                        &lines,
                        index,
                        index,
                        context_lines,
                        matcher.matched_queries(matched_query_flags),
                    ));
                }
            }
        }
        SearchMatchMode::AllOnSameLine => {
            for (index, matched_query_flags) in line_matches.iter().enumerate() {
                if matched_query_flags.iter().all(|matched| *matched) {
                    matches.push(build_search_match(
                        resolved,
                        path,
                        &lines,
                        index,
                        index,
                        context_lines,
                        matcher.matched_queries(matched_query_flags),
                    ));
                }
            }
        }
        SearchMatchMode::AllWithinLines { line_count } => {
            let mut windows = Vec::new();
            for start_index in 0..lines.len() {
                if !line_matches[start_index].iter().any(|matched| *matched) {
                    continue;
                }
                let last_allowed_index = start_index
                    .saturating_add(line_count.saturating_sub(1))
                    .min(lines.len().saturating_sub(1));
                let mut matched_query_flags = vec![false; matcher.queries.len()];
                for (end_index, line_match_flags) in line_matches
                    .iter()
                    .enumerate()
                    .take(last_allowed_index + 1)
                    .skip(start_index)
                {
                    for (index, matched) in line_match_flags.iter().enumerate() {
                        matched_query_flags[index] |= matched;
                    }
                    if matched_query_flags.iter().all(|matched| *matched) {
                        windows.push((start_index, end_index, matched_query_flags));
                        break;
                    }
                }
            }
            for (index, (start_index, end_index, matched_query_flags)) in windows.iter().enumerate()
            {
                let strictly_contains_another_window =
                    windows
                        .iter()
                        .enumerate()
                        .any(|(other_index, (other_start, other_end, _))| {
                            index != other_index
                                && start_index <= other_start
                                && end_index >= other_end
                                && (start_index != other_start || end_index != other_end)
                        });
                if strictly_contains_another_window {
                    continue;
                }
                matches.push(build_search_match(
                    resolved,
                    path,
                    &lines,
                    *start_index,
                    *end_index,
                    context_lines,
                    matcher.matched_queries(matched_query_flags),
                ));
            }
        }
    }
    Ok(())
}

/// 组装单条命中：上下文行窗口 + 展示路径 + 命中查询项。
fn build_search_match(
    resolved: &ResolvedPath,
    path: &Path,
    lines: &[&str],
    match_start_index: usize,
    match_end_index: usize,
    context_lines: usize,
    matched_queries: Vec<String>,
) -> MemorySearchMatch {
    let content_start_index = match_start_index.saturating_sub(context_lines);
    let content_end_index = match_end_index
        .saturating_add(context_lines)
        .saturating_add(1)
        .min(lines.len());
    MemorySearchMatch {
        path: display_path(resolved, path),
        match_line_number: match_start_index + 1,
        content_start_line_number: content_start_index + 1,
        content: lines[content_start_index..content_end_index].join("\n"),
        matched_queries,
    }
}

/// 查询准备与逐行匹配（大小写 / 归一化口径照抄 codex）。
struct SearchMatcher {
    queries: Vec<String>,
    prepared_queries: Vec<String>,
    comparison: SearchComparison,
    match_mode: SearchMatchMode,
}

impl SearchMatcher {
    fn new(
        queries: Vec<String>,
        match_mode: SearchMatchMode,
        case_sensitive: bool,
        normalized: bool,
    ) -> Result<Self, MemoryError> {
        let comparison = SearchComparison::new(case_sensitive, normalized);
        let prepared_queries = queries
            .iter()
            .map(|query| comparison.prepare(query))
            .map(Cow::into_owned)
            .collect::<Vec<_>>();
        // 归一化可能把查询清空（如 `--`）⇒ 与空查询同错。
        if prepared_queries.iter().any(String::is_empty) {
            return Err(MemoryError::EmptyQuery);
        }
        Ok(Self {
            queries,
            prepared_queries,
            comparison,
            match_mode,
        })
    }

    fn matched_query_flags(&self, line: &str) -> Vec<bool> {
        let line = self.comparison.prepare(line);
        self.prepared_queries
            .iter()
            .map(|query| line.as_ref().contains(query))
            .collect()
    }

    fn matched_queries(&self, matched_query_flags: &[bool]) -> Vec<String> {
        self.queries
            .iter()
            .zip(matched_query_flags)
            .filter_map(|(query, matched)| matched.then_some(query.clone()))
            .collect()
    }
}

/// 大小写 / 归一化比较口径（照抄 codex `SearchComparison`）。
#[derive(Clone, Copy)]
struct SearchComparison {
    case_sensitive: bool,
    normalized: bool,
}

impl SearchComparison {
    fn new(case_sensitive: bool, normalized: bool) -> Self {
        Self {
            case_sensitive,
            normalized,
        }
    }

    fn prepare<'a>(self, value: &'a str) -> Cow<'a, str> {
        if self.case_sensitive && !self.normalized {
            return Cow::Borrowed(value);
        }

        let value = if self.case_sensitive {
            Cow::Borrowed(value)
        } else {
            Cow::Owned(value.to_lowercase())
        };
        if !self.normalized {
            return value;
        }

        Cow::Owned(
            value
                .chars()
                .filter(|ch| ch.is_alphanumeric())
                .collect::<String>(),
        )
    }
}
