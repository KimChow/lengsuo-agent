//! 平台沙箱**真实探测**（判定端输入）与危险命令表（强制审批层）。
//!
//! 形态对照 参考实现 `codex-rs/sandboxing/src/manager.rs` 的
//! `ensure_linux_bubblewrap_is_supported`：能力判定与执行分离，探测只回答
//! 「当前平台能否沙箱」，不构造命令；探不到就是 `None`（fail-closed）。
//! 本仓的降级链细节（bwrap 真实跑一次最小隔离命令、landlock TODO）在
//! [`crate::linux::detect_sandbox_detailed_with`]，本模块只做平台分派与兜底。
//!
//! 危险命令表对齐 wb 22 册 §2 步骤 2 的 `checkBashSecurityWindows`：
//! **独立于沙箱的第二层命令安检**，命中 ⇒ 判定端强制 ask；
//! 危险命令不因「能沙箱」而免审批。
//!
//! 失败口径（硬约束）：任何探测路径 panic / 返回空后端，一律视为**不能沙箱**，
//! 不 panic 到调用方、不放行。

use crate::types::SandboxType;

/// 平台探测结论：可用后端 + 不可用原因（不可用时 [`Self::reason`] 必有值）。
///
/// 与 [`crate::linux::SandboxDetection`] 同形，但跨平台统一由本模块产出，
/// 判定端无需按 `target_os` 分叉。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformProbe {
    /// 探测到的可用后端；不可用时为 `None`（`SandboxType::None` 不算可用）。
    pub backend: Option<SandboxType>,
    /// 不可用原因；可用时为 `None`。
    pub reason: Option<String>,
}

impl PlatformProbe {
    /// 探测成功：后端可用。
    pub fn available(backend: SandboxType) -> Self {
        let backend = (backend != SandboxType::None).then_some(backend);
        match backend {
            Some(backend) => Self {
                backend: Some(backend),
                reason: None,
            },
            None => Self::unavailable("probe returned SandboxType::None"),
        }
    }

    /// 探测失败/不可用：原因入结论，后端恒 `None`。
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            backend: None,
            reason: Some(reason.into()),
        }
    }

    /// 是否真的可沙箱（`None` 与 `SandboxType::None` 都不可用）。
    pub fn is_available(&self) -> bool {
        self.backend
            .is_some_and(|backend| backend != SandboxType::None)
    }

    /// 指标/日志标签：可用后端 → 短标签，否则 `None`。
    pub fn metric_tag(&self) -> Option<&'static str> {
        self.backend
            .filter(|backend| *backend != SandboxType::None)
            .map(SandboxType::as_metric_tag)
    }

    /// 不可用原因；可用时返回静态占位（调用方无需 `Option` 分支）。
    pub fn reason(&self) -> &str {
        self.reason.as_deref().unwrap_or("sandbox available")
    }
}

/// Linux：bwrap **真实跑通** → `LinuxSeccomp`；landlock 未实现 → 不可用并保留原因。
#[cfg(target_os = "linux")]
fn platform_probe_inner() -> PlatformProbe {
    let detection = crate::linux::detect_sandbox_detailed_with(&crate::linux::SystemProbe);
    match detection.sandbox {
        Some(backend) => PlatformProbe::available(backend),
        None => PlatformProbe::unavailable(
            detection
                .reason
                .unwrap_or_else(|| "bwrap unusable".to_string()),
        ),
    }
}

/// Windows：干跑建一次受限令牌（`windows::get_platform_sandbox` 的真实探测）。
#[cfg(windows)]
fn platform_probe_inner() -> PlatformProbe {
    match crate::windows::get_platform_sandbox() {
        Some(backend) => PlatformProbe::available(backend),
        None => PlatformProbe::unavailable("windows restricted-token probe failed"),
    }
}

/// macOS：seatbelt 尚未实现，恒不可用（不假装支持）。
#[cfg(target_os = "macos")]
fn platform_probe_inner() -> PlatformProbe {
    match crate::macos::get_platform_sandbox() {
        Some(backend) => PlatformProbe::available(backend),
        None => PlatformProbe::unavailable("macos seatbelt backend not implemented"),
    }
}

/// 其余平台：无沙箱实现（fail-closed）。
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_probe_inner() -> PlatformProbe {
    PlatformProbe::unavailable("no sandbox backend for this platform")
}

/// 探测当前平台能否沙箱：宿主每次判定调用一次，**绝不让探测失败变成放行**。
///
/// 平台实现可能因外部命令（bwrap 干跑）失败而 panic，故用 `catch_unwind`
/// 兜底：panic 一律折算为 [`PlatformProbe::unavailable`]，调用方只看到
/// 「不能沙箱」，不会收到 panic，也不会误判为可用。
pub fn probe_platform_sandbox() -> PlatformProbe {
    guarded_probe(platform_probe_inner)
}

/// 可注入的 panic 兜底（测试用）：`probe` panic ⇒ 不可用 + 固定原因。
pub fn guarded_probe(
    probe: impl FnOnce() -> PlatformProbe + std::panic::UnwindSafe,
) -> PlatformProbe {
    match std::panic::catch_unwind(probe) {
        Ok(result) => result,
        Err(_) => PlatformProbe::unavailable("platform probe panicked"),
    }
}

/// 一条危险命令规则：规则名（静态、可入审计）+ 命中片段（全文子串，小写）。
#[derive(Clone, Copy, Debug)]
pub struct DangerousRule {
    /// 审计用规则名（不含命令参数值）。
    pub name: &'static str,
    /// 命中片段；命令串先转小写再逐条 `contains`。
    pub needles: &'static [&'static str],
}

/// 危险命令表（最小集 11 条，对齐 22 册 §2 步骤 2 的 `checkBashSecurityWindows`）。
///
/// 匹配口径：**命令串全文子串匹配，大小写不敏感**（`to_ascii_lowercase`）；
/// 命中即强制 ask。表是刻意保守的最小集，宁可多问一次，不可漏放。
pub const DANGEROUS_COMMANDS: [DangerousRule; 11] = [
    DangerousRule {
        name: "rm-force-recursive",
        needles: &["rm -rf", "rm -fr"],
    },
    DangerousRule {
        name: "disk-format",
        needles: &["mkfs"],
    },
    DangerousRule {
        name: "dd-device-write",
        needles: &["dd if=", "of=/dev/"],
    },
    DangerousRule {
        name: "git-push-force",
        needles: &["git push --force", "git push -f"],
    },
    DangerousRule {
        name: "git-reset-hard",
        needles: &["git reset --hard"],
    },
    DangerousRule {
        name: "fork-bomb",
        needles: &[":(){ :|:& };:"],
    },
    DangerousRule {
        name: "power-off",
        needles: &["shutdown", "reboot"],
    },
    DangerousRule {
        name: "chmod-777-root",
        needles: &["chmod -r 777 /"],
    },
    DangerousRule {
        name: "pipe-to-shell",
        needles: &["| sh", "| bash"],
    },
    DangerousRule {
        name: "windows-registry-write",
        needles: &["reg add", "reg delete"],
    },
    DangerousRule {
        name: "windows-remove-item",
        needles: &["remove-item -recurse -force"],
    },
];

/// 危险命令匹配：命中返回**规则名**（静态字符串，可入事件流），未命中 `None`。
///
/// 只回规则名，不回显命令串——审计面不复制用户参数。
pub fn dangerous_command_reason(command: &str) -> Option<&'static str> {
    if command.is_empty() {
        return None;
    }
    let lowered = command.to_ascii_lowercase();
    DANGEROUS_COMMANDS
        .iter()
        .find(|rule| rule.needles.iter().any(|needle| lowered.contains(needle)))
        .map(|rule| rule.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_table_matches_case_insensitively() {
        assert_eq!(
            dangerous_command_reason("RM -RF /"),
            Some("rm-force-recursive")
        );
        assert_eq!(
            dangerous_command_reason("git push --FORCE origin main"),
            Some("git-push-force")
        );
        assert_eq!(
            dangerous_command_reason("shutdown -h now"),
            Some("power-off")
        );
        assert_eq!(dangerous_command_reason("ls -la"), None);
        assert_eq!(dangerous_command_reason(""), None);
    }

    #[test]
    fn guarded_probe_turns_panic_into_unavailable() {
        let probe = guarded_probe(|| panic!("boom"));
        assert!(!probe.is_available());
        assert_eq!(probe.backend, None);
        assert_eq!(probe.reason(), "platform probe panicked");
    }

    #[test]
    fn available_probe_exposes_metric_tag() {
        let probe = PlatformProbe::available(SandboxType::LinuxSeccomp);
        assert!(probe.is_available());
        assert_eq!(probe.metric_tag(), Some("seccomp"));
        let none = PlatformProbe::available(SandboxType::None);
        assert!(!none.is_available());
        assert_eq!(none.metric_tag(), None);
    }
}
