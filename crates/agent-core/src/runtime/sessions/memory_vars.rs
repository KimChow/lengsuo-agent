//! 记忆四段取值（h3-memory-inject-4seg 卡）：身份 / 用户记忆 / 项目记忆 / 当日日志。
//!
//! 分层、命名与调用顺序照抄规格 `harness-memory.md` §3.2（四段）与 wb 04 §2.3
//! （三层记忆的注入变量）：
//! - **身份**：AI 名 / 用户称呼 / SOUL（`CAP_SOUL` = 8000 全文；名字不截）；
//! - **用户记忆**：`<home>/memory/MEMORY.md` 全文（写路径已限 `CAP_USER_MEMORY` = 4000）；
//! - **项目记忆**：`<workspace>/.lengsuo/memory/MEMORY.md` 全文（`CAP_PROJECT_MEMORY` = 3000）；
//! - **当日日志**：当日文件的**尾部**（compose 取尾部 ≤200 行，再取尾部 2000 字符）——
//!   越新的越重要。
//!
//! **总量 8000 字符**（[`MEMORY_TOTAL_CHARS`]），超限**按层裁剪**：身份层优先保留
//! （`MEMORY-SPEC.md` §4「总注入上限 8000 字符，超限按层裁剪（身份层优先保留）」），
//! 其余按 用户 → 项目 → 当日 的注入顺序依次让位（当日最先被裁）。
//!
//! 截断一律是**字符级**截断（[`fit_head`] / [`fit_tail`]），**无** result_ref 语义
//! （只有工具结果才用 `narrow`）。
//!
//! 空值语义：某段为空 ⇒ 该变量为空串 ⇒ 模板 `slot()` 整段省略（`agent-prompts`
//! `model.rs` 的 `slot()`，本卡不改）。

use agent_memory::MemoryEngine;
use agent_memory::MemoryPaths;
use agent_memory::CAP_PROJECT_MEMORY;
use agent_memory::CAP_SOUL;
use agent_memory::CAP_USER_MEMORY;

use crate::error::CoreError;
use crate::runtime::preview;
use crate::runtime::Runtime;

/// 记忆注入总量上限（字符；v0 单段摘要沿用同一上限）。
pub(crate) const MEMORY_TOTAL_CHARS: usize = 8000;

/// 当日日志段的字符上限（取**尾部**）。
pub(crate) const DAILY_TAIL_CHARS: usize = 2000;

/// 四层的原始内容（未脱敏、未裁剪）。
///
/// 「层缺失」与「层存在但为空」用 `Option` 区分：前者不进摘要、后者进摘要（各层
/// 前 200 字符预览为空）；注入面（段）两者都为空。
#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryLayers {
    /// AI 名（身份层）。
    pub ai_name: String,
    /// 用户称呼（身份层）。
    pub user_name: String,
    /// SOUL 全文（身份层）。
    pub soul: String,
    /// 用户记忆全文。
    pub user: Option<String>,
    /// 项目记忆全文。
    pub project: Option<String>,
    /// 当日日志尾部文本（compose 取的尾部 ≤200 行）。
    pub daily: Option<String>,
}

/// 四段的最终注入文本（已脱敏、已按层裁剪；空串 ⇒ 模板整段省略）。
#[derive(Debug, Clone, Default)]
pub(crate) struct MemorySegments {
    /// 身份段（AI 名 / 用户称呼 / SOUL）。
    pub identity: String,
    /// 用户记忆段。
    pub user: String,
    /// 项目记忆段。
    pub project: String,
    /// 当日日志段。
    pub daily: String,
}

/// 一回合的记忆产物：旧单段摘要（会话快照面）+ 四段（注入面）。
#[derive(Debug, Clone, Default)]
pub(crate) struct TurnMemory {
    /// 旧单段摘要文本（`Session.memory_digest` / `TurnContext.memory_digest`）。
    pub digest: String,
    /// 四段注入文本（`PromptVars` 的四个记忆字段）。
    pub segments: MemorySegments,
}

impl Runtime {
    /// 读四层原始内容（只读聚合，恒读四层）。
    ///
    /// 任一层读失败 ⇒ **fail-closed** 上抛（与 agent-memory `compose` 同口径：
    /// 文件缺失只算「该层无内容」，其余 IO 错误才是失败）。`memory.enabled` 的
    /// 判定在调用方（`Runtime::turn_memory`），不在此处——身份层按
    /// `MEMORY-SPEC.md` §2 不受开关限制，但**注入面**整块受开关控制。
    pub(crate) fn collect_memory_layers(
        &self,
        workspace_root: &str,
    ) -> Result<MemoryLayers, CoreError> {
        let engine = MemoryEngine::new(
            MemoryPaths::new(self.workdir.join(".lengsuo"), workspace_root),
            true,
        );
        let bundle = engine
            .compose()
            .map_err(|e| CoreError::InvalidConfig(format!("记忆组装失败: {e}")))?;
        Ok(MemoryLayers {
            ai_name: bundle.identity.ai_name,
            user_name: bundle.identity.user_name,
            soul: bundle.identity.soul,
            user: bundle.user_memory,
            project: bundle.project_memory,
            daily: bundle.daily_tail,
        })
    }

    /// 一回合的记忆产物（**一次读盘** ⇒ 摘要与四段同源，不会互相矛盾）。
    pub(crate) fn compose_turn_memory(
        &self,
        workspace_root: &str,
    ) -> Result<TurnMemory, CoreError> {
        let layers = self.collect_memory_layers(workspace_root)?;
        Ok(TurnMemory {
            digest: self.compose_digest_text(&layers),
            segments: self.compose_memory_segments(&layers),
        })
    }

    /// 四段取值：脱敏 → 按层裁剪（身份层优先保留）。
    pub(crate) fn compose_memory_segments(&self, layers: &MemoryLayers) -> MemorySegments {
        fit_segments(
            self.redact(&identity_text(layers)),
            self.redact(layers.user.as_deref().unwrap_or_default()),
            self.redact(layers.project.as_deref().unwrap_or_default()),
            self.redact(layers.daily.as_deref().unwrap_or_default()),
        )
    }

    /// 旧单段摘要文本（会话快照 `Session.memory_digest` 与 `TurnContext.memory_digest`）。
    ///
    /// 格式与本卡之前**逐字一致**（`[身份] AI=… 用户=…` / `[人格] …` / `[用户记忆] …` /
    /// `[项目记忆] …` / `[当日日志尾] …`，各层前 200 字符预览，整体 ≤8000）：会话快照是
    /// **热态展示面**（网关 `session:get`），四段拆分只作用于**注入面**。
    pub(crate) fn compose_digest_text(&self, layers: &MemoryLayers) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "[身份] AI={} 用户={}\n",
            layers.ai_name, layers.user_name
        ));
        if !layers.soul.is_empty() {
            out.push_str(&format!("[人格] {}\n", preview(&layers.soul, 200)));
        }
        if let Some(m) = &layers.user {
            out.push_str(&format!("[用户记忆] {}\n", preview(m, 200)));
        }
        if let Some(m) = &layers.project {
            out.push_str(&format!("[项目记忆] {}\n", preview(m, 200)));
        }
        if let Some(m) = &layers.daily {
            out.push_str(&format!("[当日日志尾] {}\n", preview(m, 200)));
        }
        preview(&self.redact(&out), MEMORY_TOTAL_CHARS)
    }
}

/// 身份层文本：AI 名 / 用户称呼 / SOUL（名字不截；SOUL 按 `CAP_SOUL` 取全文）。
///
/// 形态照抄 v0 单段摘要的身份行（`[身份] AI=… 用户=…`）的字段口径（AI 名 +
/// 用户称呼 + SOUL），但改为**逐行**形式：身份段与公共头的「## 身份」同名，
/// 逐行形态让模型能分清哪一行是名字、哪一块是 SOUL。
fn identity_text(layers: &MemoryLayers) -> String {
    format!(
        "AI 名：{}\n用户称呼：{}\nSOUL：\n{}",
        layers.ai_name,
        layers.user_name,
        preview(&layers.soul, CAP_SOUL)
    )
}

/// 按层裁剪：身份层优先保留，其余按 用户 → 项目 → 当日 的注入顺序依次让位。
///
/// 口径（`MEMORY-SPEC.md` §4「总注入上限 8000 字符，超限按层裁剪（身份层优先保留）」）：
/// - 身份段**先占额度**：整段按总量 [`MEMORY_TOTAL_CHARS`] 取（名字不截 ⇒ 极端情形
///   下由 SOUL 尾部让出），**不被**其余三层挤掉；
/// - 其余三段共享 `总量 − 身份段长度` 的额度，按 用户 → 项目 → 当日 顺序分配，
///   额度用尽者被裁短（额度为 0 ⇒ 整段留空 ⇒ 模板省略该段）——即**当日最先被裁**；
/// - 各段自身上限（用户 4000 / 项目 3000 / 当日 2000）先于总量生效。
fn fit_segments(identity: String, user: String, project: String, daily: String) -> MemorySegments {
    let identity = fit_head(&identity, MEMORY_TOTAL_CHARS);
    let mut remaining = MEMORY_TOTAL_CHARS - identity.chars().count();
    let user = fit_head(&user, CAP_USER_MEMORY.min(remaining));
    remaining -= user.chars().count();
    let project = fit_head(&project, CAP_PROJECT_MEMORY.min(remaining));
    remaining -= project.chars().count();
    let daily = fit_tail(&daily, DAILY_TAIL_CHARS.min(remaining));
    MemorySegments {
        identity,
        user,
        project,
        daily,
    }
}

/// 头部截断到 `budget` 字符**以内**（含省略号本身）；`budget = 0` ⇒ 空串。
///
/// 与 `preview` 同口径（超限加「…」，**无** result_ref 语义），但**不超预算**：
/// `preview` 会额外补一个省略号字符，故先让出 1 个字符给标记。
fn fit_head(text: &str, budget: usize) -> String {
    let len = text.chars().count();
    if len <= budget {
        return text.to_string();
    }
    if budget == 0 {
        return String::new();
    }
    preview(text, budget - 1)
}

/// 尾部截断到 `budget` 字符**以内**（超限以「…」标记**头部**被裁）。
///
/// 当日日志用本口径：越新的越重要 ⇒ 取尾部不取头部（与 `preview` 的取头相反）。
fn fit_tail(text: &str, budget: usize) -> String {
    let len = text.chars().count();
    if len <= budget {
        return text.to_string();
    }
    if budget == 0 {
        return String::new();
    }
    let cut: String = text.chars().skip(len - (budget - 1)).collect();
    format!("…{cut}")
}
