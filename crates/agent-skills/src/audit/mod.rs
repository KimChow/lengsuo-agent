//! 技能包安全审计五步：结构 → 捆绑文件 → 体积上限 → 授权面 → 正文可疑模式。
//!
//! 依据 参考客户端 09 册 §5.1（安装生命周期：`queryScanResult` 安全扫描）与
//! 18 册 §9.1（安装前强制安全审计五步：审计 `SKILL.md` 与全部捆绑文件
//! `scripts/references/assets`，出报告后按 P0/P1/P2 分级处置）。
//!
//! 分级与处置：
//! - **P0**：强警告、建议不装 —— 本仓取更严口径：**拒绝加载**（fail-closed）。
//! - **P1**：警告 + 需用户确认 —— 不阻断加载，但报告随正文一起返回，由宿主确认。
//! - **P2**：提示后放行。
//!
//! 审计**只读**：只读目录项与文件内容，不写、不删、不改名、不移动。
//! 本模块只审计**已落盘的技能包**：不下载、不解压、不安装。
//!
//! 危险命令表在本 crate 内**独立实现**（`agent-skills` 不依赖 `agent-core`，
//! 依赖方向不可反），形态对齐 `agent-core/src/sensitive.rs` 的规则表；
//! 与 P2 卡的收敛说明见 `docs/skill-audit-status.md`。

mod bundles;
mod content;
mod scope;
mod size;
mod structure;

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// 风险级别：P0 最重（拒绝加载），P2 最轻（提示放行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RiskLevel {
    /// 拒绝加载：必须显式处置（fail-closed）。
    P0,
    /// 需确认：宿主应向用户确认后再用。
    P1,
    /// 提示放行：记录即可。
    P2,
}

impl RiskLevel {
    /// 严重度序（0 最重）：便于比较与取最重档。
    pub const fn severity(self) -> u8 {
        match self {
            RiskLevel::P0 => 0,
            RiskLevel::P1 => 1,
            RiskLevel::P2 => 2,
        }
    }

    /// 级别标签（`P0`/`P1`/`P2`）。
    pub const fn label(self) -> &'static str {
        match self {
            RiskLevel::P0 => "P0",
            RiskLevel::P1 => "P1",
            RiskLevel::P2 => "P2",
        }
    }
}

impl Ord for RiskLevel {
    fn cmp(&self, other: &Self) -> Ordering {
        // 序上 P0 最大（最重），故 `max()` 取到的就是最重档。
        other.severity().cmp(&self.severity())
    }
}

impl PartialOrd for RiskLevel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 审计步（五步固定顺序，与参考件一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditStep {
    /// 步 1：目录/文件结构检查（`SKILL.md` 存在、附属文件是否越界）。
    Structure,
    /// 步 2：捆绑文件递归审计（危险命令 / 网络下载）。
    Bundles,
    /// 步 3：总体字节上限（包总量与文件数）。
    Size,
    /// 步 4：声明的工具与指令是否超出授权面。
    Scope,
    /// 步 5：正文里的可疑模式（提示注入、越权指令、外部下载 URL）。
    Content,
}

impl AuditStep {
    /// 步序号（1..=5）。
    pub const fn index(self) -> u8 {
        match self {
            AuditStep::Structure => 1,
            AuditStep::Bundles => 2,
            AuditStep::Size => 3,
            AuditStep::Scope => 4,
            AuditStep::Content => 5,
        }
    }

    /// 步标题。
    pub const fn title(self) -> &'static str {
        match self {
            AuditStep::Structure => "目录/文件结构检查",
            AuditStep::Bundles => "捆绑文件递归审计",
            AuditStep::Size => "总体字节上限",
            AuditStep::Scope => "声明工具与指令的授权面",
            AuditStep::Content => "正文可疑模式",
        }
    }
}

/// 一条审计结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 风险级别。
    pub level: RiskLevel,
    /// 触发该结论的审计步。
    pub step: AuditStep,
    /// 定位：相对技能目录的路径，或包级位置描述。
    pub location: String,
    /// 结论说明（不包含凭据原文）。
    pub message: String,
}

/// 审计报告：级别取全部结论里最重的一档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    /// 最高风险级别；无结论时为 P2（最低档）。
    pub level: RiskLevel,
    /// 全部结论（按发现顺序）。
    pub findings: Vec<Finding>,
}

impl Default for AuditReport {
    fn default() -> Self {
        Self {
            level: RiskLevel::P2,
            findings: Vec::new(),
        }
    }
}

impl AuditReport {
    /// 无任何结论（干净技能）。
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// 是否存在某级别的结论。
    pub fn has_level(&self, level: RiskLevel) -> bool {
        self.findings.iter().any(|f| f.level == level)
    }

    /// 涉及到的步（去重、保序）。
    pub fn steps(&self) -> Vec<AuditStep> {
        let mut out: Vec<AuditStep> = Vec::new();
        for finding in &self.findings {
            if !out.contains(&finding.step) {
                out.push(finding.step);
            }
        }
        out
    }

    /// 摘要：给错误信息与日志用（逐条结论一行，不含正文）。
    pub fn summary(&self) -> String {
        self.findings
            .iter()
            .map(|f| format!("[{}] {}: {}", f.level, f.location, f.message))
            .collect::<Vec<_>>()
            .join("；")
    }
}

/// 审计上限（可调）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditLimits {
    /// 单个技能包总字节上限。
    pub max_package_bytes: u64,
    /// 单个技能包文件数上限（含 `SKILL.md`）。
    pub max_package_files: usize,
    /// 单个捆绑文件字节上限。
    pub max_file_bytes: u64,
    /// `SKILL.md` 字节上限。
    pub max_skill_md_bytes: u64,
}

impl Default for AuditLimits {
    fn default() -> Self {
        Self {
            max_package_bytes: 2 * 1024 * 1024,
            max_package_files: 64,
            max_file_bytes: 256 * 1024,
            max_skill_md_bytes: 32 * 1024,
        }
    }
}

/// 审计一个已落盘的技能包（默认上限）。
///
/// `host_tools` 是宿主已授权工具集：给出时才判步 4（授权面）；为 `None` 时
/// 跳过该步（没有可比对的授权面，不臆造结论）。
pub fn audit_skill(dir: &Path, host_tools: Option<&BTreeSet<String>>) -> AuditReport {
    audit_skill_with_limits(dir, host_tools, &AuditLimits::default())
}

/// 审计一个已落盘的技能包（自定义上限）：五步按参考顺序依次执行。
pub fn audit_skill_with_limits(
    dir: &Path,
    host_tools: Option<&BTreeSet<String>>,
    limits: &AuditLimits,
) -> AuditReport {
    // 步 1 顺带收集包清单，步 2/3 复用（只遍历一次，不重复读目录）。
    let (package, mut findings) = structure::inspect(dir);
    findings.extend(bundles::audit(&package));
    findings.extend(size::audit(&package, limits));
    findings.extend(scope::audit(&package, host_tools));
    findings.extend(content::audit(&package));
    let level = findings
        .iter()
        .map(|f| f.level)
        .max()
        .unwrap_or(RiskLevel::P2);
    AuditReport { level, findings }
}

/// 技能包清单：步 1 收集，步 2/3/4/5 复用。
pub(crate) struct Package {
    /// 技能目录。
    pub dir: PathBuf,
    /// `SKILL.md` 路径（不存在或不是普通文件时为 `None`）。
    pub skill_md: Option<PathBuf>,
    /// 捆绑文件清单（相对路径 + 绝对路径 + 字节数），按相对路径升序。
    pub files: Vec<BundleFile>,
}

/// 清单里的一条捆绑文件。
pub(crate) struct BundleFile {
    /// 相对技能目录的路径（`scripts/run.sh`）。
    pub relative: String,
    /// 绝对路径。
    pub absolute: PathBuf,
    /// 字节数。
    pub bytes: u64,
}

/// 内容扫描的单文件读取上限（超过则跳过内容扫描，只由步 3 记体积结论）。
pub(crate) const MAX_READ_BYTES: u64 = 1024 * 1024;

/// 读文本文件：二进制（含 NUL 字节）、超读取上限或非 UTF-8 时返回 `None`
/// （跳过内容扫描，不臆造结论；体积另由步 3 判）。
pub(crate) fn read_text(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_READ_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    // 前 8 KiB 出现 NUL 即判二进制（脚本/文本不会含 NUL）。
    let window = &bytes[..bytes.len().min(8192)];
    if window.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// 大小写不敏感的子串匹配：命中位置左侧不得是字母/数字/下划线（词边界近似），
/// 避免 `perform -rf` 这类散文误判。
pub(crate) fn contains_needle(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = haystack.to_ascii_lowercase();
    let pat = needle.to_ascii_lowercase();
    let mut start = 0usize;
    while let Some(pos) = hay[start..].find(&pat) {
        let pos = start + pos;
        let left_ok = pos == 0
            || !pat.as_bytes()[0].is_ascii_alphanumeric()
            || !is_word_byte(hay.as_bytes()[pos - 1]);
        if left_ok {
            return true;
        }
        start = pos + 1;
    }
    false
}

/// 命中片段（返回第一个命中的 needle 原文，供结论说明用）。
pub(crate) fn matched_needle<'a>(haystack: &'a str, needles: &[&'a str]) -> Option<&'a str> {
    needles
        .iter()
        .find(|needle| contains_needle(haystack, needle))
        .copied()
}

/// 首个外部下载 URL（去掉查询串并截断，避免把 URL 里的令牌写进结论）。
pub(crate) fn first_url(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for scheme in ["http://", "https://"] {
        if let Some(pos) = lower.find(scheme) {
            let better = best.is_none_or(|(best_pos, _)| pos < best_pos);
            if better {
                best = Some((pos, &lower[pos..]));
            }
        }
    }
    let (_, tail) = best?;
    let end = tail
        .find(|c: char| {
            c.is_whitespace() || c == '"' || c == '\'' || c == ')' || c == '>' || c == '`'
        })
        .unwrap_or(tail.len());
    let url = &tail[..end];
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let url: String = url.chars().take(48).collect();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

/// 是否是 ASCII 单词字符（字母/数字/下划线）。
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
