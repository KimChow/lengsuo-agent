//! 运行时配置 v0（开工说明 §2/§4：默认超时、默认模式）。
//!
//! 字段名冻结为 camelCase 序列化；`config:set` 走 `apply_patch`，
//! 未知字段必须报错（不静默忽略），非法值同样拒绝，失败的 set 不改动现值。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::CoreError;
use crate::schema::Mode;

/// 审批挂起默认超时（§4：默认 120s → deny）。
pub const DEFAULT_APPROVAL_TIMEOUT_MS: u64 = 120_000;
/// RPC 默认超时（§2：默认 30s）。
pub const DEFAULT_RPC_TIMEOUT_MS: u64 = 30_000;

/// 单回合步数上限默认值（金标 opencode `packages/schema/src/agent.ts:33` 的
/// `steps: PositiveInt.pipe(optional)` + `llm.ts:226`
/// `stepLimitReached = agent.info.steps !== undefined && step >= agent.info.steps`）。
///
/// v0 复用 agent loop 的既有熔断阈值（[`crate::r#loop::DEFAULT_MAX_STEPS`]）：
/// 配置只是把这条默认变成可覆盖的，**默认行为完全不变**（本卡的兼容性硬约束）。
pub const DEFAULT_MAX_TURN_STEPS: usize = crate::r#loop::DEFAULT_MAX_STEPS;

/// 运行时配置（v0 全量字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConfig {
    pub approval_timeout_ms: u64,
    pub rpc_timeout_ms: u64,
    pub default_mode: Mode,
    /// 跨会话记忆开关（product-direction §3：默认关，显式开启）。
    #[serde(default)]
    pub memory_enabled: bool,
    /// 单回合步数上限（一步 = 一次逻辑 LLM 调用）。
    ///
    /// 语义与金标 `agent.steps` 一致：达上限仍不收敛 ⇒ 末步缴械（见
    /// `runtime/turns.rs`）。`#[serde(default)]` 保证旧载荷（无该字段）反序列化
    /// 回默认值；`apply_patch` 侧按 `PositiveInt` 口径校验（正整数，
    /// 0/负数/非整数一律 `InvalidConfig` 且整体失败、原配置不变）。
    #[serde(default = "default_max_turn_steps")]
    pub max_turn_steps: usize,
    /// 语气风格（x-tone-style-wire）：空串 = 无风格（`<tone_and_style>` 整段省略）。
    ///
    /// 取值可为七个风格枚举名之一（大小写不敏感，`-` / `_` 与无分隔等价），
    /// 也可为自定义自由文本；归一化由 `agent_prompts::style::body_of` 完成。
    #[serde(default)]
    pub tone_style: String,
    /// 用户自定义规则（参考端 `personalization.customPrompt`）。
    ///
    /// 非空 ⇒ 提示词渲染 `## 用户自定义规则` 段；空 ⇒ 整段省略。
    /// 参考端只做 `trim()`，**不设长度上限**，本仓同形（仅拒控制字符）。
    #[serde(default)]
    pub user_rules: String,
    /// 回复语言（参考端 `ResponseLanguage`）。
    ///
    /// 空串 = 未显式声明 ⇒ 按产品默认地域取中文（参考端 `isOversea=false` 且系统
    /// locale 为 zh 的分支）；显式给 `zh*` ⇒ 中文；其余 ⇒ 英文兜底指令。
    /// 本仓**不读系统 locale**（避免引入平台依赖），故只有「显式值」与「默认中文」两态。
    #[serde(default)]
    pub response_language: String,
}

/// serde 缺字段默认值（与 [`RuntimeConfig::default_v0`] 保持一致）。
fn default_max_turn_steps() -> usize {
    DEFAULT_MAX_TURN_STEPS
}

impl RuntimeConfig {
    /// v0 默认值。
    pub fn default_v0() -> Self {
        Self {
            approval_timeout_ms: DEFAULT_APPROVAL_TIMEOUT_MS,
            rpc_timeout_ms: DEFAULT_RPC_TIMEOUT_MS,
            default_mode: Mode::Ask,
            memory_enabled: false,
            max_turn_steps: DEFAULT_MAX_TURN_STEPS,
            tone_style: String::new(),
            user_rules: String::new(),
            response_language: String::new(),
        }
    }

    /// 应用补丁：只接受已知字段；值非法则整体失败（原配置不变）。
    pub fn apply_patch(&self, patch: &Value) -> Result<Self, CoreError> {
        let Value::Object(map) = patch else {
            return Err(CoreError::InvalidConfig(
                "config:set 载荷必须是 JSON 对象".into(),
            ));
        };
        let mut next = self.clone();
        for (key, value) in map {
            match key.as_str() {
                "approvalTimeoutMs" => next.approval_timeout_ms = positive_ms(key, value)?,
                "rpcTimeoutMs" => next.rpc_timeout_ms = positive_ms(key, value)?,
                "maxTurnSteps" => next.max_turn_steps = positive_usize(key, value)?,
                "memoryEnabled" => {
                    let b = value.as_bool().ok_or_else(|| {
                        CoreError::InvalidConfig(format!("memoryEnabled 非法: {value}"))
                    })?;
                    next.memory_enabled = b;
                }
                "toneStyle" => {
                    let s = value.as_str().ok_or_else(|| {
                        CoreError::InvalidConfig(format!("toneStyle 非法: {value}"))
                    })?;
                    if s.chars().any(char::is_control) {
                        return Err(CoreError::InvalidConfig("toneStyle 不得含控制字符".into()));
                    }
                    next.tone_style = s.to_string();
                }
                "userRules" => next.user_rules = multiline_string(key, value)?,
                "responseLanguage" => next.response_language = plain_string(key, value)?,
                "defaultMode" => {
                    let mode = serde_json::from_value::<Mode>(value.clone()).map_err(|_| {
                        CoreError::InvalidConfig(format!("defaultMode 非法: {value}"))
                    })?;
                    next.default_mode = mode;
                }
                other => return Err(CoreError::InvalidConfig(format!("未知配置字段: {other}"))),
            }
        }
        Ok(next)
    }

    /// 序列化为网关 `config:get` 的 data。
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        match serde_json::to_value(self) {
            Ok(Value::Object(fields)) => map = fields,
            Ok(_) => {}
            Err(e) => return Value::String(format!("<序列化失败: {e}>")),
        }
        Value::Object(map)
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::default_v0()
    }
}

/// 单行文本配置值：必须是 JSON 字符串，且不得含任何控制字符。
///
/// 用于 `responseLanguage` 这类「标识符式」取值（`zh-CN` / `en-US`）。
fn plain_string(key: &str, value: &Value) -> Result<String, CoreError> {
    let s = text_of(key, value)?;
    if s.chars().any(char::is_control) {
        return Err(CoreError::InvalidConfig(format!("{key} 不得含控制字符")));
    }
    Ok(s.to_string())
}

/// 多行文本配置值：必须是 JSON 字符串；允许换行/回车/制表符，其余控制字符拒绝。
///
/// 用于 `userRules`（用户自定义规则本就可能是多行清单）；参考端对该字段只做
/// `trim()`，不设长度上限 —— 本仓保持同样宽松，只拦控制字符。
fn multiline_string(key: &str, value: &Value) -> Result<String, CoreError> {
    let s = text_of(key, value)?;
    if s.chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(CoreError::InvalidConfig(format!(
            "{key} 不得含控制字符（换行/制表符除外）"
        )));
    }
    Ok(s.to_string())
}

/// 取字符串值；非字符串一律 `InvalidConfig`（含 `null` / 数字 / 数组）。
fn text_of<'a>(key: &str, value: &'a Value) -> Result<&'a str, CoreError> {
    value
        .as_str()
        .ok_or_else(|| CoreError::InvalidConfig(format!("{key} 非法: {value}")))
}

fn positive_ms(key: &str, value: &Value) -> Result<u64, CoreError> {
    let raw = value
        .as_u64()
        .ok_or_else(|| CoreError::InvalidConfig(format!("{key} 必须是非负整数")))?;
    if raw == 0 {
        return Err(CoreError::InvalidConfig(format!("{key} 必须大于 0")));
    }
    Ok(raw)
}

/// 正整数校验（金标 `packages/schema/src/schema.ts:3` 的
/// `PositiveInt = Schema.Int.check(Schema.isGreaterThan(0))`）。
///
/// `Value::as_u64` 只在 JSON 值为**整数且 ≥ 0** 时命中：负数、浮点（含 `2.0`
/// 这类整值浮点——不是 `Schema.Int`）、字符串、布尔、null、数组/对象都落 `None`
/// ⇒ `InvalidConfig`；0 单独按「必须大于 0」拒绝。两条失败路径都发生在
/// `apply_patch` 组装 `next` 的中途，调用方（`Runtime::config_set`）不会写回，
/// 所以整体失败、原配置不变——与既有 `positive_ms` 逐条同形。
fn positive_usize(key: &str, value: &Value) -> Result<usize, CoreError> {
    let raw = value
        .as_u64()
        .ok_or_else(|| CoreError::InvalidConfig(format!("{key} 必须是非负整数")))?;
    if raw == 0 {
        return Err(CoreError::InvalidConfig(format!("{key} 必须大于 0")));
    }
    usize::try_from(raw)
        .map_err(|_| CoreError::InvalidConfig(format!("{key} 超出本平台 usize 表示范围")))
}
