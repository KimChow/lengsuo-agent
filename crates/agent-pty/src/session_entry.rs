//! 受策略约束的 PTY 会话入口与单会话句柄。
//!
//! 分层与调用顺序对齐参考实现的 PTY 启动流程：
//! 1. 解析配置（`PtyConfig::argv`，shell-words 语义）；
//! 2. **先**过 argv 策略门（[`policy_gate::check_config`]）：Deny/Ask 直接返回，
//!    不创建 PTY、不起子进程；
//! 3. 仅 `Allow` 才走 `session::spawn`（openpty → spawn_command）。
//!
//! 句柄内部复用 [`PtyManager`] 的单会话能力（写/resize/kill/输出/回放），
//! 不复制一份数据面实现。既有的 [`PtyManager::create`](crate::PtyManager::create)
//! 保持原签名与原行为，不经过策略门；[`PtySession::open_guarded`] 是新增的
//! 受约束入口，[`PtySession::open`] 与它共享除策略判定外的全部启动语义。

use agent_execpolicy::argv::Policy;
use tokio::sync::mpsc;

use crate::config::PtyConfig;
use crate::error::PtyError;
use crate::manager::PtyManager;

/// 一个已启动的 PTY 会话句柄（单属主持有）。
pub struct PtySession {
    manager: PtyManager,
    id: u64,
    shell: String,
}

impl std::fmt::Debug for PtySession {
    /// 只展示会话 id 与存活状态；命令行可能含凭据，不进 `Debug` 输出。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtySession")
            .field("id", &self.id)
            .field("alive", &self.alive())
            .finish_non_exhaustive()
    }
}

impl PtySession {
    /// 按配置启动 PTY，**不**经过 argv 策略门（与既有 `PtyManager::create`
    /// 相同的启动语义）。
    ///
    /// 未走 [`PtySession::open_guarded`] 的调用不受策略约束——这是本卡明确
    /// 记录的残余缺口；需要策略约束的执行链必须使用 guarded 入口。
    pub async fn open(cfg: PtyConfig) -> Result<Self, PtyError> {
        let shell = cfg.shell.clone();
        let mut manager = PtyManager::new();
        let id = manager.create(cfg).await?;
        Ok(Self { manager, id, shell })
    }

    /// 按配置启动 PTY，spawn 前先做 argv 级策略判定。
    ///
    /// - `Allow`：创建 PTY 并启动子进程；
    /// - `NeedsApproval`（含无匹配规则的 fail-closed、`bash -c` 内层不可见）：
    ///   返回 [`PtyError::NeedsApproval`]，不创建 PTY、不起子进程；
    /// - `Deny`：返回 [`PtyError::PolicyDenied`]，同样无副作用。
    pub async fn open_guarded(cfg: PtyConfig, policy: &Policy) -> Result<Self, PtyError> {
        let shell = cfg.shell.clone();
        let mut manager = PtyManager::new();
        let id = manager.create_guarded(cfg, policy).await?;
        Ok(Self { manager, id, shell })
    }

    /// 启动的命令行（原样回显）。
    pub fn shell(&self) -> &str {
        &self.shell
    }

    /// 本句柄对应的终端会话 id。
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 子进程 pid；平台未报告时为 `None`。
    pub fn pid(&self) -> Option<u32> {
        self.info().pid
    }

    /// 创建时间（unix 毫秒）。
    pub fn started_at_ms(&self) -> u64 {
        self.info().started_at_ms
    }

    /// 取回输出流；每会话仅一条、只能取走一次。
    pub async fn take_output(&mut self) -> Option<mpsc::Receiver<Vec<u8>>> {
        self.manager.take_output(self.id).await
    }

    /// 向 stdin 写入原始字节。
    pub async fn write(&self, data: &[u8]) -> Result<(), PtyError> {
        self.manager.write(self.id, data).await
    }

    /// 调整窗口尺寸。
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.manager.resize(self.id, cols, rows).await
    }

    /// 终止子进程并尽最大努力回收。
    pub async fn kill(&mut self) -> Result<(), PtyError> {
        self.manager.kill(self.id).await
    }

    /// 历史回放（≤1 MiB）。
    pub fn replay(&self) -> Vec<u8> {
        self.manager.replay(self.id)
    }

    /// 子进程是否仍活动。
    pub fn alive(&self) -> bool {
        self.info().alive
    }

    fn info(&self) -> crate::manager::SessionInfo {
        self.manager
            .list()
            .into_iter()
            .find(|info| info.id == self.id)
            // 句柄持有管理器，条目不可能消失；缺失按已死处理。
            .unwrap_or(crate::manager::SessionInfo {
                id: self.id,
                shell: self.shell.clone(),
                cwd: None,
                pid: None,
                started_at_ms: 0,
                alive: false,
            })
    }
}
