//! 判定结果 `Decision` 与冻结的 reason 词汇表（规格 §2.1、任务卡 D3）。
//!
//! `reasons[0]` 的文案是审计展示面，逐字冻结；测试按 `assert_eq!` 比对，
//! 新增文案必须先进入词汇表，不允许调用处临时拼接。

/// 规则身份：`PolicySpec.rules` 的下标（规则序即身份）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleItemId(usize);

impl RuleItemId {
    /// 由 `PolicySpec.rules` 下标构造（判定层内部使用）。
    pub(crate) const fn new(index: usize) -> Self {
        Self(index)
    }

    /// `PolicySpec.rules` 中的下标。
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// 判定结果（规格 §2.1）。不是线格式类型，因此不实现 serde。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// 是否放行。
    pub allowed: bool,
    /// 参与结论的规则；无命中规则时为 `None`。
    pub matched_rule: Option<RuleItemId>,
    /// 判定原因；`[0]` 来自冻结词汇表。
    pub reasons: Vec<String>,
    /// 是否记录访问行为（规格 §1：`active_range` 存在时禁用）。
    pub should_log: bool,
}

impl Decision {
    /// 拒绝结论；`matched_rule` 由知道命中规则的调用处补。
    #[must_use]
    pub fn deny(reason: &str) -> Self {
        Self {
            allowed: false,
            matched_rule: None,
            reasons: vec![reason.to_string()],
            should_log: false,
        }
    }

    /// 放行结论；`matched_rule` 由知道命中规则的调用处补。
    #[must_use]
    pub fn allow(reason: &str) -> Self {
        Self {
            allowed: true,
            matched_rule: None,
            reasons: vec![reason.to_string()],
            should_log: false,
        }
    }
}

// ---- fail-closed 拒绝词汇（前缀固定 `fail-closed: `）----

/// 生效对象为空。
pub const REASON_TARGETS_EMPTY: &str = "fail-closed: 生效对象为空";
/// 生效对象不匹配。
pub const REASON_TARGETS_MISMATCH: &str = "fail-closed: 生效对象不匹配";
/// 生效时间窗不在窗内。
pub const REASON_OUT_OF_WINDOW: &str = "fail-closed: 生效时间窗不在窗内";
/// 规则集为空。
pub const REASON_NO_RULES: &str = "fail-closed: 规则集为空";
/// 规则模式编译失败。
pub const REASON_BAD_PATTERN: &str = "fail-closed: 规则模式编译失败";
/// 请求 URL 非法。
pub const REASON_BAD_URL: &str = "fail-closed: 请求 URL 非法";
/// 未命中任何规则（白名单 / 特殊案例模式的兜底）。
pub const REASON_NO_MATCH: &str = "fail-closed: 未命中任何规则";
/// 操作未授权。
pub const REASON_ACTION_DENIED: &str = "fail-closed: 操作未授权";
/// 点击操作缺少选择器。
pub const REASON_SELECTOR_MISSING: &str = "fail-closed: 点击操作缺少选择器";
/// 点击操作选择器未授权。
pub const REASON_SELECTOR_DENIED: &str = "fail-closed: 点击操作选择器未授权";

// ---- 放行 / 黑名单命中词汇（不带 `fail-closed: ` 前缀）----

/// 命中黑名单规则（`is_accessible == false`）。
pub const REASON_BLACKLIST_HIT: &str = "命中黑名单规则";
/// 全放行模式。
pub const REASON_ALL_MODE: &str = "全放行模式";
/// 部分黑名单模式未命中黑名单规则。
pub const REASON_SOME_BLACK_MISS: &str = "部分黑名单模式未命中黑名单规则";
/// 规则允许。
pub const REASON_RULE_ALLOWED: &str = "规则允许";
