use super::tasks::TASK_EVENT_KIND_PREFIX;
use super::*;
use crate::event_log::DEFAULT_PAGE_LIMIT;
use crate::schema::{Message, Role, ToolInfo};
use agent_tasks::TaskEvent;

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

    /// 组装：事件日志与 blob 目录建在 workdir 下；启动回放恢复 seq、会话索引
    /// 与任务看板（未注入看板时由 `task.` 前缀事件重建）。
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
        // 启动回放：恢复既有会话的消息历史（读侧投影，见 [`replay_messages`]）、
        // seq 锚点与标题。
        for id in log.sessions() {
            let last_seq = log.last_seq(id).unwrap_or_default();
            let title = replay_title(&log, id);
            let messages = replay_messages(&log, id);
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
                        messages,
                        created_at_ms: now_ms(),
                        updated_at_ms: now_ms(),
                        status: SessionStatus::Idle,
                        last_seq,
                    },
                },
            );
        }
        // 注入优先：调用方经 task_board() 注入自定义看板时不做重建（测试可控）；
        // 未注入则从事件日志重建（W1：任务事件启动回放）。
        let tasks = Arc::new(Mutex::new(match self.task_board {
            Some(board) => board,
            None => replay_task_board(&log),
        }));
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
            tasks,
            executor: self.tool_executor,
        })
    }
}

/// 启动重建：逐会话回放事件日志，过滤 `task.` 前缀事件，反序列化为 `TaskEvent`
/// 后按事件 seq 顺序 apply（`replay` 返回即 seq 升序，不自行排序——事件 seq 就是
/// 顺序，对照 opencode projector 按 durable seq 消费）。
/// 解析失败的事件跳过并打日志计数：一条坏事件不得让整个运行时起不来。
fn replay_task_board(log: &Arc<dyn EventLog>) -> TaskBoard {
    let mut board = TaskBoard::new();
    let mut skipped = 0usize;
    for session_id in log.sessions() {
        let mut after_seq = 0u64;
        loop {
            let Ok(page) = log.replay(session_id, after_seq, DEFAULT_PAGE_LIMIT) else {
                // 单会话回放失败：按该会话无任务事件处理，不阻断启动。
                skipped += 1;
                eprintln!("runtime: 任务事件回放失败，跳过会话 {session_id}");
                break;
            };
            for event in &page {
                if !event.kind.starts_with(TASK_EVENT_KIND_PREFIX) {
                    continue;
                }
                match serde_json::from_value::<TaskEvent>(event.payload.clone()) {
                    Ok(task_event) => {
                        board.apply(&task_event);
                    }
                    Err(err) => {
                        skipped += 1;
                        eprintln!(
                            "runtime: 任务事件解析失败已跳过（session={session_id} seq={} kind={}）：{err}",
                            event.seq, event.kind
                        );
                    }
                }
            }
            if page.len() < DEFAULT_PAGE_LIMIT {
                break;
            }
            after_seq = page.last().map_or(after_seq, |event| event.seq);
        }
    }
    if skipped > 0 {
        eprintln!("runtime: 任务事件启动回放共跳过 {skipped} 条坏事件");
    }
    board
}

/// 启动重建：按事件 seq 顺序把会话重建为消息序列（读侧投影）。
///
/// 语义出处：金标 opencode `packages/core/src/session/history.ts:78-116`
/// （`messageEntries` 按 `seq` 升序读出会话消息行、逐行解码，读侧不重新排序）
/// 与 `packages/core/src/session/projector.ts`（durable 事件是权威，消息是它的
/// 投影）。本仓把这条链路落在启动回放上：**消息不是权威，事件才是**——进程重启
/// 后用 `user` / `text` / `tool_use` / `tool_result` 四类事件按 seq 重放，重建
/// `Session.messages`。运行期仍以内存为准，重建只发生在启动时，且是**只读**操作
/// （不为重建写任何事件，对照 `history.ts` 的纯读路径）。
///
/// 分层与调用顺序逐条对齐 `messageEntries`：
/// - 分页取该会话**全部**事件（`messageEntries` 一次查全，本仓 `replay` 带
///   `limit`，故必须循环翻页取完，见 [`for_each_event`]）；
/// - 未识别的 kind 不投影（`message-updater.ts` 里 `session.created` /
///   `session.execution.*` / `session.instructions.updated` 等直接 `Effect.void`）；
/// - 单条事件字段缺失或类型不对 ⇒ 跳过并计数（对照 `history.ts:165-179`
///   `firstUserMessage` 的 `Effect.orElseSucceed(() => undefined)`：坏行不致命，
///   只是这条不投影），不 panic、不中断启动。
///
/// 旧日志可能没有 `user` 事件 ⇒ 只重建可还原的部分（历史里会缺用户行），不伪造。
fn replay_messages(log: &Arc<dyn EventLog>, id: Uuid) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut skipped = 0usize;
    for_each_event(log, id, |event| match replay_message(event) {
        Ok(Some(message)) => messages.push(message),
        // `None` 是「该 kind 不投影」，不是坏事件：不计入跳过。
        Ok(None) => {}
        Err(reason) => {
            skipped += 1;
            eprintln!(
                "runtime: 消息重建跳过坏事件（session={id} seq={} kind={}）：{reason}",
                event.seq, event.kind
            );
        }
    });
    if skipped > 0 {
        eprintln!("runtime: 会话 {id} 消息重建共跳过 {skipped} 条坏事件");
    }
    messages
}

/// 分页取全量后逐条交给 `visit`（`replay` 返回即 seq 升序，不自行排序——
/// 事件 seq 就是顺序，对照 `history.ts` 的 `orderBy(asc(seq))`）。
///
/// 分页是真实坑：`EventLog::replay(id, after_seq, limit)` 一次最多返回
/// `DEFAULT_PAGE_LIMIT` 条，只取首页会让超过 500 条事件的长会话丢掉后段历史。
/// 与 [`replay_task_board`] 同一翻页口径（游标 = 本页最后一条的 seq）。
fn for_each_event(log: &Arc<dyn EventLog>, id: Uuid, mut visit: impl FnMut(&Event)) {
    let mut after_seq = 0u64;
    loop {
        let Ok(page) = log.replay(id, after_seq, DEFAULT_PAGE_LIMIT) else {
            // 单页回放失败：按已取到的部分重建，不阻断启动。
            eprintln!("runtime: 消息重建回放失败，停止读取会话 {id} 的第 {after_seq} 条之后");
            break;
        };
        if page.is_empty() {
            break;
        }
        for event in &page {
            visit(event);
        }
        if page.len() < DEFAULT_PAGE_LIMIT {
            break;
        }
        after_seq = page.last().map_or(after_seq, |event| event.seq);
    }
}

/// 单条事件的读侧投影：`Ok(Some)` 产出一条消息，`Ok(None)` 表示该 kind 不投影，
/// `Err` 表示 payload 畸形（调用方跳过并计数）。
///
/// 映射表（`content` 与 `ts_ms` 的来源逐条对应生产端的 emit 形状）：
/// | 事件 kind | 角色 | content | tool |
/// | --- | --- | --- | --- |
/// | `user` | `User` | `payload.text` | — |
/// | `text` | `Assistant` | `payload.text` | — |
/// | `tool_use` | `ToolCall` | `summarize_args(payload.args)` | `{name, args_ref: None, ..}` |
/// | `tool_result` | `ToolResult` | `payload.preview` | `{name, result_ref: payload.resultRef, ..}` |
///
/// `content` 一律取事件 payload，不重算：`user` / `text` 生产时已脱敏一次
/// （`emit` 的二次脱敏是幂等空操作），重算就会与运行期历史不一致。`tool_use`
/// 是唯一例外——事件里存的是脱敏后的 `args` 对象，而历史 `content` 是它的
/// `summarize_args` 摘要，重建必须经**同一个函数**才能逐字相同。
///
/// 已知缺口：`tool_use` payload 没有 `args_ref`（blob 引用只进内存历史），
/// 重建时 `ToolInfo.args_ref` 只能留 `None`；补该字段需改事件生产端，另开卡。
/// 事件里 `error` 与 `warning` 不带消息体（`error` 无消息行），故都不投影。
fn replay_message(event: &Event) -> Result<Option<Message>, String> {
    let ts_ms = event.ts_ms;
    let text = |field: &str| -> Result<String, String> {
        event
            .payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("payload.{field} 缺失或不是字符串"))
    };
    let tool_info = |name: String, result_ref: Option<String>| ToolInfo {
        name,
        // 事件 payload 无 args_ref ⇒ 只能留 None（已知缺口，见上）。
        args_ref: None,
        result_ref,
        ms: None,
    };
    Ok(match event.kind.as_str() {
        "user" => Some(Message {
            role: Role::User,
            content: text("text")?,
            tool: None,
            ts_ms,
        }),
        "text" => Some(Message {
            role: Role::Assistant,
            content: text("text")?,
            tool: None,
            ts_ms,
        }),
        "tool_use" => {
            let name = text("name")?;
            let args = event
                .payload
                .get("args")
                .cloned()
                .ok_or_else(|| "payload.args 缺失".to_string())?;
            Some(Message {
                role: Role::ToolCall,
                // 与生产端 `record_tool_call` 同一函数 ⇒ 摘要文本逐字相同。
                content: Redactor::default_v0().summarize_args(&args),
                tool: Some(tool_info(name, None)),
                ts_ms,
            })
        }
        "tool_result" => Some(Message {
            role: Role::ToolResult,
            content: text("preview")?,
            tool: Some(tool_info(
                text("name")?,
                event
                    .payload
                    .get("resultRef")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )),
            ts_ms,
        }),
        // `thinking` / `done` / `warning` / `approval_*` / `task.*` /
        // `session_update` / `files` / `error` 一律不投影。
        _ => None,
    })
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
