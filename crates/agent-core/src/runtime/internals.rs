//! 内部辅助：消息落库、事件发布、脱敏（pub(crate) 供同 crate 复用）。

use serde_json::Value;
use uuid::Uuid;

use crate::error::CoreError;
use crate::schema::{Event, Message, Role, Session, SessionStatus, ToolInfo};

use super::{now_ms, Runtime};

impl Runtime {
    /// 落一条消息进会话历史。**消息时间由调用方取一次时钟读数并显式传入**，与随后
    /// 事件 payload 的 `"tsMs"` 用**同一次**读数（`turn_ts()` 的来由见
    /// [`Runtime::turn_ts`]）：消息自身的 `ts_ms` 因此随事件落盘，重启重建能取回真值，
    /// 而不是拿「事件落盘时刻」冒充「消息产生时刻」。
    pub(super) fn push_message(
        &self,
        id: Uuid,
        role: Role,
        content: String,
        tool: Option<ToolInfo>,
        ts_ms: u64,
    ) -> Result<Session, CoreError> {
        let mut guard = self.sessions.lock().expect("会话表锁不得毒化");
        let Some(hot) = guard.get_mut(&id) else {
            return Err(CoreError::SessionNotFound(id));
        };
        if matches!(
            hot.session.status,
            SessionStatus::Done | SessionStatus::Failed
        ) {
            return Err(CoreError::SessionClosed(id));
        }
        hot.session.messages.push(Message {
            role,
            content,
            tool,
            ts_ms,
        });
        hot.session.updated_at_ms = now_ms();
        Ok(hot.session.clone())
    }

    /// 落盘 + 广播一条事件（脱敏后）。
    pub(super) fn emit(
        &self,
        session: &Session,
        kind: &str,
        payload: Value,
    ) -> Result<Event, CoreError> {
        let payload = self.redactor.redact_json(&payload);
        let event = self.log.append(session.id, kind, payload)?;
        if let Ok(mut guard) = self.sessions.lock() {
            if let Some(hot) = guard.get_mut(&session.id) {
                hot.session.last_seq = hot.session.last_seq.max(event.seq);
                hot.session.updated_at_ms = now_ms();
            }
        }
        self.hub
            .bus(&session.isolation.key())
            .publish(event.clone());
        Ok(event)
    }

    pub(super) fn redact(&self, text: &str) -> String {
        self.redactor.redact_str(text)
    }
}
