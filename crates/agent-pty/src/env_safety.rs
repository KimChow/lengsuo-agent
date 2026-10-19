//! 请求上下文环境清洗：PTY 子进程不得继承会话/请求标识。
//!
//! 这是「防止启动上下文意外外泄」的**非沙箱**措施：它只从子进程环境里摘掉
//! 指定名字的变量，不构成文件系统、网络或凭据安全边界。真正的沙箱/策略约束
//! 由上层（执行策略与沙箱分发层）负责，本 crate 不冒充。
//!
//! 命名与调用顺序对齐参考实现的环境策略：先按字面名逐个摘除，再对
//! **继承环境**与**显式配置名**做一次大小写不敏感扫除；显式覆盖在扫除之前
//! 生效，因此覆盖无法把这些键重新注入（与参考实现「覆盖后再过滤」同序）。

use std::ffi::OsString;

use portable_pty::CommandBuilder;

/// 不得进入 PTY 子进程的请求上下文变量名（大小写不敏感匹配）。
///
/// 这些键把一次工具调用/会话的身份带进子进程环境，会被模型可触达的进程读到，
/// 因此与继承环境和 `PtyConfig.env` 显式覆盖一并清洗。
pub(crate) const REQUEST_CONTEXT_ENV_VARS: &[&str] = &[
    "CONVERSATION_MESSAGE_ID",
    "ROOT_REQUEST_ID",
    "TOOL_CALL_ID",
    "TRACE_ID",
];

/// 名字是否属于请求上下文变量（ASCII 大小写不敏感）。
pub(crate) fn is_request_context_env_var(name: &str) -> bool {
    REQUEST_CONTEXT_ENV_VARS
        .iter()
        .any(|restricted| restricted.eq_ignore_ascii_case(name))
}

/// 从待启动的命令中摘除请求上下文变量。
///
/// 顺序与参考实现一致：
/// 1. 先按规范名逐个 `env_remove`（覆盖大小写正确但取值非 UTF-8 的条目）；
/// 2. 再扫一遍**继承环境名**（`std::env::vars_os()`，只取名字，不看取值）与
///    **调用方显式配置名**，凡大小写不敏感命中即摘除。
///
/// `portable-pty` 的 `CommandBuilder` 没有 `get_envs()`，显式名只能经
/// `iter_extra_env_as_str()` 取得；而 `PtyConfig.env` 的键值都是 `String`，
/// 必然 UTF-8，不存在被该迭代器跳过的显式条目。继承环境的名字则直接取自
/// `std::env::vars_os()`，因此取值非 UTF-8 的继承条目同样会被扫到。
pub(crate) fn scrub_request_context_env(cmd: &mut CommandBuilder) {
    let configured_names = cmd
        .iter_extra_env_as_str()
        .map(|(name, _)| OsString::from(name))
        .collect::<Vec<_>>();

    for name in REQUEST_CONTEXT_ENV_VARS {
        cmd.env_remove(name);
    }
    for name in std::env::vars_os()
        .map(|(name, _)| name)
        .chain(configured_names)
    {
        if name.to_str().is_some_and(is_request_context_env_var) {
            cmd.env_remove(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// 进程环境变量的设置/还原守卫。
    struct EnvGuard {
        saved: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&str, &str)]) -> Self {
            let mut saved = Vec::with_capacity(pairs.len());
            for (key, value) in pairs {
                saved.push((key.to_string(), std::env::var(key).ok()));
                std::env::set_var(key, value);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn names_match_case_insensitively() {
        for name in REQUEST_CONTEXT_ENV_VARS {
            assert!(is_request_context_env_var(name));
            assert!(is_request_context_env_var(&name.to_ascii_lowercase()));
            let mixed = format!(
                "{}{}",
                name[..1].to_ascii_uppercase(),
                name[1..].to_ascii_lowercase()
            );
            assert!(is_request_context_env_var(&mixed));
        }
        assert!(!is_request_context_env_var("PATH"));
        assert!(!is_request_context_env_var("CONVERSATION_MESSAGE"));
        assert!(!is_request_context_env_var(""));
    }

    #[test]
    fn scrub_removes_explicit_overrides_and_keeps_normal_vars() {
        let mut cmd = CommandBuilder::new("env");
        cmd.env("CONVERSATION_MESSAGE_ID", "injected");
        cmd.env("trace_id", "injected");
        cmd.env("W3_ENV_SAFETY_KEEP", "kept");

        scrub_request_context_env(&mut cmd);

        assert_eq!(cmd.get_env("CONVERSATION_MESSAGE_ID"), None);
        assert_eq!(cmd.get_env("trace_id"), None);
        assert_eq!(cmd.get_env("W3_ENV_SAFETY_KEEP"), Some(OsStr::new("kept")));
    }

    #[test]
    fn scrub_removes_inherited_case_variants_and_keeps_path() {
        // 父进程环境注入大小写变种；`CommandBuilder::new` 会快照基础环境。
        let _guard = EnvGuard::set(&[
            ("RoOt_ReQuEsT_iD", "inherited-leak"),
            ("tool_call_id", "inherited-leak"),
            ("W3_ENV_SAFETY_KEEP", "kept"),
        ]);
        let mut cmd = CommandBuilder::new("env");
        assert!(
            cmd.get_env("RoOt_ReQuEsT_iD").is_some(),
            "前置条件：继承环境应含大小写变种"
        );

        scrub_request_context_env(&mut cmd);

        assert_eq!(cmd.get_env("RoOt_ReQuEsT_iD"), None, "继承变种必须被清洗");
        assert_eq!(cmd.get_env("tool_call_id"), None, "继承变种必须被清洗");
        assert_eq!(
            cmd.get_env("W3_ENV_SAFETY_KEEP"),
            Some(OsStr::new("kept")),
            "普通变量不得被误删"
        );
        assert!(cmd.get_env("PATH").is_some(), "PATH 不得被清洗");
    }

    #[test]
    fn scrub_is_idempotent_on_already_clean_command() {
        let mut cmd = CommandBuilder::new("env");
        cmd.env("W3_ENV_SAFETY_KEEP", "kept");
        scrub_request_context_env(&mut cmd);
        scrub_request_context_env(&mut cmd);
        assert_eq!(cmd.get_env("W3_ENV_SAFETY_KEEP"), Some(OsStr::new("kept")));
    }
}
