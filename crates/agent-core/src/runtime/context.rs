//! 回合上下文组装（provider-context 卡）：Composer × 记忆 × 历史 × 本轮输入。
//!
//! Runtime 发起回合时不再只把用户输入送给 Provider，而是构造 [`TurnContext`]：
//! - `system_prompt`：`agent_prompts::Composer::build(模式, 变量)`，变量含身份层
//!   （AI 名/用户称呼/人格，agent-memory 身份层不受跨会话记忆开关限制）、工作区、
//!   确定性日期文本、以及**已启用**时的**记忆四段**（身份/用户记忆/项目记忆/当日日志，
//!   h3-memory-inject-4seg 卡；空段整段省略）；
//! - `history`：会话已有消息（时间序），本轮输入在其后单独作为 `user_text`；
//!   进上下文先过**压缩路径**（[crate::runtime::compaction]：历史只从最近一次
//!   completed 压缩开始读，老段由摘要承载），**压缩不可用时**降级到兜底裁剪
//!   （先逐条收窄 `HISTORY_MESSAGE_CHARS`，再按总预算裁剪 `fit_history`：超
//!   `HISTORY_TOTAL_CHARS` 从最旧整组丢弃，工具调用与结果同组同进退，最新一条
//!   永不丢；对齐 opencode 上下文重建/裁剪的确定性口径）；
//! - `memory_digest`：`memory.enabled` 关闭时为 `None`，Composer 不留空段。
//! - **技能注入段**（h3-skill-inject-guard）：会话已注入的技能正文段
//!   （`<skill-content>` + 数据边界声明）**追加在宿主模板之后**，与宿主模板
//!   **分区**（技能正文是数据、不是指令）。段内容取自 durable 事件
//!   （`skill_injected`），因此进程重启后注入段仍在。
//!
//! 隐私（AGENTS §6）：进入上下文的任何文本先过脱敏器；凭据不进模型输入。
//!
//! 记忆新鲜度：开启记忆的每个回合都重新组装，不信任建会话时的 `session`
//! 快照（见 [`Runtime::turn_memory`]），否则本会话内的显式记忆写入永不生效。

use std::ops::Range;

use super::*;
use crate::schema::Role;
use agent_prompts::{Composer, PromptMode, PromptVars};
use serde_json::json;

use super::compaction;
use super::sessions::memory_vars::TurnMemory;
use super::sessions::ExpertDirs;

// 产品 / 身份文件类变量取值（x-prompt-vars-wire 卡）：文件与 `memory_vars.rs`
// 同目录、同风格。本仓的会话子模块声明点是 `runtime/sessions.rs`（不是卡面写的
// `sessions/mod.rs`），而本卡硬边界不含该文件，故在 `context.rs` 用 `#[path]`
// 挂载同一文件，避免越界改动共享模块声明点。
#[path = "sessions/product_vars.rs"]
mod product_vars;

/// 历史消息进上下文前的字符上限（防单条大输出顶爆模型输入；大结果走 result_ref）。
const HISTORY_MESSAGE_CHARS: usize = 2000;

/// 历史总预算（字符口径）：逐条收窄后仍超 ⇒ 从最旧整组丢弃（见 [`fit_history`]）。
///
/// **兜底口径**：压缩路径不可用时才用（见 [`fit_history`] 的说明）。
const HISTORY_TOTAL_CHARS: usize = 24_000;

impl Runtime {
    /// 构造本轮上下文。`history` 是本轮输入推入之前的历史快照。
    pub(crate) fn build_turn_context(
        &self,
        session: &Session,
        history: &[Message],
        user_text: &str,
    ) -> TurnContext {
        let memory = self.turn_memory(session);
        let vars = self.prompt_vars(session, &memory);
        let system_prompt = Composer::build(prompt_mode(session.mode), &vars);
        TurnContext {
            session_id: session.id,
            mode: session.mode,
            system_prompt: self.with_skill_segments(session, system_prompt),
            memory_digest: (!memory.digest.is_empty()).then_some(memory.digest),
            history: self.compacted_history(history),
            user_text: self.redact(user_text),
        }
    }

    /// 宿主模板 + 技能注入段（h3-skill-inject-guard）。
    ///
    /// **分区**：技能段追加在宿主模板**之后**，并自带 `<skill-content>` 标签与
    /// 数据边界声明——模型看到的是「一段被明确标注为数据的内容」，而不是宿主
    /// 指令的延续。技能正文里的越权要求因此不构成授权（授权面只由权限门与
    /// 宿主已授权工具集决定）。
    ///
    /// 无注入段时逐字节返回宿主模板（零回归）。
    fn with_skill_segments(&self, session: &Session, template: String) -> String {
        self.ensure_skills_injected(session);
        let segments = self.skill_segments(session.id);
        if segments.is_empty() {
            return template;
        }
        let mut out = template;
        for segment in segments {
            out.push_str("\n\n");
            out.push_str(&segment);
        }
        out
    }

    /// 技能注入的生产接线（x-skill-inject-wire）：上下文构建时接入**已安装**
    /// 技能（`HostServices.skills` 的可见目录，禁用面由 `list()` 收窄）。
    ///
    /// - **时机**：只在上下文构建时；每个技能每会话**只注入一次**——以 durable
    ///   `skill_injected` 事件判重，后续构建（含同回合后续步）只回放不重注。
    /// - **来源**：`InvocationSource::Model`。自动注入是宿主代模型装配上下文，
    ///   因此 `disable-model-invocation: true` 的技能不会被自动注入（更严口径）。
    /// - **P0**：`inject_skill` → `SkillCatalog::load()` 的审计门直接拒绝加载，
    ///   本方法不绕过；正文不会进上下文。
    /// - **拒绝不静默**：目录扫描越界或单技能被拒（P0/禁用/来源门）落
    ///   `warning` 事件（`kind = skill_injection_rejected`），不阻断回合，
    ///   其它技能照常注入。
    fn ensure_skills_injected(&self, session: &Session) {
        let Some(catalog) = self.host_services().skills.as_ref() else {
            return;
        };
        let summaries = match catalog.list() {
            Ok(summaries) => summaries,
            Err(e) => {
                // 目录级拒绝（如 symlink 越界）整次 fail-closed：显式记录，不静默。
                let _ = self.emit(
                    session,
                    "warning",
                    json!({
                        "kind": "skill_injection_rejected",
                        "reason": e.to_string(),
                    }),
                );
                return;
            }
        };
        let injected = self.injected_skill_names(session.id);
        for summary in summaries {
            if injected.contains(&summary.name) {
                continue;
            }
            if let Err(e) = self.inject_skill(session.id, &summary.name, InvocationSource::Model) {
                let _ = self.emit(
                    session,
                    "warning",
                    json!({
                        "kind": "skill_injection_rejected",
                        "skill": summary.name,
                        "reason": e.to_string(),
                    }),
                );
            }
        }
    }

    /// 历史进上下文的**主路径**：压缩边界 + 摘要。
    ///
    /// 压缩路径（[crate::runtime::compaction]）只做两件事，都不碰网络：
    /// 1. **历史边界**：只从最近一次 completed 压缩开始读（照抄 opencode
    ///    `history.ts:28` 的 `latestCompaction`）——压缩产物是 checkpoint 消息，
    ///    之前的原始消息已被摘要覆盖，不必再进模型；
    /// 2. **兜底裁剪**：边界之后的新消息仍可能超预算 ⇒ 降级到 [`fit_history`]
    ///    （逐条收窄 + 总预算整组丢弃）。
    ///
    /// **checkpoint 逐字保留**（不进收窄、也不被兜底裁剪丢）：摘要是压缩的**全部
    /// 价值所在**，收窄它等于把压缩白做；参考件也是把 checkpoint 原样发给模型
    /// （`to-llm-message.ts:311-325`）。兜底裁剪只作用在 checkpoint **之后**的
    /// 新消息上。
    ///
    /// **为什么压缩产物不重写历史**：本卡只做「决策 + 提示词构造 + 产物落历史」，
    /// 真实摘要调用（provider）尚未接线（见 `docs/compaction-status.md` 的
    /// 「未做」节）。在接线之前，历史里**只有**落进会话的 checkpoint 消息才是
    /// 压缩产物；本方法只负责**读**边界，不负责**造**摘要。
    fn compacted_history(&self, history: &[Message]) -> Vec<Message> {
        let bounded = compaction::history_from_latest_checkpoint(history);
        match bounded.split_first() {
            Some((checkpoint, rest)) if compaction::is_checkpoint(checkpoint) => {
                let mut out = Vec::with_capacity(rest.len() + 1);
                out.push(checkpoint.clone());
                out.extend(fit_history(&narrow_history(rest)));
                out
            }
            // 无 checkpoint ⇒ 纯兜底裁剪（与压缩路径接线前的行为逐条相同）。
            _ => fit_history(&narrow_history(bounded)),
        }
    }

    /// 本轮记忆产物：仅在 `memory.enabled` 时给出（关闭 ⇒ 四段皆空，Composer 不留空段）。
    ///
    /// 新鲜度（memory-freshness 卡）：**每次开启的回合都按现存 MemoryEngine 重组**，
    /// 不采用建会话时留在 `session.memory_digest` 上的快照——否则本会话内通过既有
    /// 记忆写工具（`memory.set_user` / `memory.set_project` / `memory.append_daily`）
    /// 做的显式更新永远进不了模型上下文。会话快照只作为建会话时刻的热态展示用。
    ///
    /// 重组失败 ⇒ 本回合**不带任何记忆**（并落一条 `warning` 事件可告警），回合照常
    /// 跑完；尤其**不回退**到旧会话快照——失败时旧快照可能已过时且含敏感层内容，
    /// 宁可少注入也不注入陈旧记忆（AGENTS §6 最小上下文）。
    ///
    /// 产物含两件：**四段**（注入面，system prompt）与**单段摘要**（展示面，
    /// `TurnContext.memory_digest`）。二者**同源**（一次读盘），不会互相矛盾。
    fn turn_memory(&self, session: &Session) -> TurnMemory {
        let enabled = self
            .config
            .lock()
            .map(|c| c.memory_enabled)
            .unwrap_or_default();
        if !enabled {
            return TurnMemory::default();
        }
        match self.compose_turn_memory(&session.workspace_root) {
            Ok(memory) => memory,
            Err(e) => {
                // 告警本身失败不得改变回合结论（记忆已确定缺席）。
                let _ = self.emit(
                    session,
                    "warning",
                    serde_json::json!({
                        "kind": "memory_compose_failed",
                        "reason": e.to_string(),
                    }),
                );
                TurnMemory::default()
            }
        }
    }

    /// 运行时配置快照：一次加锁取全（锁中毒 ⇒ 默认值）。
    ///
    /// 返回 `(tone_style, user_rules, response_language)` 原文。提示词变量里有三项
    /// 来自运行时配置，一次取全既避免重复加锁，也保证**同一回合内三者一致**。
    fn prompt_settings(&self) -> (String, String, String) {
        self.config
            .lock()
            .map(|c| {
                (
                    c.tone_style.clone(),
                    c.user_rules.clone(),
                    c.response_language.clone(),
                )
            })
            .unwrap_or_default()
    }

    /// 模板变量：**每个 `PromptVars` 字段都显式赋值**（不再用 `..default()` 掩盖）。
    ///
    /// 来源分三类：
    /// - **运行时事实**：身份层（`read_identity`：AI 名 / 用户称呼 / SOUL）、
    ///   身份四文件正文与路径、`product.json`（产品名 / 数据目录名 / `is_windows`）、
    ///   工作区、日期、记忆四段、专家人格（见 `sessions/product_vars.rs`）；
    /// - **运行时配置**（`config:set`）：`tone_style`（`toneStyle`，经
    ///   [`agent_prompts::style::body_of`] 归一化为正文）、`user_rules`（`userRules`，
    ///   渲染「## 用户自定义规则」段）、`response_language`（`responseLanguage`，
    ///   解析为**整句语言指令**，见 [`resolve_response_language`]）；
    ///   三者空值 ⇒ 对应段整段省略；
    /// - **由文件事实推导**：`identity_mode`（`BOOTSTRAP.md` 存在 ⇒ `onboarding`，
    ///   否则 `normal`；推导在 `sessions/product_vars.rs`，与参考端
    ///   `IdentityCollector` 同形）；
    /// - **配置面尚未提供**：`sub_agent_prompt`、功能开关、工具呈现、产物目录 ——
    ///   显式填空串 / `false`（空值 ⇒ 模板既有语义整段省略）；
    /// - **有意留空**：`expert_persona` 专用槽（人格只填 `plugin_agent_prompt` 一处，
    ///   两处同填会渲染两次，见 [`Self::expert_persona`] 文档）。
    ///
    /// 脱敏（AGENTS §6）：从文件/配置读来的**正文**一律过 [`Self::redact`]；
    /// **路径**类变量不脱敏（路径不是凭据）。
    fn prompt_vars(&self, session: &Session, memory: &TurnMemory) -> PromptVars {
        let identity = self.read_identity(&session.workspace_root);
        let product = self.product_vars(&session.workspace_root);
        let (tone_style, user_rules, response_language) = self.prompt_settings();
        PromptVars {
            product_name: self.redact(&product.product_name),
            data_folder_name: self.redact(&product.data_folder_name),
            ai_name: self.redact(&identity.ai_name),
            user_name: self.redact(&identity.user_name),
            soul: self.redact(&identity.soul),
            identity: self.redact(&product.identity),
            user: self.redact(&product.user),
            bootstrap: self.redact(&product.bootstrap),
            identity_mode: product.identity_mode.clone(),
            soul_path: product.soul_path,
            identity_path: product.identity_path,
            user_path: product.user_path,
            bootstrap_path: product.bootstrap_path,
            workspace: self.redact(&session.workspace_root),
            date: date_text(),
            user_rules: self.redact(&user_rules),
            tone_style: agent_prompts::style::body_of(&tone_style),
            plugin_agent_prompt: self.expert_persona(session),
            sub_agent_prompt: String::new(),
            response_language: resolve_response_language(&response_language),
            is_windows: product.is_windows,
            feature_browser_use: false,
            feature_computer_use: false,
            feature_expert_management: false,
            tool_result_presentation: String::new(),
            artifact_dir: String::new(),
            memory_identity: memory.segments.identity.clone(),
            memory_user: memory.segments.user.clone(),
            memory_project: memory.segments.project.clone(),
            memory_daily: memory.segments.daily.clone(),
            expert_persona: String::new(),
        }
    }

    /// 专家人格正文（§10.2 步骤 4：按 `expert_id` 从注册表取人格，**只换注入段、
    /// 不换上下文**）。
    ///
    /// 降级口径与召唤面 `Runtime::bind_expert` **同一实现**（后者含脱敏）：
    /// 取不到（包在召唤后损坏/被删）⇒ 空串 ⇒ 模板整段省略，**不动会话状态**——
    /// `expert_id` 是召唤时已校验过的状态，组装期不回改。
    ///
    /// **只填一处**：卡面指定的人格注入点是既有 `plugin_agent_prompt`（expert 模板条件段
    /// `## 专家人格`，空槽不留标题）；`expert_persona`（专用槽）保持空，**不改**
    /// `agent-prompts` 一行。若两处同填，模板会把同一段人格渲染两次，来源不可辨。
    /// `docs/expert-prompts-status.md` §1 建议「只填专用槽」——二者张力已记入本卡状态
    /// 文档「偏离清单」，切换注入点只需改这一行。
    fn expert_persona(&self, session: &Session) -> String {
        let Some(expert_id) = session.expert_id.as_deref() else {
            return String::new();
        };
        let dirs = ExpertDirs::for_session(&session.workspace_root);
        match self.bind_expert(&dirs, expert_id) {
            Ok(target) => target.persona,
            Err(e) => {
                // 降级：不注入人格段（不阻断回合）；会话状态不回改。只留一条 warning
                // （与 `create_session` 记忆组装失败同口径），不阻断、不改会话。
                let _ = self.emit(
                    session,
                    "warning",
                    json!({
                        "kind": "expert_persona_unavailable",
                        "expertId": expert_id,
                        "code": e.code(),
                        "reason": e.to_string(),
                    }),
                );
                String::new()
            }
        }
    }

    /// 身份层读取（失败回退默认值：身份属产品人格，不阻断回合）。
    fn read_identity(&self, workspace_root: &str) -> agent_memory::Identity {
        use agent_memory::{MemoryEngine, MemoryPaths};
        let paths = MemoryPaths::new(self.workdir.join(".lengsuo"), workspace_root);
        MemoryEngine::new(paths, false)
            .read_identity()
            .unwrap_or_default()
    }

    /// 把压缩产物落进会话历史（照抄 opencode 的 checkpoint 落库语义）。
    ///
    /// 产物形态由 [`compaction::Compaction::apply`] 给出（`<conversation-checkpoint>`
    /// 包裹段），本方法只负责**落库**：写一条消息 + 落一条 durable 事件。
    ///
    /// 事件 kind 用既有的 `"user"`：事件面是**冻结**的（`contracts-check` 的
    /// `EVENT_KINDS` 与壳侧解码器逐项对齐，新增 kind 会被壳侧判为
    /// `UnknownKind`），而参考件渲染给模型的 completed 压缩**本来就是一条
    /// user 消息**（`to-llm-message.ts:311-325`）——故复用 `"user"` 既是参考件
    /// 语义，也是本仓事件面的唯一合法承载。
    ///
    /// 本卡**不接线**真实 Provider：调用方自己拿到摘要文本后经本方法落库
    /// （见 `docs/compaction-status.md` 的「未做」节）。
    pub fn apply_compaction(&self, id: Uuid, content: &str) -> Result<Session, CoreError> {
        let redacted = self.redact(content);
        // 一次时钟读数同时用于消息与事件 payload（与 `send_text` 同一口径）。
        let ts = self.turn_ts();
        let session = self.push_message(id, Role::User, redacted.clone(), None, ts)?;
        self.emit(&session, "user", json!({ "text": redacted, "tsMs": ts }))?;
        Ok(session)
    }
}

/// 会话模式 → 提示词模式（两侧枚举一一对应，不引入跨 crate 依赖方向变化）。
fn prompt_mode(mode: Mode) -> PromptMode {
    match mode {
        Mode::Quick => PromptMode::Quick,
        Mode::Ask => PromptMode::Ask,
        Mode::Plan => PromptMode::Plan,
        Mode::Craft => PromptMode::Craft,
        Mode::Expert => PromptMode::Expert,
    }
}

/// 历史进上下文：逐条收窄并脱敏，保持时间序与角色不变。
fn narrow_history(messages: &[Message]) -> Vec<Message> {
    let mut history = messages.to_vec();
    for message in &mut history {
        message.content = narrow(&redact_static(&message.content), HISTORY_MESSAGE_CHARS);
    }
    history
}

/// 第二道闸：总预算裁剪（对齐 opencode `compaction.ts` 的确定性丢弃核心）。
///
/// 输入应为已过第一道闸 [`narrow_history`] 的历史。超 [`HISTORY_TOTAL_CHARS`] ⇒
/// **从最旧整组丢弃**，直到保留后缀不超预算：
/// - 组 = 一段不可拆分的完整对话：连续 `ToolCall`/`ToolResult` 消息为一组
///   （调用与其结果同进退，绝不拆散），其余消息各自成组；
/// - 最新一组永不丢（对齐参考件 `recentStart` 的 `min(oldestToDrop(…), len-1)` 帽）：
///   即使它单独超预算也只保留第一道闸的逐条收窄形态；
/// - 允许裁到空（仅空输入），不伪造占位消息、不新增 Role。
///
/// 纯函数：不读时钟/环境变量、不依赖 Runtime 状态，同输入同输出。
fn fit_history(messages: &[Message]) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }
    let groups = tool_groups(messages);
    let sizes: Vec<usize> = groups.iter().map(|g| group_chars(messages, g)).collect();
    let dropped = oldest_to_drop(&sizes, HISTORY_TOTAL_CHARS).min(groups.len() - 1);
    messages[groups[dropped].start..].to_vec()
}

/// 历史分组：连续 `ToolCall`/`ToolResult` 消息为一个不可拆分组，其余每条自成一组。
///
/// 参考件以「user 边界」保证调用与结果同进退（`recentStart` 的 userBoundary /
/// `flattenAndDropOldest` 的 exchange）；本仓 ToolCall/ToolResult 是顶层消息，
/// 对应的不可拆分单位即相邻工具消息段（判定依据见卡面）。
fn tool_groups(messages: &[Message]) -> Vec<Range<usize>> {
    let mut groups = Vec::new();
    let mut start = 0;
    for i in 1..=messages.len() {
        let splits = i == messages.len()
            || !(is_tool_message(&messages[i - 1]) && is_tool_message(&messages[i]));
        if splits {
            groups.push(start..i);
            start = i;
        }
    }
    groups
}

fn is_tool_message(message: &Message) -> bool {
    matches!(message.role, Role::ToolCall | Role::ToolResult)
}

/// 组的字符量（口径与逐条收窄一致：`content` 字符数求和）。
fn group_chars(messages: &[Message], group: &Range<usize>) -> usize {
    messages[group.start..group.end]
        .iter()
        .map(|m| m.content.chars().count())
        .sum()
}

/// 逐字对齐 opencode `compaction.ts` 的 `oldestToDrop`：从最新端向旧端累加尺寸，
/// 返回需从最旧端丢弃的组数，使保留后缀的总字符 ≤ 预算（恰好等于预算仍保留，
/// 严格大于才停）。组内顺序、组间时间序均不变。
fn oldest_to_drop(sizes: &[usize], budget: usize) -> usize {
    let mut total = 0;
    let mut start = sizes.len();
    while start > 0 {
        let next = total + sizes[start - 1];
        if next > budget {
            break;
        }
        total = next;
        start -= 1;
    }
    start
}

/// 无 Runtime 句柄处的脱敏（历史收窄用同一 v0 规则集）。
fn redact_static(text: &str) -> String {
    Redactor::default_v0().redact_str(text)
}

/// 确定性日期文本（本地日期；不引入时刻，保证同一天内同输入同输出）。
fn date_text() -> String {
    chrono::Local::now().date_naive().to_string()
}

/// 回复语言的中文分支文案（与参考端 `resolveResponseLanguage` 逐字一致）。
const RESPONSE_LANGUAGE_ZH: &str = "当前处于中文环境，使用简体中文回答 (Speak in Chinese).";

/// 回复语言的英文分支文案（与参考端逐字一致，三行）。
const RESPONSE_LANGUAGE_EN: &str = "Your output language MUST be English by default.
If the user's message (<user_query>) is written in Chinese, respond in Chinese instead.
IMPORTANT: Base your language decision solely on the natural language of the user's message, not on technical content like code, paths, or logs.";

/// 把配置里的语言标识解析为**整句指令**。
///
/// 参考端的 `ResponseLanguage` 变量值是一整句话（不是语言名），模板侧用
/// `'中文' in ResponseLanguage` 判分支 —— 与 `agent-prompts` 的
/// `contains("response_language", "中文")` 条件逐条对应，故此处必须产出整句。
///
/// 判定同参考端 `resolveResponseLocaleIsZh`：
/// 1. 显式值以 `zh` 开头（大小写不敏感）⇒ 中文；
/// 2. 显式值非空且非 `zh*` ⇒ 英文；
/// 3. **未声明 ⇒ 中文**——参考端此时查系统 locale 并叠加 `isOversea`；
///    本仓不读系统 locale（需新增平台依赖，违反「不加依赖」约束），按产品默认
///    地域（非海外、中文优先）取中文分支。该差异已记入 `docs/prompt-config-wire-status.md`。
fn resolve_response_language(configured: &str) -> String {
    let value = configured.trim();
    let is_zh = value.is_empty() || value.to_ascii_lowercase().starts_with("zh");
    if is_zh {
        RESPONSE_LANGUAGE_ZH.to_string()
    } else {
        RESPONSE_LANGUAGE_EN.to_string()
    }
}

#[cfg(test)]
mod prompt_language_tests {
    use super::{resolve_response_language, RESPONSE_LANGUAGE_EN, RESPONSE_LANGUAGE_ZH};

    /// 未声明 / `zh*`（大小写不敏感）⇒ 中文整句。
    #[test]
    fn zh_by_default_and_by_prefix() {
        for value in ["", "   ", "zh", "zh-CN", "ZH-cn"] {
            assert_eq!(
                resolve_response_language(value),
                RESPONSE_LANGUAGE_ZH,
                "value={value:?} 必须解析为中文整句"
            );
        }
    }

    /// 非空且非 `zh*` ⇒ 英文整句（三行原文）。
    #[test]
    fn non_zh_falls_back_to_english() {
        for value in ["en-US", "en", "ja-JP"] {
            assert_eq!(
                resolve_response_language(value),
                RESPONSE_LANGUAGE_EN,
                "value={value:?} 必须解析为英文整句"
            );
        }
        assert!(RESPONSE_LANGUAGE_EN.contains("Your output language MUST be English by default."));
        assert!(RESPONSE_LANGUAGE_ZH.contains("Speak in Chinese"));
    }
}
