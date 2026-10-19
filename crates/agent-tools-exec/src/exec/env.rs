//! 子进程环境构造：派生命令前剔除不可继承的宿主上下文键。
//!
//! 分层与调用顺序：键名判定（[`is_non_inheritable_env_key`]）→ 环境映射构造
//! （[`create_child_env`]，映射级过滤）→ 派生前整体重建（`process::run` 里的
//! `env_clear` + `envs`）。过滤只发生在环境构造这一处，不引入新的工具运行时。
//!
//! 判定按 **ASCII 大小写不敏感** 的整键名比较：`Trace_Id`、`trace_id` 这类变种
//! 与 `TRACE_ID` 同样被剔除；前缀/后缀相似但键名不同的变量（如 `TRACE_ID_EXTRA`）
//! 不受影响。非 UTF-8 键名无法与 ASCII 键名相等，原样透传。

use std::collections::HashMap;
use std::ffi::OsString;

/// 模型可见子进程不得继承的宿主上下文键（会话消息/根请求/工具调用/链路标识）。
pub(crate) const NON_INHERITABLE_ENV_KEYS: &[&str] = &[
    "CONVERSATION_MESSAGE_ID",
    "ROOT_REQUEST_ID",
    "TOOL_CALL_ID",
    "TRACE_ID",
];

/// 键名是否为不可继承键：整键名、ASCII 大小写不敏感。
pub(crate) fn is_non_inheritable_env_key(name: &str) -> bool {
    NON_INHERITABLE_ENV_KEYS
        .iter()
        .any(|restricted| restricted.eq_ignore_ascii_case(name))
}

/// 从继承环境派生子进程环境：整体剔除不可继承键，其余键值原样保留。
///
/// `PATH`/`HOME` 等 shell 必需变量照常继承；非 UTF-8 的键名与值不经字符串化，
/// 原样透传（[`OsString::to_str`] 失败即不可能命中 ASCII 键名）。
pub(crate) fn create_child_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> HashMap<OsString, OsString> {
    let mut env: HashMap<OsString, OsString> = vars.into_iter().collect();
    env.retain(|name, _| !name.to_str().is_some_and(is_non_inheritable_env_key));
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    /// 便于构造：`&str` 对 → `OsString` 对。
    fn pairs(vars: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        vars.iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    /// 键名排序后比较，避免 HashMap 顺序不稳定。
    fn sorted_keys(env: &HashMap<OsString, OsString>) -> Vec<String> {
        let mut keys = env
            .keys()
            .map(|key| key.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    #[test]
    fn four_keys_are_dropped_with_all_ascii_case_variants() {
        let env = create_child_env(pairs(&[
            ("CONVERSATION_MESSAGE_ID", "conversation"),
            ("Conversation_Message_Id", "conversation-mixed"),
            ("conversation_message_id", "conversation-lower"),
            ("ROOT_REQUEST_ID", "root"),
            ("Root_Request_Id", "root-mixed"),
            ("TOOL_CALL_ID", "tool"),
            ("Tool_Call_Id", "tool-mixed"),
            ("TRACE_ID", "trace"),
            ("Trace_Id", "trace-mixed"),
            ("trace_id", "trace-lower"),
            ("PATH", "/usr/bin"),
            ("HOME", "/home/tester"),
            ("AGENT_TOOLS_EXEC_ENV_CANARY", "canary"),
        ]));
        assert_eq!(
            sorted_keys(&env),
            vec!["AGENT_TOOLS_EXEC_ENV_CANARY", "HOME", "PATH"]
        );
    }

    #[test]
    fn ordinary_variables_survive_including_near_miss_names() {
        // 前缀/后缀相似但键名不同 ⇒ 不是不可继承键，必须保留。
        let env = create_child_env(pairs(&[
            ("TRACE_ID_EXTRA", "keep"),
            ("MY_TRACE_ID", "keep"),
            ("TOOL_CALL_IDX", "keep"),
            ("ROOT_REQUEST_IDS", "keep"),
            ("CONVERSATION_MESSAGE_IDS", "keep"),
            ("PATH", "/usr/bin"),
            ("HOME", "/root"),
        ]));
        assert_eq!(
            sorted_keys(&env),
            vec![
                "CONVERSATION_MESSAGE_IDS",
                "HOME",
                "MY_TRACE_ID",
                "PATH",
                "ROOT_REQUEST_IDS",
                "TOOL_CALL_IDX",
                "TRACE_ID_EXTRA",
            ]
        );
    }

    #[test]
    fn non_utf8_names_and_values_pass_through_untouched() {
        let name = OsString::from_vec(vec![b'X', 0xff, b'Y']);
        let value = OsString::from_vec(vec![0xfe, b'z']);
        let env = create_child_env([(name.clone(), value.clone())]);
        assert_eq!(env.get(&name), Some(&value));
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn predicate_is_case_insensitive_and_whole_name_only() {
        assert!(is_non_inheritable_env_key("CONVERSATION_MESSAGE_ID"));
        assert!(is_non_inheritable_env_key("root_request_id"));
        assert!(is_non_inheritable_env_key("Tool_Call_Id"));
        assert!(is_non_inheritable_env_key("tRaCe_Id"));
        assert!(!is_non_inheritable_env_key("trace_id_extra"));
        assert!(!is_non_inheritable_env_key("traceid"));
        assert!(!is_non_inheritable_env_key(""));
    }
}
