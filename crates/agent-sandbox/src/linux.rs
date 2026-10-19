//! Linux 沙箱实现：bwrap argv 构造 + 可注入探测 + 降级链 + 执行。
//!
//! argv 顺序冻结（黄金向量逐项断言；已在 47 上 bubblewrap 0.9.0 实测）：
//!
//! 1. 会话/父死子亡/身份：`--new-session --die-with-parent --uid 0 --gid 0`
//! 2. 基础挂载：`--ro-bind / /`、`--proc /proc`、`--dev /dev`、`--tmpfs /tmp`
//!
//! 注：不显式 `--unshare-user`——47 实测传该 flag 会让 deny 遮蔽用的 `--tmpfs` 挂载点
//! mkdir 失败；无特权宿主下 bwrap 自行拒绝 userns 时以非零退出失败，fail-closed。
//! 3. `NetworkMode::Blocked` 追加 `--unshare-net`（Allow 则不出现）
//! 4. read_only_roots 逐条 `--ro-bind r r`（按输入顺序；安全基线命中者剔除）
//! 5. writable_roots 逐条 `--bind w w`（安全基线命中者剔除，绝不进入绑定列表）
//! 6. deny 遮蔽：`masks` 逐条 `--tmpfs m`（空 tmpfs 盖掉 .ssh/.gnupg，连读取都不允许）
//! 7. `--chdir <cwd>`（有 cwd 时）
//! 8. env：`--clearenv` 后按白名单顺序 `--setenv K V`（R8 清洗）
//! 9. `--cap-drop ALL`，然后 `-- program args`

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use crate::baseline::{env_passlist, is_denied_path};
use crate::bounded;
use crate::types::{
    CommandSpec, ExecBounds, ExecOutcome, SandboxError, SandboxPolicy, SandboxType,
    SandboxablePreference,
};

pub mod probe;
pub mod seccomp;
pub mod wsl;

/// bwrap 固定前缀（构造层黄金向量的锚点，禁止漂移）。
const PREFIX: &[&str] = &[
    "--new-session",
    "--die-with-parent",
    "--uid",
    "0",
    "--gid",
    "0",
    "--ro-bind",
    "/",
    "/",
    "--proc",
    "/proc",
    "--dev",
    "/dev",
    "--tmpfs",
    "/tmp",
];

/// 探测注入面：文件系统/内核能力可替换，测试不触真实磁盘。
pub trait SandboxProbe {
    /// bwrap 是否真的可运行（**必须跑通最小隔离命令**，不是「PATH 里找得到」）。
    fn bwrap_available(&self) -> bool;
    /// landlock 是否可用（本卡恒 false：Landlock 路径留 TODO，不假装支持）。
    fn landlock_available(&self) -> bool;

    /// 带原因的 bwrap 探测结论。
    ///
    /// 默认实现沿用 [`Self::bwrap_available`]，供只关心布尔面的既有注入实现继续编译；
    /// [`SystemProbe`] 覆盖它以保留失败原因（权限不足/超时/非零退出），
    /// `run_sandboxed_with` 也走这里，确保探测只发生一次且原因能带进审批说明。
    fn bwrap_probe(&self) -> probe::BwrapProbe {
        if self.bwrap_available() {
            probe::BwrapProbe::Usable
        } else {
            probe::BwrapProbe::Missing
        }
    }
}

/// 真实系统探测：PATH 找到 bwrap 后**真实执行最小隔离命令**并验证退出状态。
/// 只有路径存在但跑不起来（权限不足/无 user namespace/超时）时如实返回不可用，
/// 上层因此不会据假阳性跳过审批。
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProbe;

impl SandboxProbe for SystemProbe {
    fn bwrap_available(&self) -> bool {
        self.bwrap_probe().is_usable()
    }
    fn landlock_available(&self) -> bool {
        // TODO(landlock): 探测 /sys/kernel/security/landlock 并实现降级路径。
        // 实现前返回 false：无 bwrap 时探测结果为 None（fail-closed）。
        false
    }
    fn bwrap_probe(&self) -> probe::BwrapProbe {
        probe::probe_system_bwrap()
    }
}

/// 在 PATH 中查找可执行的 bwrap。
pub fn find_system_bwrap_in_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    find_bwrap_in_path_with(&path_var, is_executable_file)
}

fn is_executable_file(p: &Path) -> bool {
    std::fs::metadata(p)
        .map(|m| {
            m.is_file() && (std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
        })
        .unwrap_or(false)
}

/// 注入版 PATH 查找：`is_executable` 回调决定命中，便于单测（不起真实进程）。
pub fn find_bwrap_in_path_with(
    path_var: &std::ffi::OsStr,
    is_executable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join("bwrap"))
        .find(|candidate| is_executable(candidate))
}

/// 探测结论：能力 + 不可用原因（`None` 沙箱时原因必有值）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxDetection {
    /// 探测得到的沙箱实现；不可用时为 `None`。
    pub sandbox: Option<SandboxType>,
    /// 不可用原因；可用时为 `None`。
    pub reason: Option<String>,
}

/// 降级链（注入版，带原因）：bwrap **真实跑通** → LinuxSeccomp；
/// landlock → TODO（保持不可用）；否则不可用并保留探测原因。
pub fn detect_sandbox_detailed_with(probe: &impl SandboxProbe) -> SandboxDetection {
    match probe.bwrap_probe() {
        probe::BwrapProbe::Usable => SandboxDetection {
            sandbox: Some(SandboxType::LinuxSeccomp),
            reason: None,
        },
        failure => {
            let reason = failure
                .reason()
                .unwrap_or_else(|| "bwrap unavailable".to_string());
            if probe.landlock_available() {
                // TODO(landlock): 接入 Landlock 后端；实现前保持 None（fail-closed），不假装支持。
                return SandboxDetection {
                    sandbox: None,
                    reason: Some(format!("{reason}; landlock backend not implemented")),
                };
            }
            SandboxDetection {
                sandbox: None,
                reason: Some(reason),
            }
        }
    }
}

/// 降级链（注入版）：bwrap 可用 → LinuxSeccomp；landlock → TODO（返回 None）；都无 → None。
pub fn detect_sandbox_with(probe: &impl SandboxProbe) -> Option<SandboxType> {
    detect_sandbox_detailed_with(probe).sandbox
}

/// 平台沙箱探测：Linux 下 bwrap 可用 → Some(LinuxSeccomp)，否则 None。
pub fn get_platform_sandbox() -> Option<SandboxType> {
    detect_sandbox_with(&SystemProbe)
}

/// 收集需要遮蔽的凭据路径：显式 roots 里命中安全基线的 + 家目录下真实存在的 .ssh/.gnupg。
/// 返回排序去重后的列表；不存在的路径不遮蔽（目标本来不可见）。
pub fn masks_for(spec: &CommandSpec) -> Vec<PathBuf> {
    let mut masks: Vec<PathBuf> = spec
        .writable_roots
        .iter()
        .chain(spec.read_only_roots.iter())
        .filter(|p| is_denied_path(p))
        .cloned()
        .collect();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for name in ["ssh", "gnupg"] {
            let p = home.join(format!(".{name}"));
            if p.is_dir() && !masks.contains(&p) {
                masks.push(p);
            }
        }
    }
    masks.sort();
    masks.dedup();
    masks
}

/// 构造 bwrap argv（不含 argv[0]=bwrap 本身）。
///
/// - writable/read_only roots 中命中安全基线（.ssh/.gnupg 等）的条目被剔除，绝不进绑定列表；
/// - `masks` 逐条转 `--tmpfs` 遮蔽；不命中安全基线的 mask 项被忽略（防误用）。
pub fn build_bwrap_argv(spec: &CommandSpec, masks: &[PathBuf]) -> Vec<String> {
    let mut argv: Vec<String> = PREFIX.iter().map(|s| s.to_string()).collect();
    if spec.network.should_unshare_network() {
        argv.push("--unshare-net".to_string());
    }
    for root in &spec.read_only_roots {
        if is_denied_path(root) {
            continue;
        }
        argv.push("--ro-bind".to_string());
        push_path(&mut argv, root);
        push_path(&mut argv, root);
    }
    for root in &spec.writable_roots {
        if is_denied_path(root) {
            continue;
        }
        argv.push("--bind".to_string());
        push_path(&mut argv, root);
        push_path(&mut argv, root);
    }
    for mask in masks {
        if !is_denied_path(mask) {
            continue;
        }
        argv.push("--tmpfs".to_string());
        push_path(&mut argv, mask);
    }
    if let Some(cwd) = &spec.cwd {
        argv.push("--chdir".to_string());
        push_path(&mut argv, cwd);
    }
    argv.push("--clearenv".to_string());
    for (k, v) in env_passlist(&spec.env) {
        argv.push("--setenv".to_string());
        argv.push(k);
        argv.push(v);
    }
    argv.push("--cap-drop".to_string());
    argv.push("ALL".to_string());
    argv.push("--".to_string());
    push_path(&mut argv, &spec.program);
    argv.extend(spec.args.iter().cloned());
    argv
}

fn push_path(argv: &mut Vec<String>, p: &Path) {
    argv.push(p.to_string_lossy().into_owned());
}

/// 主 API：按偏好/策略分发执行（真实探测），使用 [`ExecBounds::default`]。
pub fn run_sandboxed(
    spec: CommandSpec,
    pref: SandboxablePreference,
    policy: SandboxPolicy,
) -> Result<ExecOutcome, SandboxError> {
    run_sandboxed_bounded(spec, pref, policy, ExecBounds::default())
}

/// 主 API（有界版）：`bounds` 同时约束 bwrap 与 Forbid 直跑两条路径。
pub fn run_sandboxed_bounded(
    spec: CommandSpec,
    pref: SandboxablePreference,
    policy: SandboxPolicy,
    bounds: ExecBounds,
) -> Result<ExecOutcome, SandboxError> {
    run_sandboxed_with_bounded(&SystemProbe, spec, pref, policy, bounds)
}

/// 注入版分发：降级链与 preference 语义的完整实现。
///
/// Forbid 不额外探测（不产生探测进程）；探测失败时 Require 拒绝、Auto 按策略
/// 转人工审批或拒绝——**没有任何分支会静默回退成非沙箱直跑**。
pub fn run_sandboxed_with(
    probe: &impl SandboxProbe,
    spec: CommandSpec,
    pref: SandboxablePreference,
    policy: SandboxPolicy,
) -> Result<ExecOutcome, SandboxError> {
    run_sandboxed_with_bounded(probe, spec, pref, policy, ExecBounds::default())
}

/// 注入版分发（有界版）：与 [`run_sandboxed_with`] 同语义，超时/输出上限由 `bounds` 决定。
pub fn run_sandboxed_with_bounded(
    probe: &impl SandboxProbe,
    spec: CommandSpec,
    pref: SandboxablePreference,
    policy: SandboxPolicy,
    bounds: ExecBounds,
) -> Result<ExecOutcome, SandboxError> {
    // Forbid：明确不沙箱 → 非沙箱直跑（env 仍按 R8 清洗），一次探测都不发生。
    if pref == SandboxablePreference::Forbid {
        return run_unsandboxed(spec, bounds);
    }
    let detection = detect_sandbox_detailed_with(probe);
    let Some(target) = detection.sandbox else {
        let reason = detection
            .reason
            .unwrap_or_else(|| "no sandbox available on this platform".to_string());
        // Require + 无能力 → fail-closed 拒绝。
        // Auto + 无能力 → 按策略转人工审批，或拒绝。
        return Err(match pref {
            SandboxablePreference::Require => {
                SandboxError::Unavailable(format!("{reason} but Require was requested"))
            }
            _ if policy.allow_approval_fallback => SandboxError::NeedsApproval(format!(
                "{reason}; command must go through manual approval"
            )),
            _ => SandboxError::Denied(format!("{reason} and approval fallback disabled")),
        });
    };
    match target {
        SandboxType::LinuxSeccomp => run_bwrap(spec, bounds),
        other => Err(SandboxError::Unavailable(format!(
            "{} is not a Linux sandbox backend",
            other.as_metric_tag()
        ))),
    }
}

fn run_unsandboxed(spec: CommandSpec, bounds: ExecBounds) -> Result<ExecOutcome, SandboxError> {
    let label = spec.program.to_string_lossy().into_owned();
    let mut cmd = ProcessCommand::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(env_passlist(&spec.env));
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    bounded::capture(cmd, SandboxType::None, &label, bounds)
}

/// bwrap 沙箱执行。
/// TODO(seccomp-apply): 把 `seccomp::linux_seccomp_profile()` 编译为 BPF 并经 fd 传给
/// `--seccomp <fd>`（参照 参考实现 linux-sandbox/src/seccomp.rs）。当前仅 bwrap 命名空间/绑定层
/// 生效，seccomp profile 只以构造+断言存在——**未应用，不得声称已生效**。
fn run_bwrap(spec: CommandSpec, bounds: ExecBounds) -> Result<ExecOutcome, SandboxError> {
    let bwrap = find_system_bwrap_in_path()
        .ok_or_else(|| SandboxError::Unavailable("bwrap not found in PATH".to_string()))?;
    let masks = masks_for(&spec);
    let argv = build_bwrap_argv(&spec, &masks);
    let label = bwrap.to_string_lossy().into_owned();
    let mut cmd = ProcessCommand::new(&bwrap);
    cmd.args(&argv);
    bounded::capture(cmd, SandboxType::LinuxSeccomp, &label, bounds)
}
