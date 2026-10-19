//! 共用小工具：写/执行工具判定、结果回填消息、可见工具面、事件发射与时钟。

use serde_json::{json, Value};

use crate::mode::WRITE_TOOLS;
use crate::permission::Decision;
use crate::schema::Event;

use super::{AgentLoop, Driver};

/// 写/执行类工具判定。
pub(super) fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// 工具结果消息（大结果由宿主落盘为引用，本层只回填 JSON；仅本地路径用）。
pub(super) fn tool_result(id: &str, name: &str, result: &Value, is_error: bool) -> Value {
    json!({
        "role": "tool_result",
        "content": result.to_string(),
        "tool": {"id": id, "name": name, "isError": is_error},
    })
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

impl<'a> AgentLoop<'a> {
    /// 本次请求可见的工具定义。
    ///
    /// 历史真相源不同，取法也就不同（见 [`Driver`]）：
    /// - 本地路径：工具面就是当前 [`crate::mode::ModePolicy`] 的白名单；
    /// - `Runtime` 路径：**与 `Runtime::step` 实际会用的那一份保持一致**——取
    ///   [`crate::runtime::Runtime::check_tool`] 在同一个执行器上的判定结果，逐个
    ///   白名单工具问「现在可不可见」，而不是复刻一遍权限门的规则（工具未注册、
    ///   未白名单、规划态写工具被拒等情形都因此自动落在同一条判定上）。
    pub(super) fn tool_defs(&self) -> Vec<Value> {
        match &self.driver {
            Driver::Local { .. } => self
                .policy
                .tools
                .iter()
                .map(|name| json!({"name": name}))
                .collect(),
            Driver::Runtime {
                runtime,
                session,
                step_executor,
            } => {
                // 未借到执行器 ⇒ 本层无从得知 Runtime 实际会用的工具面，按空处理
                // （只影响请求里的工具定义，不影响工具执行）。
                if step_executor.is_none() {
                    return Vec::new();
                }
                self.policy
                    .tools
                    .iter()
                    .filter(|name| {
                        matches!(
                            runtime.check_tool(*session, name, &Value::Null),
                            Ok(Decision::Allow)
                        )
                    })
                    .map(|name| json!({"name": name}))
                    .collect()
            }
        }
    }

    /// 发一个事件（seq 会话内单调递增）。
    pub(super) fn emit(&mut self, kind: &str, payload: Value) {
        self.seq += 1;
        (self.emit)(Event {
            seq: self.seq,
            session_id: self.session_id,
            kind: kind.to_string(),
            payload,
            ts_ms: now_ms(),
        });
    }
}
