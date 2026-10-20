//! consolidate 测试共享夹具：临时工作区、造日文件、读聚合文件。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_memory::{MemoryPaths, NaiveDate};
use chrono::{DateTime, TimeZone, Utc};

/// 固定「现在」，窗口判定全部相对它计算。
pub const fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("无效测试日期")
}

pub const NOW_DATE: NaiveDate = ymd(2026, 10, 21);

pub fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0)
        .single()
        .expect("有效时刻")
}

/// 极简临时目录（不新增 tempfile 依赖，保持依赖面不变）。
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let id = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "lengsuo-agent-memory-consolidate-{}-{id}-{tag}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("临时目录可创建");
        Self { path }
    }

    pub fn workspace(&self) -> PathBuf {
        self.path.join("ws")
    }

    pub fn paths(&self) -> MemoryPaths {
        MemoryPaths::new(self.path.join("home"), self.workspace())
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.paths().project_memory_dir()
    }

    pub fn daily_file(&self, date: NaiveDate) -> PathBuf {
        self.paths().daily_file(date)
    }

    pub fn consolidated_file(&self) -> PathBuf {
        self.paths().consolidated_file()
    }

    /// 原始写入日文件（内容逐字节落盘，不经 daily 追加语义）。
    pub fn write_daily(&self, date: NaiveDate, content: &str) {
        fs::create_dir_all(self.memory_dir()).unwrap();
        fs::write(self.daily_file(date), content).unwrap();
    }

    /// 预置聚合文件（模拟既有水位）。
    pub fn write_consolidated(&self, content: &str) {
        fs::create_dir_all(self.memory_dir()).unwrap();
        fs::write(self.consolidated_file(), content).unwrap();
    }

    pub fn consolidated(&self) -> String {
        fs::read_to_string(self.consolidated_file()).expect("聚合文件应存在")
    }

    pub fn exists(&self, date: NaiveDate) -> bool {
        self.daily_file(date).exists()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(self.memory_dir(), fs::Permissions::from_mode(0o700));
            let _ = fs::set_permissions(self.workspace(), fs::Permissions::from_mode(0o700));
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// 聚合文件中各日期节的标题（按出现顺序）。
pub fn section_headers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.strip_prefix("## ").map(str::to_string))
        .collect()
}

/// 取某个日期节的正文行。
pub fn section_body<'a>(text: &'a str, date: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            inside = rest == date;
            continue;
        }
        if inside && !line.trim().is_empty() {
            out.push(line);
        }
    }
    out
}
