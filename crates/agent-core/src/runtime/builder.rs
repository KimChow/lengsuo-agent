use super::*;

/// 运行时构造器。
#[derive(Default)]
pub struct RuntimeBuilder {
    workdir: Option<PathBuf>,
    provider: Option<Arc<dyn Provider>>,
    gate: Option<PermissionGate>,
    config: Option<RuntimeConfig>,
    task_board: Option<TaskBoard>,
    tool_executor: Option<Arc<dyn ToolExecutor + Send + Sync>>,
}

impl RuntimeBuilder {
    #[must_use]
    pub fn workdir(mut self, path: impl AsRef<Path>) -> Self {
        self.workdir = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    #[must_use]
    pub fn gate(mut self, gate: PermissionGate) -> Self {
        self.gate = Some(gate);
        self
    }

    #[must_use]
    pub fn config(mut self, config: RuntimeConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// 注入任务看板（默认系统时钟；测试注入受控时钟保证时间戳确定）。
    #[must_use]
    pub fn task_board(mut self, board: TaskBoard) -> Self {
        self.task_board = Some(board);
        self
    }

    /// 注入工具执行器：工具步按权限门判定真实执行并结算结果；
    /// 不注入（默认 `None`）保持 pending 占位（执行由宿主承担）。
    #[must_use]
    pub fn tool_executor(mut self, executor: Arc<dyn ToolExecutor + Send + Sync>) -> Self {
        self.tool_executor = Some(executor);
        self
    }

    /// 组装：事件日志与 blob 目录建在 workdir 下；启动回放恢复 seq 与会话索引。
    pub fn build(self) -> Result<Runtime, CoreError> {
        let workdir = self
            .workdir
            .unwrap_or_else(|| std::env::temp_dir().join("lengsuo-agent-runtime"));
        std::fs::create_dir_all(&workdir).map_err(|e| CoreError::Io {
            path: workdir.clone(),
            source: e,
        })?;
        let log: Arc<dyn EventLog> = Arc::new(JsonlEventLog::open(workdir.join("events"))?);
        let gate = self
            .gate
            .unwrap_or_else(|| PermissionGate::new(ToolRegistry::new()));
        let provider = self
            .provider
            .ok_or_else(|| CoreError::InvalidConfig("Runtime 必须注入 provider".into()))?;
        let mut sessions: BTreeMap<Uuid, HotSession> = BTreeMap::new();
        // 启动回放：恢复既有会话的 seq 锚点（消息历史 v0 从事件重建为最小集）。
        for id in log.sessions() {
            let last_seq = log.last_seq(id).unwrap_or_default();
            let title = replay_title(&log, id);
            sessions.insert(
                id,
                HotSession {
                    drain: Arc::new(Mutex::new(())),
                    session: Session {
                        memory_digest: None,
                        id,
                        title,
                        workspace_root: workdir.display().to_string(),
                        mode: Mode::Ask,
                        permission_mode: PermissionMode::ReadOnly,
                        env_binding: None,
                        isolation: Isolation::local_default(),
                        messages: Vec::new(),
                        created_at_ms: now_ms(),
                        updated_at_ms: now_ms(),
                        status: SessionStatus::Idle,
                        last_seq,
                    },
                },
            );
        }
        Ok(Runtime {
            workdir: workdir.clone(),
            sessions: Arc::new(Mutex::new(sessions)),
            log,
            blobs: Arc::new(BlobStore::new(workdir.join("blobs"))),
            hub: Arc::new(EventHub::new()),
            gate: Arc::new(gate),
            provider,
            redactor: Redactor::default_v0(),
            config: Arc::new(Mutex::new(
                self.config.unwrap_or_else(RuntimeConfig::default_v0),
            )),
            tasks: Arc::new(Mutex::new(self.task_board.unwrap_or_default())),
            executor: self.tool_executor,
        })
    }
}

/// 从事件回放里取最近一次标题（重启后 `session:list` 仍能显示标题）。
fn replay_title(log: &Arc<dyn EventLog>, id: Uuid) -> String {
    let events = log.replay(id, 0, 500).unwrap_or_default();
    for event in events.iter().rev() {
        if event.kind == "session_update" {
            if let Some(title) = event.payload.get("title").and_then(Value::as_str) {
                return title.to_string();
            }
        }
    }
    format!("会话 {id}")
}
