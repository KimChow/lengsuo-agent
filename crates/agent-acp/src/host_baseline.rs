//! host 侧基线方法表：一个合规 ACP 端点至少要应答的零副作用方法。
//!
//! **生效层**：本模块只注册到 host 的**方法表**（[`crate::host::Host::register_method`]），
//! 与 `agent-core` 存储层、`agent-gateway` 的 `/rpc` 协议面（`域:动作`）**不是同一层**，
//! 三者口径允许不同；这里不出现 `session:create` 一类 v0 动作名。
//!
//! **清单**：只有 [`method::INITIALIZE`] 与 [`method::LOGOUT`]——握手与登出必需，
//! 且都是零副作用；`session/*` 等有状态方法**不在基线内**。
//!
//! **默认不启用**：[`crate::host::Host::new`] 行为不变（空表起步，未注册即 `-32601`），
//! 基线必须由调用方显式 [`register_baseline`]。既有用例依赖「未注册即 `-32601`」
//! 这条不变量，故不做隐式兜底。

use serde_json::Value;

use crate::host::Host;
use crate::message::{JsonRpcErrorObject, JSONRPC_ERROR_INVALID_PARAMS};
use crate::methods::{method, Implementation, InitializeParams, InitializeResult};
use crate::router::{RpcOutcome, ACP_ERROR_INTERNAL};

/// 本端默认支持的 ACP 协议代际（与 18 册 wire 样例一致）。
pub const DEFAULT_PROTOCOL_VERSION: u32 = 1;

/// 基线方法清单：只有握手与登出（均为零副作用）。
///
/// 名字一律取自 [`crate::methods::method`] 常量，禁止写字面量。
pub const BASELINE_METHODS: &[&str] = &[method::INITIALIZE, method::LOGOUT];

/// 基线注册配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineConfig {
    /// 本端支持的最高协议代际；应答取 `min(请求值, 本值)`。
    pub protocol_version: u32,
    /// 应答 `agentInfo.name` 使用的实现名。
    pub agent_name: String,
}

impl Default for BaselineConfig {
    fn default() -> Self {
        Self {
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            agent_name: env!("CARGO_PKG_NAME").to_string(),
        }
    }
}

/// 在 `host` 的方法表上注册基线方法（同名重复注册 = 覆盖）。
///
/// - `initialize`：解析 [`InitializeParams`]，失败回 `-32602`（不 panic）；成功回
///   [`InitializeResult`]，`protocolVersion = min(请求值, 配置值)`，`agentInfo`
///   填本 crate 名与版本，能力位留空。
/// - `logout`：无参数、回 `null`，不写文件、不改全局状态。
pub fn register_baseline(host: &Host, config: &BaselineConfig) {
    let protocol_version = config.protocol_version;
    let agent_name = config.agent_name.clone();

    host.register_method(method::INITIALIZE, move |req| {
        let params: InitializeParams = match req.params_as() {
            Ok(params) => params,
            Err(e) => {
                return RpcOutcome::Err(JsonRpcErrorObject {
                    code: JSONRPC_ERROR_INVALID_PARAMS,
                    message: format!("initialize 参数无效: {e}"),
                    data: None,
                })
            }
        };
        let result = InitializeResult {
            protocol_version: params.protocol_version.min(protocol_version),
            agent_info: Some(Implementation {
                name: agent_name.clone(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            agent_capabilities: None,
            auth_methods: None,
        };
        serialize_result(result)
    });

    host.register_method(method::LOGOUT, |_req| RpcOutcome::Ok(Value::Null));
}

/// 结果序列化失败归为内部错误（字段均为可序列化标量，正常路径不可达）。
fn serialize_result(result: InitializeResult) -> RpcOutcome {
    serde_json::to_value(result).map_err(|e| JsonRpcErrorObject {
        code: ACP_ERROR_INTERNAL,
        message: format!("initialize 结果序列化失败: {e}"),
        data: None,
    })
}
