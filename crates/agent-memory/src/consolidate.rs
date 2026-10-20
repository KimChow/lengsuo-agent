//! 30 天日日志蒸馏（纯机械聚合，不含模型调用；规格 §7.2）。
//!
//! 三步固定顺序：
//! 1. **机械聚合**：按文件名日期升序读日文件，抽摘要行，写成 `## YYYY-MM-DD` 分节；
//! 2. **幂等水位**：`CONSOLIDATED.md` 中已存在的日期节跳过不重算（对应 watermark，
//!    不引入 git baseline）；
//! 3. **窗口删除**：**先写后删**；仅删除「超过 30 天且运行前已入册」的日文件，
//!    未入册日文件绝不删（失败时宁可留）。
//!
//! 失败语义 fail-closed：任何日文件读失败 / 聚合文件读失败 / 写失败都直接上抛，
//! 且不删除任何日文件。聚合文件用临时文件 + rename 原子替换。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, NaiveDate, TimeZone};

use crate::caps::CAP_CONSOLIDATED;
use crate::error::MemoryError;
use crate::paths::MemoryPaths;

/// 聚合文件标题（无日期节时文件也只含它）。
const CONSOLIDATED_HEADER: &str = "# 项目记忆蒸馏\n";

/// 删除窗口：`now - date > 30 天` 的日文件才可能被删。
const WINDOW_DAYS: i64 = 30;

/// 一次蒸馏的结果（可观测、可测试；不含文件内容，避免日志泄漏）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidateReport {
    /// 扫描到的日文件数（文件名可解析为日期）。
    pub days_scanned: usize,
    /// 本次新追加的日期节数。
    pub days_appended: usize,
    /// 水位已存在、跳过重算的日期节数。
    pub days_skipped: usize,
    /// 超过 30 天且已入册、被删除的日文件数。
    pub days_pruned: usize,
    /// 聚合文件路径。
    pub consolidated_path: PathBuf,
}

/// 把 `<workspace>/.lengsuo/memory/` 下的日日志压成 `CONSOLIDATED.md`。
///
/// 幂等：相同输入（日文件 + 既有聚合文件）恒得相同输出；已入册日期不重算。
/// `now` 由调用方注入，便于测试 30 天窗口，不读真实时钟。
pub fn consolidate<Tz: TimeZone>(
    workspace: &Path,
    now: DateTime<Tz>,
) -> Result<ConsolidateReport, MemoryError> {
    // home 在本函数不使用（只涉及项目记忆目录），以 workspace 占位。
    let paths = MemoryPaths::new(workspace, workspace);
    let target = paths.consolidated_file();
    MemoryPaths::ensure_within(workspace, &target)?;

    let mut sections = load_existing(&target)?;
    // 删除判据使用**运行前**水位：本次刚聚合进来的旧日文件留给下一次运行，
    // 确保「未入册的日文件绝不删」在同一次运行内也成立。
    let existing_dates: BTreeSet<NaiveDate> = sections.keys().copied().collect();
    let daily = scan_daily_files(&paths, workspace)?;
    let days_scanned = daily.len();
    let mut days_appended = 0usize;
    let mut days_skipped = 0usize;

    for (date, file) in &daily {
        if existing_dates.contains(date) {
            days_skipped += 1;
            continue;
        }
        let lines = summary_lines(file)?;
        if lines.is_empty() {
            // 空日文件不产生空节，也不计入追加（保持「新增节」口径准确）。
            continue;
        }
        sections.insert(*date, lines);
        days_appended += 1;
    }

    // 上限淘汰：按最旧日期节整节淘汰，最新节完整保留（即使单节已超上限）。
    let mut text = render(&sections);
    while text.len() > CAP_CONSOLIDATED && sections.len() > 1 {
        if let Some(oldest) = sections.keys().next().copied() {
            sections.remove(&oldest);
        }
        text = render(&sections);
    }

    // 先写：写失败直接 Err，后面的删除一步都不会执行。
    write_atomic(&target, text.as_bytes())?;

    let kept: BTreeSet<NaiveDate> = sections.keys().copied().collect();
    let today = now.date_naive();
    let mut days_pruned = 0usize;
    for (date, file) in &daily {
        let expired = (today - *date).num_days() > WINDOW_DAYS;
        if !expired || !existing_dates.contains(date) || !kept.contains(date) {
            continue;
        }
        match fs::remove_file(file) {
            Ok(()) => days_pruned += 1,
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(file, e)),
        }
    }

    Ok(ConsolidateReport {
        days_scanned,
        days_appended,
        days_skipped,
        days_pruned,
        consolidated_path: target,
    })
}

/// 读既有聚合文件；不存在视为空。
fn load_existing(path: &Path) -> Result<BTreeMap<NaiveDate, Vec<String>>, MemoryError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(parse_sections(&text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(io_err(path, e)),
    }
}

/// 解析 `## YYYY-MM-DD` 分节；非日期行与标题行忽略，节内相同行只留一次。
fn parse_sections(text: &str) -> BTreeMap<NaiveDate, Vec<String>> {
    let mut sections: BTreeMap<NaiveDate, Vec<String>> = BTreeMap::new();
    let mut current: Option<NaiveDate> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            if let Some(date) = parse_date(rest.trim()) {
                sections.entry(date).or_default();
                current = Some(date);
                continue;
            }
        }
        let Some(date) = current else { continue };
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        push_unique(sections.entry(date).or_default(), trimmed.to_string());
    }
    sections
}

/// 列出日文件并按文件名日期升序；目录不存在 ⇒ 空。
fn scan_daily_files(
    paths: &MemoryPaths,
    workspace: &Path,
) -> Result<Vec<(NaiveDate, PathBuf)>, MemoryError> {
    let dir = paths.project_memory_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(&dir, e)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err(&dir, e))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        let Some(date) = parse_date(stem) else {
            continue;
        };
        // 读/删前统一复核：symlink 逃逸到 workspace 外 ⇒ 显式拒绝。
        MemoryPaths::ensure_within(workspace, &path)?;
        files.push((date, path));
    }
    files.sort_by_key(|(date, _)| *date);
    Ok(files)
}

/// 提取摘要行：
/// - 约定格式 `- HH:MM <一句话摘要>` 的行全部收集（同日多次追加也各算一条）；
/// - 没有任何约定格式行时，回退到**首个非空行**（兼容任意文本追加）；
/// - 节内完全相同的行只留一次。
fn summary_lines(path: &Path) -> Result<Vec<String>, MemoryError> {
    let content = fs::read_to_string(path).map_err(|e| io_err(path, e))?;
    let mut summaries = Vec::new();
    let mut fallback: Option<String> = None;
    for line in content.lines() {
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        if is_summary_line(text) {
            push_unique(&mut summaries, text.to_string());
        } else if fallback.is_none() {
            fallback = Some(normalize_summary(text));
        }
    }
    if summaries.is_empty() {
        if let Some(line) = fallback {
            push_unique(&mut summaries, line);
        }
    }
    Ok(summaries)
}

/// `- HH:MM ...`（两位时:两位分）。
fn is_summary_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("- ") else {
        return false;
    };
    let bytes = rest.as_bytes();
    bytes.len() >= 5
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b':'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
}

/// 统一成 `- ` 开头的行，保证渲染形态一致。
fn normalize_summary(line: &str) -> String {
    if line.starts_with("- ") {
        line.to_string()
    } else {
        format!("- {line}")
    }
}

/// 去重追加（保持首次出现顺序）。
fn push_unique(lines: &mut Vec<String>, line: String) {
    if !lines.contains(&line) {
        lines.push(line);
    }
}

/// 规范化渲染：日期升序、每节 `## 日期` + 摘要行，节间空一行。
fn render(sections: &BTreeMap<NaiveDate, Vec<String>>) -> String {
    let mut out = String::from(CONSOLIDATED_HEADER);
    for (date, lines) in sections {
        out.push('\n');
        out.push_str("## ");
        out.push_str(&date.to_string());
        out.push('\n');
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 严格日期解析：仅接受 `YYYY-MM-DD` 原样格式。
fn parse_date(text: &str) -> Option<NaiveDate> {
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    (date.format("%Y-%m-%d").to_string() == text).then_some(date)
}

/// 临时文件 + rename 原子写（同目录，保证 rename 原子性）。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let parent = path.parent().ok_or_else(|| MemoryError::InvalidPath {
        path: path.display().to_string(),
        reason: "缺少父目录".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;

    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("consolidated");
    let tmp = parent.join(format!(".{name}.tmp-{}-{seq}", std::process::id()));

    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(path, e));
    }
    Ok(())
}

/// 统一 IO 错误包装：带路径，不带内容（隐私要求）。
fn io_err(path: &Path, source: std::io::Error) -> MemoryError {
    MemoryError::Io {
        path: path.to_path_buf(),
        source,
    }
}
