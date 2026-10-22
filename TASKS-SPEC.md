# 会话内任务看板规格 v0（agent-tasks crate）

> 来源：`crates/agent-tasks/README.md` 的冻结点文案 + `src/{task,board,event,error}.rs` 实现 +
> `tests/*` 断言三方对齐成文；行为对照 参考客户端 的 task 工具族。
> 本文件是 `agent-tasks` crate 的唯一规格源；与实现冲突时以本文件为准并回改实现。

## 1. 数据模型

```text
Task {                        // src/task.rs:56-66
  id: Uuid,                   // uuid v4
  session_id: Uuid,
  subject: String,            // 非空白；原样存储，不裁剪
  description: String,        // 允许为空
  status: TaskStatus,
  created_at_ms: u64,         // Unix 毫秒
  updated_at_ms: u64,         // 只前进不回拨
}
TaskStatus: pending | in_progress | done | cancelled                        // src/task.rs:9-19
TaskError : NotFound | InvalidTransition{from,to} | EmptySubject | Json(String)  // src/error.rs:7-19
TaskEvent  : Created{task_id, occurred_at_ms, snapshot}
          | Updated{task_id, occurred_at_ms, subject?, status?}              // src/event.rs:15-33
```

- `TaskStatus` 序列化为小写词（`pending` / `in_progress` / `done` / `cancelled`）；`ALL` 是 4 元素定长数组，`as_str()` 返回同一组词（`src/task.rs:23-37`）。
- `is_terminal()` 仅 `Done | Cancelled` 为真（`src/task.rs:39-42`）。
- `can_transition(to)` 仅 3 对为真（`src/task.rs:45-52`），其余（含终态出边）一律 false。
- `TaskEvent` 用 `type` 作内部标签（`task.created` / `task.updated`）；`Updated` 的 `subject` / `status` 为 `None` 时不参与序列化（`src/event.rs:14,25,28`）。
- `TaskError` 的 `Display` 文案冻结：`任务不存在` / `非法状态迁移: {from:?} → {to:?}` / `subject 不能为空` / `JSON 处理失败: {0}`（`src/error.rs:8-18`；`tests/errors.rs:6-22`）。
- `TaskBoard` 是纯内存表：`tasks: HashMap<Uuid, Task>` + 会话索引 `by_session: HashMap<Uuid, Vec<Uuid>>` + 时钟 `now: NowMs`（`src/board.rs:19-24`）；`NowMs = fn() -> u64`（`src/board.rs:14`）。

## 2. 语义（冻结）

### 2.1 方法签名

```text
TaskBoard::new() -> TaskBoard                                   // src/board.rs:34   系统时钟
TaskBoard::with_clock(now: NowMs) -> TaskBoard                  // src/board.rs:39   注入时钟
TaskBoard::default() -> TaskBoard                               // src/board.rs:26   等价 new()

create(session_id: Uuid, subject: &str, description: &str) -> Result<Task, TaskError>
get(id: Uuid) -> Result<&Task, TaskError>
get_for_session(session_id: Uuid, id: Uuid) -> Result<&Task, TaskError>
to_json(id: Uuid) -> Result<serde_json::Value, TaskError>
update_status(id: Uuid, to: TaskStatus) -> Result<Task, TaskError>
update_status_for_session(session_id: Uuid, id: Uuid, to: TaskStatus) -> Result<Task, TaskError>
update_text(id: Uuid, subject: Option<&str>, description: Option<&str>) -> Result<Task, TaskError>
apply(event: &TaskEvent) -> bool
replay(events: &[TaskEvent]) -> TaskBoard
list_by_session(session_id: Uuid) -> Vec<&Task>
```

### 2.2 逐方法语义

1. `create`：`subject` 去空白后为空 ⇒ `EmptySubject`；校验先于写入（失败不留痕迹、不部分更新）；初始 `status = pending`，`created_at_ms == updated_at_ms == now`，`id` 为 uuid v4；写入任务表并登记会话索引（`src/board.rs:48-70`；`tests/board.rs:12-38`）。
2. `get` / `to_json`：未知 id 一律 `NotFound`；`to_json` 序列化失败 ⇒ `Json(String)`（`src/board.rs:72-74,258-261`；`tests/board.rs:100-114`）。
3. `get_for_session`：任务不存在或 `task.session_id != session_id` ⇒ 同一个 `NotFound`（不泄露他人任务的存在性与内容）（`src/board.rs:80-85`；`tests/owned_access.rs:38-55`）。
4. `update_status`：未知 id ⇒ `NotFound`；同状态**幂等**，返回当前任务且不刷新 `updated_at_ms`；非法迁移 ⇒ `InvalidTransition{from,to}` 且状态不变；合法迁移 ⇒ 写状态并刷新 `updated_at_ms`（`src/board.rs:104-118`；`tests/state_machine.rs:39-72,113-142`）。
5. `update_status_for_session`：owner 不匹配或任务不存在 ⇒ `NotFound` 且不修改状态与时间戳；owner 匹配时完全复用 `update_status` 的状态机与幂等语义（`src/board.rs:90-100`；`tests/owned_access.rs:75-164`）。
6. `update_text`：给出的 `subject` 为空白 ⇒ `EmptySubject`；`None` 表示该字段不变；只写实际变化的字段；无变化不刷新 `updated_at_ms`；不改 `status` 与 `created_at_ms`。校验顺序为「subject 空白检查 → 存在性检查」（`src/board.rs:121-150`；`tests/board.rs:43-95`）。
7. `apply`：返回 `true` 表示事件被接受；`false` 表示被幂等丢弃且看板零变化（`src/board.rs:161-173`）。
   - `task.created`：`task_id != snapshot.id` ⇒ 拒绝；id 已存在 ⇒ 保留首个快照不覆盖；否则插入快照并登记会话索引（`src/board.rs:178-188`；`tests/task_events.rs:54-74,305-324`）。
   - `task.updated`：id 不存在 ⇒ 丢弃且不凭空插入；`subject` 空白 ⇒ 整事件拒绝；状态迁移非法 ⇒ 整事件拒绝（状态机不被事件路径绕过）；只写给出的字段，同值同状态视为无变化；`updated_at_ms` 取事件时间且只前进（`src/board.rs:192-229`；`tests/task_events.rs:78-147`）。
8. `replay`：按 `(occurred_at_ms, task_id, 变体序)` 排序后逐条 `apply`；同任务同刻 `created` 先于 `updated`；结果与输入顺序无关，且与逐条 `apply` 等价（`src/board.rs:233-241`；`src/event.rs:54-60`；`tests/task_events.rs:151-248`）。
9. `list_by_session`：未知会话 ⇒ 空列表；按 `created_at_ms` 升序，同毫秒再按 `id` 升序；同毫秒时多次调用顺序完全一致；会话之间互不可见（`src/board.rs:244-255`；`tests/board.rs:119-172`）。
10. 刷新时间戳统一走 `bump`：取 `max(旧值, now)`，即只前进不回拨（注入时钟回退时亦然）（`src/board.rs:265-267`；`tests/monotonic_time.rs:36-198`）。

### 2.3 状态机（唯一合法路径，终态不可出）

```text
pending -> in_progress -> done
                     \-> cancelled
```

- 4×4 迁移矩阵：合法 3 组、同状态幂等 4 组、非法 9 组（`src/task.rs:45-52`；`tests/state_machine.rs:39-72`）。
- `pending` 不能直达 `done` / `cancelled`（`tests/state_machine.rs:102-110`）。
- `done` / `cancelled` 为终态，不可迁出（`src/task.rs:39-42`；`tests/state_machine.rs:75-99`）。

### 2.4 JSON 契约

- `to_json` 顶层字段名固定 7 个：`id` / `session_id` / `subject` / `description` / `status` / `created_at_ms` / `updated_at_ms`（`tests/json_contract.rs:11-32`；`tests/task_events.rs:262-275`）。
- `id` 与 `session_id` 为字符串形式 UUID；时间戳为非负整数毫秒；`status` 为小写词（`in_progress` 带下划线）；`Task` 自身 serde 往返的顶层字段集与 `to_json` 一致（`tests/json_contract.rs:33-44,92-103`）。
- `TaskStatus` 的 serde 只接受四个小写词，`"PENDING"` / `"blocked"` 一律拒绝（`tests/json_contract.rs:79-89`）。
- `TaskEvent` 序列化带 `type` 标签与 `task_id` / `occurred_at_ms`；`Created.snapshot` 的字段集复用 `Task` 的 7 个字段；`Updated` 的 `None` 字段不出现在 JSON 中（`tests/task_events.rs:252-301`）。

## 3. 验收边界

- 状态机 4×4 全矩阵、终态不可出、`pending` 不得跳级：`tests/state_machine.rs`（7 例）。
- subject 空/非空、文本更新的部分与幂等语义、`NotFound` 全入口、list 排序与会话隔离与同毫秒稳定顺序、快照不可变、系统时钟入口：`tests/board.rs`（12 例）。
- 错误变体与 `Display` 文案、迁移两端携带、可比较可克隆：`tests/errors.rs`（3 例）。
- JSON 字段名与 `status` 小写词、`Task` / `TaskStatus` serde 往返：`tests/json_contract.rs`（5 例）。
- 时钟回拨下 `updated_at_ms` 只前进不回拨、幂等请求不刷新时间：`tests/monotonic_time.rs`（7 例）。
- 会话作用域读取门与更新门、同形 `NotFound`、越权零副作用、owner 仍受状态机约束、旧 API 兼容：`tests/owned_access.rs`（12 例）。
- 事件 apply 幂等、未知 id 丢弃、状态机不被绕过、replay 确定性与等价、事件 serde 契约：`tests/task_events.rs`（8 例）。
- 测试基线：7 个集成测试二进制合计 54 例（12 / 3 / 5 / 7 / 12 / 7 / 8），单元测试与文档测试各 0 例；本 crate 不改代码时该数字须逐字不变。

## 4. 硬约束

- 纯域 crate：不依赖 tokio、不做任何 IO、不落盘；持久化与会话存储归属上层（`agent-core` 运行时 / `agent-gateway` 工具接入）（`src/lib.rs:3-4`；`crates/agent-tasks/README.md:4-5`）。
- 全 crate `#![forbid(unsafe_code)]`（`src/lib.rs:7`）。
- 依赖仅 `serde` / `serde_json` / `thiserror` / `uuid`（`crates/agent-tasks/Cargo.toml:9-13`）；`Json(String)` 变体与 `serde_json::Value` 返回值决定必须使用 `serde_json`。
- 公开面仅 `NowMs` / `TaskBoard` / `TaskError` / `TaskEvent` / `Task` / `TaskStatus`（`src/lib.rs:14-17`）。
- 不含权限判断、事件推送、会话存储与模型提示，这些归属其他 crate（`crates/agent-tasks/README.md:56`）。
- 「≥3 步任务才提示使用任务系统」属工具层/提示词层判断，本 crate 只提供任务事实，不内置提示策略（`crates/agent-tasks/README.md:47-50`）。
- 单文件 ≤700 行（400 软目标），`scripts/check_line_limit.py` 对 `.md` 同样生效。

## 5. 已知差异

1. **README 漏列会话作用域与事件面**：README 的冻结点清单未列 `get_for_session` / `update_status_for_session`（`src/board.rs:80,90`）与 `TaskEvent` / `apply` / `replay`（`src/event.rs:15-33`；`src/board.rs:161,233`），但实现与 `tests/owned_access.rs`、`tests/task_events.rs` 均已覆盖 ⇒ 以测试断言为准，本规格 §2 已补齐。
2. **README 未写时间戳只前进**：README 只说明 `with_clock` 注入时钟，未写 `updated_at_ms` 只前进不回拨（`src/board.rs:265-267`；`tests/monotonic_time.rs`）⇒ 本规格 §2.2 第 10 条已补齐。
3. **README 的 JSON 覆盖面不全**：README 只称契约由 `tests/json_contract.rs` 钉住，而事件面的 `type` 标签、`snapshot` 字段集与 `None` 省略由 `tests/task_events.rs` 钉住（`crates/agent-tasks/README.md:39`）⇒ 本规格 §2.4 已把事件契约一并冻结。
4. **「未知 id + 空白 subject」组合无测试断言**：`update_text` 的校验顺序为「subject 空白 → 存在性」，故 `update_text(未知 id, Some(""), None)` 返回 `EmptySubject` 而非 `NotFound`（`src/board.rs:127-132`）；`tests/board.rs:107-111` 只覆盖非空 subject + 未知 id ⇒ 本规格 §2.2 第 6 条按实现记录。
5. **任务卡 D8 的「27 passed」与实际基线不符**：实际 `cargo test -p agent-tasks --locked --offline` 为 54 例（见 §3 末条）⇒ 回归判据取「与基线逐字一致 = 54」，卡面数字记为笔误。
