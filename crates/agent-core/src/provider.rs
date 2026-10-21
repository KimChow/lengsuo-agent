//! Provider 抽象（开工说明 §7 卡 A：真实 LLM 接入不做，Provider trait + echo 实现）。
//!
//! agent-core 不依赖 tokio：`generate` 是同步的纯函数式步骤产出，
//! 网关层在自己的运行时里调用。v0 只提供 `EchoProvider`（测试用，回显用户输入），
//! 真实模型接入留待后续（返回 `Unsupported` 占位实现不误导能力）。

use serde_json::Value;

use crate::error::CoreError;
use crate::schema::{Message, Mode};

/// OpenAI 兼容的 token 用量。
///
/// 字段名与 OpenAI Chat Completions 规范的 `usage` 对象一致，
/// 便于网关透传的响应体直接映射，**不自创协议**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// 输入 token（含命中缓存的部分）。
    pub prompt_tokens: u64,
    /// 输出 token。
    pub completion_tokens: u64,
    /// 总 token。
    pub total_tokens: u64,
    /// 命中缓存的输入 token（OpenAI 规范：usage.prompt_tokens_details.cached_tokens）。
    pub cached_tokens: u64,
}

impl TokenUsage {
    /// 计费口径：输入扣掉缓存命中，再加上输出。
    ///
    /// 推理 token 与总数不计入计费；缓存命中的输入 token 不重复计费。
    pub fn billed_tokens(&self) -> u64 {
        self.prompt_tokens.saturating_sub(self.cached_tokens) + self.completion_tokens
    }
}

/// 一次回合产出的一个步骤。
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStep {
    /// 思考过程（事件 `thinking`）。
    Thinking(String),
    /// 回复文本（事件 `text`，同时进 `Message`）。
    Text(String),
    /// 请求调用工具（事件 `tool_use`）；是否执行由权限门决定。
    ToolCall { name: String, args: Value },
    /// 回合结束（事件 `done`）。
    Finished,
}

/// 一次回合送给 Provider 的完整上下文（provider-context 卡）。
///
/// 组成：Composer 按会话模式构建的 system prompt（含已启用记忆摘要、身份变量、
/// 硬规则与门禁片段）+ 会话历史（不含本轮输入）+ 本轮用户输入。
/// 凭据约束：所有字段在进入本结构前已过脱敏器（§6）。
#[derive(Debug, Clone, PartialEq)]
pub struct TurnContext {
    /// 会话 id。
    pub session_id: uuid::Uuid,
    /// 会话模式（决定 Composer 模板）。
    pub mode: Mode,
    /// Composer 构建的系统提示词。
    pub system_prompt: String,
    /// 已启用的三层记忆摘要；`memory.enabled` 关闭时为 `None`。
    pub memory_digest: Option<String>,
    /// 本轮之前的会话历史（时间序，末尾即本轮输入的前一条）。
    pub history: Vec<Message>,
    /// 本轮用户输入（只出现于此，不在 `history` 中重复）。
    pub user_text: String,
}

/// 单次 provider 步的工具选择（金标 `llm.ts` 的 `toolChoice`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolChoice {
    /// 模型自主决定是否用工具（默认，既有行为）。
    #[default]
    Auto,
    /// 禁用工具：maxSteps 末步缴械专用（仍返回文本）。
    None,
}

/// 单次 provider 步的请求选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StepOptions {
    /// 工具选择。
    pub tool_choice: ToolChoice,
}

/// provider 侧的压缩能力（本仓 v1 **只有声明，没有实现**）。
///
/// `native` 压缩的产物是 provider 侧生成的加密上下文，**不可跨 provider 重放**；
/// 本仓尚未具备 provenance 校验与端点绑定，因此这里只区分「没声明」与
/// 「声明了但尚不可用」，两档在 [`crate::runtime::compaction::native_compact`]
/// 里都 fail-closed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactionCapability {
    /// 不支持 provider 侧压缩（默认；所有现有 provider 都应是这一档）。
    #[default]
    Unsupported,
    /// 声明支持，但本仓尚无 provenance 校验与端点绑定 ⇒ **调用一律拒绝**。
    Declared,
}

/// 模型/回合提供方。
pub trait Provider: Send + Sync {
    /// 针对会话最新一条用户输入产出步骤序列（echo provider 返回固定两步）。
    fn next_steps(
        &self,
        session_id: uuid::Uuid,
        user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError>;

    /// 针对完整回合上下文产出步骤序列。
    ///
    /// 默认实现降级到旧 `next_steps`（只送本轮输入），保证既有实现方
    /// （`EchoProvider`、网关测试里的 `Slow` 等）无需改动即可继续跑回合；
    /// 需要真实 system prompt / 历史 / 记忆的新适配器覆写本方法。
    fn next_steps_with_context(
        &self,
        context: &TurnContext,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        self.next_steps(context.session_id, &context.user_text)
    }

    /// 针对完整回合上下文 + 请求选项产出步骤序列（末步缴械以
    /// `tool_choice: None` 调用，见 `loop` 的 `MAX_STEPS_PROMPT`）。
    ///
    /// 默认实现忽略选项并降级到 [`Provider::next_steps_with_context`]，
    /// 既有实现方无需改动；真实适配器应覆写本方法把 `toolChoice`
    /// 透传给上游模型。
    fn next_steps_with_options(
        &self,
        context: &TurnContext,
        options: &StepOptions,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        let _ = options;
        self.next_steps_with_context(context)
    }

    /// provider 侧的压缩能力声明。
    ///
    /// 默认 [`CompactionCapability::Unsupported`]；实现方可覆写为
    /// [`CompactionCapability::Declared`]，但本仓尚无 provenance 校验与端点绑定，
    /// **不会**因此被允许执行 native 压缩（见
    /// [`crate::runtime::compaction::native_compact`]）。带默认实现是为了让
    /// 既有 `impl Provider` 零改动。
    fn compaction_capability(&self) -> CompactionCapability {
        CompactionCapability::Unsupported
    }
}

/// 测试用回显 provider：把用户输入原样回显，并产出 `Finished`。
#[derive(Debug, Clone, Copy, Default)]
pub struct EchoProvider;

impl Provider for EchoProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Ok(vec![
            ProviderStep::Thinking(format!("echo: 收到 {} 个字符", user_text.chars().count())),
            ProviderStep::Text(user_text.to_string()),
            ProviderStep::Finished,
        ])
    }
}

/// 真实模型 provider 占位：明确报未实现，不静默回退到 echo。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnimplementedProvider;

impl Provider for UnimplementedProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Err(CoreError::Unsupported(
            "模型 provider 未接入（v0 只做网关与运行时骨架）",
        ))
    }
}
