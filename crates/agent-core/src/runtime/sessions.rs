//! 会话生命周期：创建、查询、列表、更新与状态迁移。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::CoreError;
use crate::run_status::RunEvent;
use crate::schema::{Isolation, Mode, PermissionMode, Session, SessionStatus};

use super::{now_ms, CreateSession, HotSession, Runtime};

// 四段取值的落点（h3-memory-inject-4seg 卡）：`context.rs` 据此接线 `prompt_vars()`。
pub(super) mod memory_vars;

// 专家召唤与切换（h4-expert-session-bind 卡）：包目录定位 + 注册表在 `experts`，
// 召唤校验与人格取用两条纯入口在 `expert_bind`。
pub mod expert_bind;
mod experts;

use expert_bind::BindTarget;

pub use experts::ExpertDirs;

impl Runtime {
    /// 创建会话（写 `session_update` 事件）。
    pub fn create_session(&self, input: &CreateSession) -> Result<Session, CoreError> {
        let now = now_ms();
        let mut session = Session {
            id: Uuid::new_v4(),
            title: input.title.clone(),
            workspace_root: input
                .workspace_root
                .clone()
                .unwrap_or_else(|| self.workdir.display().to_string()),
            mode: input.mode,
            permission_mode: input.permission_mode,
            env_binding: input.env_binding.clone(),
            isolation: input.isolation.clone(),
            messages: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
            status: SessionStatus::Idle,
            last_seq: 0,
            memory_digest: None,
            run_status: None,
            expert_id: None,
        };
        // 会话绑定环境时，隔离键 env_id 必须与绑定一致（环境隔离边界）。
        if let Some(env) = &session.env_binding {
            session.isolation.env_id = Some(env.clone());
        }
        // 记忆注入（agent-memory-spec §4）：增强项，失败不阻断建会话。
        if self.config.lock().expect("配置锁不得毒化").memory_enabled {
            match self.compose_memory_digest(&session.workspace_root) {
                Ok(digest) => session.memory_digest = Some(digest),
                Err(e) => {
                    self.emit(
                        &session,
                        "warning",
                        json!({ "kind": "memory_compose_failed", "reason": e.to_string() }),
                    )?;
                }
            }
        }
        let id = session.id;
        self.sessions.lock().expect("会话表锁不得毒化").insert(
            id,
            HotSession {
                session: session.clone(),
                drain: Arc::new(Mutex::new(())),
                // 生命周期：会话创建时取消标志初始为 false。
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        self.emit(
            &session,
            "session_update",
            json!({ "status": session.status, "title": session.title, "mode": session.mode }),
        )?;
        Ok(session)
    }

    /// 组装记忆摘要（会话快照面：单段文本，各层前 200 字符预览；总 ≤8000 字符）。
    ///
    /// 注入面（system prompt）走**四段**（见 [`memory_vars::MemorySegments`]）；
    /// 本方法只服务会话快照 `Session.memory_digest`（网关 `session:get` 的展示面），
    /// 格式与本卡之前逐字一致。
    pub(super) fn compose_memory_digest(&self, workspace_root: &str) -> Result<String, CoreError> {
        let layers = self.collect_memory_layers(workspace_root)?;
        Ok(self.compose_digest_text(&layers))
    }

    /// 记忆工具执行入口（权限门放行后由宿主调用；每次成功发 memory_update 事件）。
    pub fn execute_memory_tool(
        &self,
        id: Uuid,
        name: &str,
        args: &Value,
    ) -> Result<Value, CoreError> {
        use agent_memory::{MemoryEngine, MemoryPaths};
        let enabled = self.config.lock().expect("配置锁不得毒化").memory_enabled;
        let session = self.get_session(id)?;
        let paths = MemoryPaths::new(self.workdir.join(".lengsuo"), &session.workspace_root);
        let engine = MemoryEngine::new(paths.clone(), enabled);
        let arg = |key: &str| -> Result<String, CoreError> {
            args.get(key)
                .and_then(Value::as_str)
                .map(|text| self.redact(text))
                .ok_or_else(|| CoreError::InvalidConfig(format!("缺少参数 {key}")))
        };
        let kind = match name {
            "memory.append_daily" => {
                let date = chrono::NaiveDate::parse_from_str(&arg("date")?, "%Y-%m-%d")
                    .map_err(|_| CoreError::InvalidConfig("date 非法（YYYY-MM-DD）".into()))?;
                let paths = agent_memory::MemoryPaths::new(
                    self.workdir.join(".lengsuo"),
                    &session.workspace_root,
                );
                if !enabled {
                    return Err(agent_memory::MemoryError::Disabled.into());
                }
                agent_memory::append_daily(&paths, date, &arg("text")?)?;
                "daily_append"
            }
            "memory.set_user" => {
                engine.set_user_memory(&arg("text")?)?;
                "user_memory"
            }
            "memory.set_project" => {
                engine.set_project_memory(&arg("text")?)?;
                "project_memory"
            }
            "identity.set_soul" => {
                engine.set_soul(&arg("text")?)?;
                "soul"
            }
            // 检索三工具只读：直接返回后端响应，不写 memory_update 事件。
            "memory.list" => {
                let request =
                    crate::tool::parse_memory_list_args(args).map_err(CoreError::InvalidConfig)?;
                return Ok(serde_json::to_value(agent_memory::list::list(
                    &paths, request,
                )?)?);
            }
            "memory.search" => {
                let mut request = crate::tool::parse_memory_search_args(args)
                    .map_err(CoreError::InvalidConfig)?;
                // 与写路径同口径：查询串可能含凭据，先脱敏再检索（D5）。
                request.queries = request.queries.iter().map(|q| self.redact(q)).collect();
                return Ok(serde_json::to_value(agent_memory::search::search(
                    &paths, request,
                )?)?);
            }
            "memory.read" => {
                let request =
                    crate::tool::parse_memory_read_args(args).map_err(CoreError::InvalidConfig)?;
                return Ok(serde_json::to_value(agent_memory::read::read(
                    &paths, request,
                )?)?);
            }
            other => return Err(CoreError::InvalidConfig(format!("未知记忆工具: {other}"))),
        };
        self.emit(&session, "memory_update", json!({ "kind": kind }))?;
        Ok(json!({ "ok": true, "kind": kind }))
    }

    /// 取会话快照。
    pub fn get_session(&self, id: Uuid) -> Result<Session, CoreError> {
        let guard = self.sessions.lock().expect("会话表锁不得毒化");
        let Some(hot) = guard.get(&id) else {
            return Err(CoreError::SessionNotFound(id));
        };
        Ok(hot.session.clone())
    }

    /// 按隔离键列会话（摘要，不带消息体）。
    ///
    /// 比较完整 `Isolation`（用户/租户/环境），不用 `key()`：后者是
    /// `user_id-tenant_id` 拼接且不含 env，分段歧义会串列表。
    pub fn list_sessions(&self, isolation: &Isolation) -> Vec<Session> {
        let guard = self.sessions.lock().expect("会话表锁不得毒化");
        guard
            .values()
            .filter(|h| &h.session.isolation == isolation)
            .map(|h| h.session.clone())
            .collect()
    }

    /// 会话摘要列表（网关 `session:list` 的 data）。
    pub fn list_session_summaries(&self, isolation: &Isolation) -> Vec<Value> {
        self.list_sessions(isolation)
            .iter()
            .map(Session::summary)
            .collect()
    }

    /// 更新会话字段（模式/权限模式/标题/环境绑定/专家绑定；`None` 表示不改）。
    ///
    /// `expert_id` 沿用 `env_binding` 的 `Option<Option<T>>` 三态惯例：外层 `None` =
    /// 不改，`Some(None)` = 清空，`Some(Some(id))` = 绑定。`workspace_root` 是召唤时
    /// 解析专家双根的基底（`None` ⇒ 取会话自己的工作区）。
    ///
    /// 参数表已到「可 patch 集」的上限（§10.2 明写只扩这一个参数）；后续再加可 patch
    /// 字段应改为补丁结构体，而不是继续加位置参数——那是另起一张卡的事。
    #[allow(clippy::too_many_arguments)]
    pub fn update_session(
        &self,
        id: Uuid,
        title: Option<&str>,
        mode: Option<Mode>,
        permission_mode: Option<PermissionMode>,
        env_binding: Option<Option<&str>>,
        expert_id: Option<Option<&str>>,
        workspace_root: Option<&str>,
    ) -> Result<Session, CoreError> {
        // 召唤是**先校验、后写字段**（I3：失败可重试）：校验在会话表锁之外完成
        // （[`Runtime::bind_expert`] 只做定位与校验、不碰会话表），失败时会话一个
        // 字段都不动、也不写事件。
        let mut clear_expert = false;
        let bind_target = match expert_id {
            Some(Some(expert)) => {
                let dirs = self.expert_dirs(id, workspace_root)?;
                Some(self.bind_expert(&dirs, expert)?)
            }
            Some(None) => {
                clear_expert = true;
                None
            }
            None => None,
        };
        let mut guard = self.sessions.lock().expect("会话表锁不得毒化");
        let Some(hot) = guard.get_mut(&id) else {
            return Err(CoreError::SessionNotFound(id));
        };
        if let Some(title) = title {
            hot.session.title = title.to_string();
        }
        // 清 expert_id ⇒ 回落 Craft；设 expert_id ⇒ 强制 Expert（§10.2 步骤 1-2：
        // 专家是 expert 模式的实例；expert 模式无所指时回落 craft 更诚实）。
        if clear_expert {
            hot.session.expert_id = None;
            hot.session.mode = Mode::Craft;
        }
        if let Some(target) = &bind_target {
            hot.session.expert_id = Some(target.expert_id.clone());
            hot.session.mode = Mode::Expert;
        }
        // 联动方向只在这一处实现：同批给的 `mode` 不得覆盖专家联动的结论。
        if let Some(mode) = mode.filter(|_| expert_id.is_none()) {
            hot.session.mode = mode;
        }
        if let Some(pm) = permission_mode {
            hot.session.permission_mode = pm;
        }
        if let Some(env) = env_binding {
            hot.session.env_binding = env.map(str::to_string);
        }
        hot.session.updated_at_ms = now_ms();
        let session = hot.session.clone();
        drop(guard);
        self.emit(
            &session,
            "session_update",
            json!({
                "status": session.status,
                "title": session.title,
                "mode": session.mode,
                "expertId": session.expert_id,
            }),
        )?;
        Ok(session)
    }

    /// 会话工作区 ⇒ 专家双根（调用方可用 `workspace_root` 覆盖，如 `/专家名` 在工作区之外触发）。
    fn expert_dirs(&self, id: Uuid, workspace_root: Option<&str>) -> Result<ExpertDirs, CoreError> {
        let workspace = match workspace_root {
            Some(root) => root.to_string(),
            None => {
                let guard = self.sessions.lock().expect("会话表锁不得毒化");
                let Some(hot) = guard.get(&id) else {
                    return Err(CoreError::SessionNotFound(id));
                };
                hot.session.workspace_root.clone()
            }
        };
        Ok(ExpertDirs::for_session(workspace))
    }

    /// 召唤校验（纯判定，不碰会话表）：扫盘建索引 ⇒ 校验并读出人格。
    ///
    /// 成功只返回 [`BindTarget`]；写 `expert_id` 由 [`Runtime::update_session`] 在
    /// 校验通过之后做（I3：失败可重试，不落「半个专家」）。
    pub fn bind_expert(&self, dirs: &ExpertDirs, expert: &str) -> Result<BindTarget, CoreError> {
        let mut target = self.resolve_expert(dirs, expert)?;
        // 人格进模型输入前先过脱敏（AGENTS §6：凭据不进上下文）。
        target.persona = self.redact(&target.persona);
        Ok(target)
    }

    /// 召唤校验的**未脱敏**形态（[`Runtime::bind_expert`] 在它之上补脱敏）。
    fn resolve_expert(&self, dirs: &ExpertDirs, expert: &str) -> Result<BindTarget, CoreError> {
        BindTarget::resolve(dirs, expert)
    }

    /// 停止回合：状态回 idle（不销毁历史）。
    ///
    /// 与 [`Runtime::cancel_session`] 是**两个并存的动作**：本方法只改状态、
    /// 不设取消标志；取消只置标志、不改状态。
    pub fn stop_session(&self, id: Uuid) -> Result<Session, CoreError> {
        self.set_status(id, SessionStatus::Idle)
    }

    /// 回合收口：运行态 `turn_completed → idle`（wb 26 册 §1.2）。
    ///
    /// `SessionStatus` 自建会话起即为 `Idle`（本仓回合不置 `Running`，故这里
    /// 不改可观测状态、也不发 `session_update`）；`Session.run_status` 已接线时
    /// 按转移表推进，未接线（本卡默认 `None`）时保持原样——运行状态机的持久化
    /// 与恢复后置到状态机卡，与「不做 session_resume」的本卡边界一致。
    pub(super) fn settle_turn_completed(&self, id: Uuid) {
        let mut guard = self.sessions.lock().expect("会话表锁不得毒化");
        let Some(hot) = guard.get_mut(&id) else {
            return;
        };
        hot.session.status = SessionStatus::Idle;
        if let Some(status) = hot.session.run_status.as_mut() {
            status.apply(RunEvent::TurnCompleted, now_ms());
        }
        hot.session.updated_at_ms = now_ms();
    }

    /// 结束会话：`done` 后拒绝新输入。
    pub fn finish_session(&self, id: Uuid) -> Result<Session, CoreError> {
        self.set_status(id, SessionStatus::Done)
    }

    /// 标记失败（回合内部错误）。
    pub fn fail_session(&self, id: Uuid) -> Result<Session, CoreError> {
        self.set_status(id, SessionStatus::Failed)
    }

    fn set_status(&self, id: Uuid, status: SessionStatus) -> Result<Session, CoreError> {
        let mut guard = self.sessions.lock().expect("会话表锁不得毒化");
        let Some(hot) = guard.get_mut(&id) else {
            return Err(CoreError::SessionNotFound(id));
        };
        hot.session.status = status;
        hot.session.updated_at_ms = now_ms();
        let session = hot.session.clone();
        drop(guard);
        self.emit(&session, "session_update", json!({ "status": status }))?;
        Ok(session)
    }

    // ---------- 回合 ----------
}
