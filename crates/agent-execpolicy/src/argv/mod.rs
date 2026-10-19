//! argv 级进程策略评估（对照参考实现 execpolicy 的
//! PrefixPattern/PrefixRule/RuleMatch/Evaluation 分层转写）。
//!
//! 与既有字符串版 [`crate::Policy`]（file/process/network 三段、首个命中）完全并存、互不影响：
//!
//! - 输入是**已分词**的 argv（`&[String]`/`Vec<String>`）。本模块不做任何 shell 串切分：
//!   token 含空格是合法内容，比较永远整体进行；调用方必须用真实 argv 语义分词后提交。
//! - 按前缀规则匹配，**所有命中规则取最严决策**（序 = 严格度：Allow < Ask < Deny，
//!   对照参考 Allow < Prompt < Forbidden）：先写 allow 不会掩盖后到的 deny。
//! - 未命中不猜：空 argv、`sh -c`/脚本解释器嵌套的内层命令都不因外层规则放行，
//!   是否放行由调用方传入的启发式回退闭包决定（本卡约定恒 [`Decision::Ask`]，
//!   fail-closed）。
//!
//! 未搬入参考件的能力（Starlark parser、network_rule、match/not_match 示例校验、
//! amend、sandbox 迁移、绝对路径新类型）见 docs/w3-policy-command-argv-status.md
//! 偏离清单；本模块不声称完整参考 execpolicy 兼容。

mod executable_name;
mod policy;
mod rule;

pub use policy::{Evaluation, MatchOptions, Policy, RequirementsExecPolicy};
pub use rule::{PatternToken, PrefixPattern, PrefixRule, Rule, RuleMatch, RuleRef};

use std::fmt;

/// argv 策略层的构造/校验错误（对照参考 error.rs 的本卡用到的子集；文案与参考一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 模式非法（如空前缀）。
    InvalidPattern(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidPattern(message) => write!(f, "invalid pattern element: {message}"),
        }
    }
}

impl std::error::Error for Error {}
