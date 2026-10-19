//! WSL1 识别（转写 参考实现 `sandboxing/src/bwrap.rs` 的 `is_wsl1` / `proc_version_indicates_wsl1`）。
//!
//! WSL1 内核无法创建 user namespaces，bwrap 沙箱在其上必然不可用；探测层用它短路给出
//! 精确原因（参考实现 `system_bwrap_warning_for_path` 里同样先于 PATH 检查短路）。
//! 纯读 `/proc/version`，零副作用。

/// WSL1 不可用原因（对照 参考实现 `WSL1_BWRAP_WARNING` 的语义与结构）。
pub const WSL1_BWRAP_WARNING: &str = concat!(
    "bwrap-based Linux sandbox is not supported on WSL1 ",
    "because WSL1 cannot create the required user namespaces. ",
    "Use WSL2 for sandboxed shell commands."
);

/// 当前宿主是否为 WSL1（读 `/proc/version`；读不到按非 WSL1 处理）。
pub fn is_wsl1() -> bool {
    std::fs::read_to_string("/proc/version")
        .is_ok_and(|proc_version| proc_version_indicates_wsl1(&proc_version))
}

/// `/proc/version` 内容是否表明 WSL1：先找 `wsl` 后跟的版本号，
/// 没有版本号时回退到「含 microsoft 但不是 microsoft-standard 内核」的启发式。
pub fn proc_version_indicates_wsl1(proc_version: &str) -> bool {
    let proc_version = proc_version.to_ascii_lowercase();
    let mut remaining = proc_version.as_str();
    while let Some(marker) = remaining.find("wsl") {
        let version_start = marker + "wsl".len();
        let version_digits: String = remaining[version_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(version) = version_digits.parse::<u32>() {
            return version == 1;
        }
        remaining = &remaining[version_start..];
    }

    proc_version.contains("microsoft") && !proc_version.contains("microsoft-standard")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wsl1_proc_version_formats() {
        assert!(proc_version_indicates_wsl1(
            "Linux version 4.4.0-22621-Microsoft"
        ));
        assert!(proc_version_indicates_wsl1(
            "Linux version 5.15.0-microsoft-standard-WSL1"
        ));
        assert!(proc_version_indicates_wsl1(
            "Linux version 5.15.0-wsl-microsoft-standard-WSL1"
        ));
    }

    #[test]
    fn does_not_treat_wsl2_or_native_linux_as_wsl1() {
        assert!(!proc_version_indicates_wsl1(
            "Linux version 6.6.87.2-microsoft-standard-WSL2"
        ));
        assert!(!proc_version_indicates_wsl1(
            "Linux version 6.6.87.2-wsl-microsoft-standard-WSL2"
        ));
        assert!(!proc_version_indicates_wsl1(
            "Linux version 4.19.104-microsoft-standard"
        ));
        assert!(!proc_version_indicates_wsl1(
            "Linux version 6.6.87.2-microsoft-standard-WSL3"
        ));
        assert!(!proc_version_indicates_wsl1("Linux version 6.8.0"));
    }
}
