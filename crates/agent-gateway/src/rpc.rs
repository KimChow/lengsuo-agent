use crate::server::App;
use agent_core::{
    error::CoreError,
    runtime::CreateSession,
    schema::{Mode, PermissionMode},
};
use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// `session:replay` 缺省分页大小；上限由 core 的 `DEFAULT_PAGE_LIMIT` 钳制。
const REPLAY_DEFAULT_LIMIT: u64 = 200;

/// 可走 ACP 旁路的动作白名单：**只有**挂上 [`AcpHostHandle`] 且动作在此表内时，
/// `run()` 才先尝试 ACP 转发。
///
/// 语义要点：
/// - **默认不启用**：`App` 的 `acp` 字段为 `None`（默认）时，本表**完全不生效**，
///   既有 11 个动作的实现路径逐字节不变（零回归）；
/// - **不发明映射**：表里的名字即 v0 冻结动作名，转发时**原样**当 ACP method
///   （见 `crates/agent-gateway/src/acp.rs` 的 `forward_action`），
///   **不做任何名字改写**——不发明 `域:动作` ↔ `session/*` 映射表；
/// - 本表是**显式**的，故「哪些动作走 ACP」由部署时拍板，不静默改变既有语义。
///
/// # 初步白名单（本卡落地）
///
/// 只放 `sys:ping`，理由是它是「**接线真的通**」的最小、最低风险可验证样本：
/// - **无副作用**：既有实现只回 `{"pid": …}`，纯读、不碰会话/配置/审批状态，
///   即便转发语义日后调整也不产生持久影响；
/// - **可判读**：应答一眼可辨真伪（对端回的载荷与既有实现不同即证明走了 ACP），
///   是「挂 handle ⇒ 旁路生效」这条链路的端到端探针
///   （见 `tests/acp_forward_whitelist.rs`）。
///
/// **其余 10 个动作本卡不动**（为何暂不放）：它们要么**改状态**、要么**带归属/
/// 审批校验**——`session:create/get/send/stop/replay` 涉及会话生命周期与隔离归属，
/// `approval:respond` 涉及安全决策，`config:set` 改运行配置，`tool:list` 受
/// permission gate 过滤——一旦原样转发给 ACP method，等于把这些语义**整体搬到对端**，
/// 而「哪些动作该搬、对端如何复刻同样的校验」需先有**方法面台账**（见
/// `docs/contracts/acp-host.md` §7 决策项 2）。KICKOFF 的「只增不改」也要求
/// 逐个评估、而非一次性切走。故本卡只放无副作用的探针动作，留口子给后续卡。
pub const ACP_FORWARD_ACTIONS: &[&str] = &["sys:ping"];

#[derive(Deserialize)]
pub(crate) struct Request {
    action: String,
    #[serde(default)]
    payload: Value,
}
pub(crate) async fn handle(
    State(app): State<App>,
    input: Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Json<Value> {
    let Ok(Json(req)) = input else {
        return Json(envelope(400, "invalid JSON", Value::Null));
    };
    let timeout = Duration::from_millis(app.runtime.config_get().rpc_timeout_ms);
    let work = tokio::task::spawn_blocking(move || dispatch(&app, &req.action, &req.payload));
    Json(match tokio::time::timeout(timeout, work).await {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => envelope(500, "request failed", Value::Null),
        Err(_) => envelope(408, "request timed out", Value::Null),
    })
}
fn envelope(code: u16, msg: &str, data: Value) -> Value {
    json!({"code":code,"msg":msg,"data":data})
}
fn dispatch(app: &App, action: &str, p: &Value) -> Value {
    match run(app, action, p) {
        Ok(data) => envelope(0, "ok", data),
        Err((code, msg)) => envelope(code, &msg, Value::Null),
    }
}
type Failure = (u16, String);
fn bad() -> Failure {
    (400, "invalid payload".into())
}
fn core(e: CoreError) -> Failure {
    match e {
        CoreError::SessionNotFound(_) | CoreError::ApprovalNotFound(_) => (404, "not found".into()),
        CoreError::InvalidConfig(_) => (400, "invalid configuration".into()),
        CoreError::SessionClosed(_) => (409, "session closed".into()),
        _ => (500, "operation failed".into()),
    }
}
fn id(p: &Value, key: &str) -> Result<Uuid, Failure> {
    p.get(key)
        .or_else(|| p.get("sessionId"))
        .or_else(|| p.get("session_id"))
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(bad)
}
fn own(app: &App, p: &Value) -> Result<Uuid, Failure> {
    let id = id(p, "id")?;
    let s = app.runtime.get_session(id).map_err(core)?;
    if s.isolation != app.isolation {
        return Err((403, "forbidden".into()));
    }
    Ok(id)
}
/// ACP 旁路：挂了 [`AcpHostHandle`] 且动作在 [`ACP_FORWARD_ACTIONS`] 内时，先把
/// 动作名**原样**投给 host 方法表（`forward_action`），成功即返回其载荷。
///
/// 返回 `Ok(Some(data))` = 已由 ACP 应答；`Ok(None)` = 不走旁路，落回既有实现。
/// 未走成旁路的两种情况：
/// - 未挂 handle、动作不在白名单 → `None`（既有路径不变）；
/// - 转发出错（超时/连接终止）→ 返回 `Err`，由 `dispatch` 归入传输错误。
///
/// # Errors
/// 转发传输失败（[`agent_acp::Error::Transport`]）→ `(502, "acp forwarding failed")`。
fn acp_forward(app: &App, action: &str, p: &Value) -> Result<Option<Value>, Failure> {
    let Some(handle) = app.acp.as_ref() else {
        return Ok(None);
    };
    if !ACP_FORWARD_ACTIONS.contains(&action) {
        return Ok(None);
    }
    handle
        .forward_action(action, p)
        .map(|outcome| Some(outcome.envelope))
        .map_err(|_| (502, "acp forwarding failed".into()))
}

fn run(app: &App, action: &str, p: &Value) -> Result<Value, Failure> {
    if let Some(data) = acp_forward(app, action, p)? {
        return Ok(data);
    }
    let rt = &app.runtime;
    Ok(match action {
        "sys:ping" => json!({"pid":std::process::id()}),
        "config:get" => {
            let config = rt.config_get().to_value();
            if let Some(key) = p.get("key").and_then(Value::as_str) {
                config.get(key).cloned().ok_or_else(bad)?
            } else {
                config
            }
        }
        "config:set" => {
            if let Some(key) = p.get("key").and_then(Value::as_str) {
                let value = p.get("value").cloned().ok_or_else(bad)?;
                rt.config_set(&json!({key:value})).map_err(core)?.to_value()
            } else {
                rt.config_set(p).map_err(core)?.to_value()
            }
        }
        "session:list" => json!(rt.list_sessions(&app.isolation)),
        "session:create" => {
            // Identity and environment authority are supplied by the host, never the RPC caller.
            if p.get("isolation").is_some()
                || p.get("envBinding").is_some()
                || p.get("env_binding").is_some()
            {
                return Err((403, "server-bound identity".into()));
            }
            let mode = p
                .get("mode")
                .map(|v| serde_json::from_value::<Mode>(v.clone()).map_err(|_| bad()))
                .transpose()?
                .unwrap_or(rt.config_get().default_mode);
            let permission_mode = p
                .get("permissionMode")
                .map(|v| serde_json::from_value::<PermissionMode>(v.clone()).map_err(|_| bad()))
                .transpose()?
                .unwrap_or_default();
            json!(rt
                .create_session(&CreateSession {
                    title: p
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("New session")
                        .to_owned(),
                    workspace_root: p
                        .get("workspaceRoot")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    mode,
                    permission_mode,
                    env_binding: app.isolation.env_id.clone(),
                    isolation: app.isolation.clone()
                })
                .map_err(core)?)
        }
        "session:get" => json!(rt.get_session(own(app, p)?).map_err(core)?),
        "session:send" => {
            let receipt = rt
                .send_text(
                    own(app, p)?,
                    p.get("text")
                        .or_else(|| p.get("content"))
                        .and_then(Value::as_str)
                        .ok_or_else(bad)?,
                )
                .map_err(core)?;
            json!({"sessionId":receipt.session_id,"events":receipt.events,"lastSeq":receipt.last_seq})
        }
        "session:stop" => json!(rt.stop_session(own(app, p)?).map_err(core)?),
        // core `Runtime::replay` 的薄暴露：`seq > after_seq` 升序一页，不做缓存/过滤/重排。
        "session:replay" => {
            let id = own(app, p)?;
            // u64::MAX 会让 core 的 `after_seq + 1` 溢出 panic，边界修复归 core lane，网关先拒收。
            let after_seq = p.get("afterSeq").and_then(Value::as_u64).ok_or_else(bad)?;
            if after_seq == u64::MAX {
                return Err(bad());
            }
            let limit = p
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(REPLAY_DEFAULT_LIMIT) as usize;
            let events = rt.replay(id, after_seq, limit).map_err(core)?;
            let last_seq = events.last().map(|e| e.seq).unwrap_or(after_seq);
            json!({"events": events, "lastSeq": last_seq})
        }
        "tool:list" => {
            // 有会话时以会话为准（并做归属校验）；无会话沿用 default_mode 兼容行为。
            let session = if p.get("id").is_some()
                || p.get("sessionId").is_some()
                || p.get("session_id").is_some()
            {
                Some(rt.get_session(own(app, p)?).map_err(core)?)
            } else {
                None
            };
            let mode = session
                .as_ref()
                .map(|s| s.mode)
                .unwrap_or_else(|| rt.config_get().default_mode);
            let gate = rt.gate();
            let mut allowed = gate.whitelist(mode);
            // readonly 会话里非只读类别不可见，与 `PermissionGate::check` 的判定一致；
            // full 只取消该遮挡，不放宽模式白名单。
            if session.is_some_and(|s| s.permission_mode == PermissionMode::ReadOnly) {
                let registry = gate.registry();
                allowed.retain(|name| {
                    registry
                        .get(name)
                        .is_some_and(|t| t.kind.allowed_when_readonly())
                });
            }
            json!(gate.registry().to_value(Some(&allowed)))
        }
        "approval:respond" => {
            let session = own(app, p)?;
            let approval = p
                .get("approvalId")
                .or_else(|| p.get("approval_id"))
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(bad)?;
            if !rt.pending_approvals(session).contains(&approval) {
                return Err((403, "forbidden".into()));
            }
            let result = rt
                .respond_approval(
                    approval,
                    p.get("approved")
                        .and_then(Value::as_bool)
                        .or_else(|| match p.get("decision").and_then(Value::as_str) {
                            Some("allow") => Some(true),
                            Some("deny") => Some(false),
                            _ => None,
                        })
                        .ok_or_else(bad)?,
                )
                .map_err(core)?;
            json!({"approved":result.approved,"timedOut":result.timed_out})
        }
        _ => return Err((404, "unknown action".into())),
    })
}
