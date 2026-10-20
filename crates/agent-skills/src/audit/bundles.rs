//! 步 2：危险命令与网络下载的规则扫描（递归覆盖 `SKILL.md` 与全部捆绑文件）。
//!
//! 参考件：harness 规格 §3「对 `SKILL.md` + 目录下 `scripts/|references/|assets/`
//! （递归，设总字节上限防 DoS）做规则匹配（复用 `agent-core/src/sensitive.rs`
//! 风格：危险 shell、`rm -rf`、外连 URL、`eval/exec`、凭据字面量）」；
//! 09 册 §5.1 安装生命周期（`queryScanResult` 安全扫描）与 18 册 §9.1
//! 「审计 `SKILL.md` 与全部捆绑文件」。
//!
//! 级别：**P0**（拒绝加载）。理由：捆绑文件是技能自带的代码/资料，其危险命令
//! 等同技能自带恶意载荷；`SKILL.md` 正文会整段注入 system 消息，正文里的
//! 破坏性命令是指示模型执行破坏动作，同样不可放行。
//!
//! 危险命令表在 `agent-skills` 内**独立实现**（本 crate 不依赖 `agent-core`，
//! 依赖方向不可反）；形态对齐 `agent-core/src/sensitive.rs` 的规则表，
//! 与 P2 卡的收敛说明见 `docs/skill-audit-status.md`。

use super::{
    contains_needle, first_url, matched_needle, read_text, AuditStep, BundleFile, Finding, Package,
    RiskLevel,
};

/// 危险命令表（独立实现，最小集 12 条）：命中即 P0。
///
/// 形态与 `agent-core/src/sensitive.rs` 的规则表同源（同样的规则名与语义），
/// 但在此 crate 内自持一份，避免反向依赖。
pub(crate) const DANGEROUS_COMMANDS: [(&str, &str); 12] = [
    ("rm-force-recursive", "rm -rf"),
    ("rm-force-root", "rm -fr /"),
    ("dd-to-device", "dd if="),
    ("disk-wipe", "mkfs"),
    ("disk-wipe", "wipefs"),
    ("fork-bomb", ":(){ :|:& };:"),
    ("chmod-777-root", "chmod 777 /"),
    ("chown-recursive", "chown -R"),
    ("shutdown", "shutdown -h"),
    ("disk-erase", "shred -u"),
    ("history-wipe", "history -c"),
    ("dd-disk-write", "of=/dev/"),
];

/// 远程下载片段（`curl`/`wget` 拉取外部内容）：命中即 P0。
pub(crate) const REMOTE_DOWNLOAD_PATTERNS: [&str; 4] =
    ["curl http", "curl https", "wget http", "wget https"];

/// 管道喂解释器片段（下载内容直接执行）：命中即 P0。
pub(crate) const PIPE_TO_INTERPRETER: [&str; 6] =
    ["| sh", "|sh", "| bash", "|bash", "| python", "|python"];

/// 步 2：扫描 `SKILL.md` 正文与全部捆绑文件（只读）。
pub(crate) fn audit(package: &Package) -> Vec<Finding> {
    let mut findings = Vec::new();
    // `SKILL.md` 正文同样过规则集：正文会被注入 system 消息。
    if let Some(path) = package.skill_md.as_ref() {
        if let Some(text) = read_text(path) {
            findings.extend(scan_text(&text, crate::parse::SKILL_FILE, false));
        }
    }
    // 捆绑文件递归扫描（清单已在步 1 收齐）。
    for file in &package.files {
        audit_file(file, &mut findings);
    }
    findings
}

/// 扫描单个捆绑文件。
fn audit_file(file: &BundleFile, findings: &mut Vec<Finding>) {
    let Some(text) = read_text(&file.absolute) else {
        // 二进制/超大文件不做内容匹配（体积另由步 3 判），不臆造结论。
        return;
    };
    findings.extend(scan_text(&text, &file.relative, true));
}

/// 规则集扫描一段文本。
///
/// `bundle_scope` 区分处置：捆绑文件里的外链 URL 是隐式下载动作 ⇒ P0；
/// `SKILL.md` 正文里的外链 URL 属步 5（正文可疑模式，P2），此处不判。
fn scan_text(text: &str, location: &str, bundle_scope: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    let lower = text.to_ascii_lowercase();

    // 下载即执行：管道喂解释器 ⇒ P0。
    if let Some(needle) = matched_needle(&lower, &PIPE_TO_INTERPRETER) {
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Bundles,
            location: location.to_string(),
            message: format!(
                "远程脚本执行（下载内容直接交给解释器）: remote-script-exec ({needle})"
            ),
        });
    }

    // 远程下载动作 ⇒ P0。
    if let Some(needle) = matched_needle(&lower, &REMOTE_DOWNLOAD_PATTERNS) {
        findings.push(Finding {
            level: RiskLevel::P0,
            step: AuditStep::Bundles,
            location: location.to_string(),
            message: format!("含网络下载动作: remote-download ({})", needle.trim()),
        });
    }

    // 危险命令表命中 ⇒ P0。
    for (rule, needle) in DANGEROUS_COMMANDS {
        if contains_needle(&lower, needle) {
            findings.push(Finding {
                level: RiskLevel::P0,
                step: AuditStep::Bundles,
                location: location.to_string(),
                message: format!("危险命令命中: {rule}"),
            });
        }
    }

    // 捆绑文件里的外链 URL ⇒ P0（技能自带资料指向外部下载源）。
    if bundle_scope {
        if let Some(url) = first_url(&lower) {
            findings.push(Finding {
                level: RiskLevel::P0,
                step: AuditStep::Bundles,
                location: location.to_string(),
                message: format!("捆绑文件含外部下载 URL: {url}"),
            });
        }
    }

    findings
}
