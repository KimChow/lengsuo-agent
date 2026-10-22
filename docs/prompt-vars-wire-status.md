# x-prompt-vars-wire · 22 个 prompt 变量接线状态

> 卡面任务：把 `agent-core` 侧 `PromptVars` 的构造从「只填 6 个 + `..default()`」
> 改为**逐个显式赋值**，并从运行时事实取值。本卡**只接线**，不改模板与变量定义，
> 不新增配置面。

## 0. 先纠一个口径：实际是 31 个字段，不是 22 个

卡面 D1 按 `agent-prompts/src/model.rs:96-140` 的 22 个变量估算；接线时该结构已扩展为
**31 个字段**（`crates/agent-prompts/src/model.rs:96-167`，键全集 `PROMPT_VAR_KEYS: [&str; 31]`
在 `model.rs:173`）。本卡按「先读 `model.rs` 把全部字段列全」的要求，**31 个字段逐个处理**，
没有漏项。

## 1. 31 个变量逐个表

`prompt_vars()` 的构造点：`crates/agent-core/src/runtime/context.rs:229-264`。
所有从文件/配置读来的**正文**都过 `self.redact(...)`；**路径不脱敏**（D5）。

| # | 字段 | 类型 | 填什么 | 来源（file:line） | 空时行为 | 是否已填 |
|---|---|---|---|---|---|---|
| 1 | `product_name` | String | 产品名 | `<home>/product.json` → `runtime/sessions/product_vars.rs:108`；构造 `context.rs:233` | 空 ⇒ 「## 产品身份」整段省略 | ✅ |
| 2 | `data_folder_name` | String | 数据目录名 | `product.json` → `product_vars.rs:119`；`context.rs:234` | 空 ⇒ 无消费段（数据面） | ✅ |
| 3 | `ai_name` | String | 助手名 | 既有 `MemoryEngine::read_identity` → `agent-memory/src/identity.rs:37-46`；`context.rs:235` | 缺省名由 agent-memory 兜底 | ✅（既有） |
| 4 | `user_name` | String | 用户称呼 | 同上；`context.rs:236` | 缺省名兜底 | ✅（既有） |
| 5 | `soul` | String | SOUL.md 正文 | 同上（`identity.rs:44`）；`context.rs:237` | 空 ⇒ 人格段省略 | ✅（既有） |
| 6 | `identity` | String | IDENTITY.md 正文 | `<home>/identity/IDENTITY.md` → `product_vars.rs:82`；`context.rs:238` | 空 ⇒ 身份段按既有语义标 missing | ✅ |
| 7 | `user` | String | USER.md 正文 | `<home>/identity/USER.md` → `product_vars.rs:83`；`context.rs:239` | 空 ⇒ 身份段标 missing | ✅ |
| 8 | `bootstrap` | String | BOOTSTRAP.md 正文 | `<home>/identity/BOOTSTRAP.md` → `product_vars.rs:84`；`context.rs:240` | 空 ⇒ 身份段标 missing | ✅ |
| 9 | `identity_mode` | String | 身份模式标识 | **无来源**（D3 定死空串）；`context.rs:241` | 空串 ⇒ 身份分支不渲染 | ⛔ 未填（见 §2） |
| 10 | `soul_path` | String | SOUL.md 路径 | `MemoryPaths::identity_dir()` → `agent-memory/src/paths.rs:29-31`；`product_vars.rs:85`；`context.rs:242` | 空 ⇒ 路径行标 missing | ✅ |
| 11 | `identity_path` | String | IDENTITY.md 路径 | 同上；`product_vars.rs:86`；`context.rs:243` | 同上 | ✅ |
| 12 | `user_path` | String | USER.md 路径 | 同上；`product_vars.rs:87`；`context.rs:244` | 同上 | ✅ |
| 13 | `bootstrap_path` | String | BOOTSTRAP.md 路径 | 同上；`product_vars.rs:88`；`context.rs:245` | 同上 | ✅ |
| 14 | `workspace` | String | 工作区路径 | 既有 `session.workspace_root`；`context.rs:246` | — | ✅（既有） |
| 15 | `date` | String | 当日日期 | 既有 `date_text()`（`context.rs:431`）；`context.rs:247` | — | ✅（既有） |
| 16 | `user_rules` | String | 用户自定义规则 | **无配置面字段**（D4）；`context.rs:248` | 空 ⇒ 段省略 | ⛔ 未填（见 §2） |
| 17 | `tone_style` | String | 语气风格 | **无配置面字段**（D4）；`context.rs:249` | 空 ⇒ 风格段省略 | ⛔ 未填（见 §2） |
| 18 | `plugin_agent_prompt` | String | 专家人格 | 既有 `expert_persona(session)`（`context.rs:279`）；`context.rs:250` | 空 ⇒ 人格段省略 | ✅（既有） |
| 19 | `sub_agent_prompt` | String | 子智能体人格 | **无配置面字段**（D4）；`context.rs:251` | 空 ⇒ 段省略 | ⛔ 未填（见 §2） |
| 20 | `response_language` | String | 响应语言 | **无配置面字段**（D4）；`context.rs:252` | 空 ⇒ 语言段省略 | ⛔ 未填（见 §2） |
| 21 | `is_windows` | bool | 是否 Windows 平台 | `product.json` 的显式 `is_windows`（**非** `cfg!`/环境变量）→ `product_vars.rs:120-124`；`context.rs:253` | `false` ⇒ Windows 段不渲染 | ✅ |
| 22 | `feature_browser_use` | bool | 浏览器能力开关 | **无来源**；`context.rs:254` | `false` | ⛔ 未填（见 §2） |
| 23 | `feature_computer_use` | bool | 桌面操作开关 | **无来源**；`context.rs:255` | `false` | ⛔ 未填（见 §2） |
| 24 | `feature_expert_management` | bool | 专家管理开关 | **无来源**；`context.rs:256` | `false` ⇒ 专家管理段不渲染 | ⛔ 未填（见 §2） |
| 25 | `tool_result_presentation` | String | 工具结果呈现提示 | **无来源**；`context.rs:257` | 空 ⇒ 无消费段 | ⛔ 未填（见 §2） |
| 26 | `artifact_dir` | String | 产物目录 | **无来源**；`context.rs:258` | 空 ⇒ 无消费段 | ⛔ 未填（见 §2） |
| 27 | `memory_identity` | String | 记忆·身份段 | 既有 `TurnMemory`（`sessions/memory_vars.rs`）；`context.rs:259` | 空 ⇒ 段省略 | ✅（既有） |
| 28 | `memory_user` | String | 记忆·用户段 | 同上；`context.rs:260` | 空 ⇒ 段省略 | ✅（既有） |
| 29 | `memory_project` | String | 记忆·项目段 | 同上；`context.rs:261` | 空 ⇒ 段省略 | ✅（既有） |
| 30 | `memory_daily` | String | 记忆·当日段 | 同上；`context.rs:262` | 空 ⇒ 段省略 | ✅（既有） |
| 31 | `expert_persona` | String | 专家人格专用槽 | **有意留空**：人格只填 `plugin_agent_prompt` 一处，两处同填会渲染两次（`context.rs:263` 与 `context.rs:279-302` 文档） | 空 ⇒ 专用槽省略 | ✅（有意） |

「✅（既有）」= 本卡之前已接线，本卡保持原填法不变（卡面硬约束）。

## 2. 未填项清单（缺来源）

| 字段 | 缺什么 | 后续卡需要 |
|---|---|---|
| `identity_mode` | 仓内无「身份模式」运行态字段；D3 定死空串，不猜值 | 身份注入/onboarding 卡提供运行时状态（`"onboarding"` / `"normal"`） |
| `user_rules` | `RuntimeConfig`（`crates/agent-core/src/config.rs:25-43`）无该字段 | 配置面卡：`RuntimeConfig` + `config:set` + 网关载荷 |
| `tone_style` | 无配置字段（`agent-prompts::ToneStyle` 只是数据面枚举） | 配置面卡：风格选择持久化 |
| `response_language` | 无配置字段 | 配置面卡：语言设置 |
| `sub_agent_prompt` | 无配置字段 | 子代理/配置面卡 |
| `feature_browser_use` / `feature_computer_use` / `feature_expert_management` | 无功能开关来源 | 能力面/配置面卡按宿主能力注入 |
| `tool_result_presentation` | 无来源 | 呈现策略卡 |
| `artifact_dir` | 无来源 | 产物目录卡 |

以上字段在 `prompt_vars()` 中**显式**填空串 / `false`（不是 `..default()` 掩盖），
空值由模板既有语义整段省略。

## 3. 身份四文件与 product.json 的实际读取路径

- 身份根：`<workdir>/.lengsuo/identity`，由 `MemoryPaths::identity_dir()` 给出
  （`crates/agent-memory/src/paths.rs:29-31`；`MemoryPaths::new` 在 `paths.rs:21-26`）。
  本卡复用既有口径，**没有另造目录约定**。
  - `SOUL.md`：既有 `read_identity` 读正文（`agent-memory/src/identity.rs:44`）；
    路径由 `product_vars.rs:85` 给出。
  - `IDENTITY.md` / `USER.md` / `BOOTSTRAP.md`：`product_vars.rs:82-88`
    （`read_text` = `std::fs::read_to_string`，任何失败 ⇒ 空串）。
- `product.json`：`<workdir>/.lengsuo/product.json`
  （`product_vars.rs:37`、读取 `product_vars.rs:103-126`）。
  - 仓内**没有**该文件，本卡**不新建**它（配置面属另一张卡），只定义读取口径。
  - 缺文件 / JSON 非法 / 字段缺失或类型不符 ⇒ 空串 / `false`，**不 panic、不报错**。
  - 字段名：`product_name`、`data_folder_name`、`is_windows`（snake_case）。

## 4. `identity_mode` / `is_windows` 的判定依据

- `identity_mode`：**空串**。不造 `"default"` / `"full"` 等枚举值；`agent-prompts`
  只在 `"onboarding"` / `"normal"` 时渲染身份分支（`templates/identity.rs:67-79`），
  空串 ⇒ 两分支都不命中 ⇒ 身份段不渲染（调用方尚未接入身份注入）。
- `is_windows`：取 **`product.json` 的显式布尔字段**（运行时配置，`product_vars.rs:120-124`）。
  - ⛔ 不用 `cfg!(windows)`、不用 `std::env::consts::OS`：研发机是 Linux，
    编译期/环境判定会让 Windows 条件段永远不渲染（卡面 D3）。
  - 仓内**没有**会话/工作区级平台字段。为实现「显式字段」而非拼凑环境，
    本卡把它放进 `product.json` 的读取口径；缺文件/缺字段 ⇒ `false`。
  - 若主理人要求严格执行「无会话/工作区字段 ⇒ 只留 `false`」，删掉
    `product_vars.rs:120-124` 并改 `is_windows: false` 即可（一行改动）。

## 5. 测试清单

`crates/agent-core/tests/prompt_vars_wire.rs`（真实 Runtime 回合链，7 条，全绿）：

1. `prompt_vars_explicitly_assigns_every_key` —— 源码扫描：遍历
   `PROMPT_VAR_KEYS` 逐个断言在 `prompt_vars()` 中显式赋值，且无
   `..PromptVars::default()`；**不锁字段总数**。
2. `product_name_renders_product_identity_section` —— 有值 ⇒「## 产品身份」。
3. `empty_product_name_omits_product_identity_section` —— 空值 ⇒ 段省略。
4. `is_windows_config_renders_windows_section` —— `true` ⇒「## Windows 命令安全」；
   `false` ⇒ 不渲染。
5. `credential_style_text_is_redacted_in_product_vars` —— 凭据明文不出现，
   `token=***` 占位存在。
6. `missing_product_json_degrades_to_empty_without_panic` —— 缺文件不阻断回合。
7. `identity_files_live_in_memory_identity_dir` —— 四文件落在既有
   `MemoryPaths::identity_dir()`，且未声明 `identity_mode` 时身份段不渲染。

## 6. 门禁与未做项

### 门禁

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ |
| `cargo clippy -p agent-core --all-targets -- -D warnings` | ✅ |
| `cargo test -p agent-core --test prompt_vars_wire` | ✅ 7 passed |
| `python3 scripts/check_no_source_leak.py` | ✅ |
| `python3 scripts/check_line_limit.py` | ✅ hard failures 0；`context.rs` 433 > 400 软目标（WARN，见下） |

`context.rs` 从 400 行增至 433 行：新增的是 31 个字段的**显式赋值**（卡面按 22 字段
估算）+ 新模块挂载注释。取值逻辑已全部放进 `product_vars.rs`（135 行）；硬上限
700 行未超。软目标的超出属「字段数比卡面估算多 9 个」的机械后果，不靠删注释/拆
既有函数来凑数。

`check_docs_not_tracked.py` 的失败是**既有状态**（索引里已有
`docs/memory-route-status.md`、历史里有 2 个 docs 对象），与本卡改动无关；
本卡只新增 `docs/prompt-vars-wire-status.md`，按仓内惯例 `git add -f` 入库。

### 落点偏差（必须记录）

卡面落点写的是 `crates/agent-core/src/runtime/sessions/mod.rs`（只加
`pub mod product_vars;`），但本仓该目录**没有 `mod.rs`**——会话子模块声明点是
`crates/agent-core/src/runtime/sessions.rs:16-21`，而该文件不在本卡硬边界内。
若新建 `sessions/mod.rs`，会与既有 `sessions.rs` 冲突（Rust E0761：同一模块
两处定义），无法编译。

⇒ 为同时满足「文件放 `sessions/product_vars.rs`」与硬边界，在
`crates/agent-core/src/runtime/context.rs:40-41` 用 `#[path = "sessions/product_vars.rs"]`
挂载该模块。若主理人允许改 `sessions.rs`，把这两行换成 `sessions.rs` 里的
`pub(crate) mod product_vars;` 即可，文件内容零改动。

### 未做项（交给后续卡）

- **配置面**：`RuntimeConfig` 增 `userRules` / `toneStyle` / `responseLanguage` /
  `subAgentPrompt` / 功能开关 / `toolResultPresentation` / `artifactDir`，并接
  `config:set` 与网关载荷；本卡按 D4 只留空串。
- **身份模式运行态**：`identity_mode` 的 `"onboarding"` / `"normal"` 判定与切换。
- **product.json 写侧**：本卡只读；文件创建/下发/校验属配置面卡。
- **is_windows 的平台来源**：若配置面后续提供会话/工作区级平台字段，应优先于
  product.json 读取。
