//! native 压缩能力面（骨架）的验收用例。
//!
//! 本卡**只**落「能力声明 + 显式拒绝」：native 路径结构上存在，但两档能力
//! 都必须 fail-closed；同时确认既有 summary 路径完全不受影响。

use agent_core::error::CoreError;
use agent_core::provider::{CompactionCapability, Provider, ProviderStep};
use agent_core::runtime::compaction::{
    native_compact, Compaction, CompactionError, CompactionSettings, NativeCompactionRequest,
    TokenEstimator, Trigger, CHECKPOINT_PREFIX,
};
use agent_core::schema::{Message, Role};

/// 最小 provider 桩：只实现必需的 `next_steps`，不覆写压缩能力声明。
struct StubProvider;

impl Provider for StubProvider {
    fn next_steps(
        &self,
        _session_id: uuid::Uuid,
        _user_text: &str,
    ) -> Result<Vec<ProviderStep>, CoreError> {
        Ok(vec![ProviderStep::Finished])
    }
}

/// 固定估算口：按字符数计，便于手算切点。
struct CharEstimator;

impl TokenEstimator for CharEstimator {
    fn estimate(&self, text: &str) -> usize {
        text.chars().count()
    }
}

fn message(role: Role, content: &str) -> Message {
    Message {
        role,
        content: content.to_string(),
        tool: None,
        ts_ms: 0,
    }
}

fn request<'a>(
    history: &'a [Message],
    capability: CompactionCapability,
) -> NativeCompactionRequest<'a> {
    NativeCompactionRequest {
        provider_id: "provider-a",
        history,
        capability,
    }
}

/// 1) 未覆写能力声明的 provider 默认是 `Unsupported`。
#[test]
fn default_provider_reports_unsupported() {
    let provider = StubProvider;
    assert_eq!(
        provider.compaction_capability(),
        CompactionCapability::Unsupported
    );
}

/// 2) `Unsupported` ⇒ `NativeUnsupported`，类型串为 `native_unsupported`。
#[test]
fn unsupported_capability_fails_with_native_unsupported() {
    let history = vec![message(Role::User, "你好")];
    let req = request(&history, CompactionCapability::Unsupported);
    let err = native_compact(CompactionCapability::Unsupported, &req)
        .expect_err("Unsupported 必须 fail-closed");
    assert_eq!(err, CompactionError::NativeUnsupported);
    assert_eq!(err.error_type(), "native_unsupported");
}

/// 3) `Declared` ⇒ `NativeNotImplemented`，类型串为 `native_not_implemented`。
#[test]
fn declared_capability_fails_with_native_not_implemented() {
    let history = vec![message(Role::User, "你好")];
    let req = request(&history, CompactionCapability::Declared);
    let err = native_compact(CompactionCapability::Declared, &req)
        .expect_err("Declared 也必须 fail-closed（尚无实现）");
    assert_eq!(err, CompactionError::NativeNotImplemented);
    assert_eq!(err.error_type(), "native_not_implemented");
}

/// 4) 两个新错误的文案非空，且带「未实现」/「不支持」语义。
#[test]
fn native_error_messages_are_explicit() {
    for err in [
        CompactionError::NativeUnsupported,
        CompactionError::NativeNotImplemented,
    ] {
        let message = err.message();
        assert!(!message.is_empty(), "错误文案不得为空");
        assert!(
            message.contains("未实现") || message.contains("不支持"),
            "错误文案须明确拒绝语义，实际为：{message}"
        );
    }
}

/// 5) 不静默退回：两档都**不返回** `Ok`，也不产出任何消息。
// 卡面明确要求用 `matches!` 断言「不返回 Ok」，故显式保留该形态。
#[allow(clippy::redundant_pattern_matching)]
#[test]
fn native_compaction_never_returns_ok() {
    let history = vec![message(Role::User, "你好")];
    for capability in [
        CompactionCapability::Unsupported,
        CompactionCapability::Declared,
    ] {
        let req = request(&history, capability);
        assert!(
            !matches!(native_compact(capability, &req), Ok(_)),
            "{capability:?} 档不得返回 Ok（禁止静默退回）"
        );
    }
}

/// 6) 既有 summary 路径不受影响：`plan` + `apply` 仍可产出 checkpoint。
#[test]
fn summary_path_still_works() {
    let compaction = Compaction::new(
        CompactionSettings {
            auto: true,
            buffer: None,
            keep: 10,
            window: Some(200_000),
        },
        Box::new(CharEstimator),
    );
    let history = vec![
        message(Role::User, &"x".repeat(100)),
        message(Role::Assistant, &"y".repeat(100)),
        message(Role::User, &"z".repeat(100)),
        message(Role::Assistant, &"w".repeat(100)),
    ];

    let plan = compaction
        .plan(&history, Trigger::Manual)
        .expect("既有 summary 路径应可压缩");
    let produced = compaction.apply(&plan, "摘要正文");

    assert_eq!(produced.len(), 1);
    assert!(
        produced[0].content.starts_with(CHECKPOINT_PREFIX),
        "摘要产物应渲染为 checkpoint 包裹段"
    );
    assert!(produced[0].content.contains("摘要正文"));
}
