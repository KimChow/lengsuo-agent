//! 适配器配置。

/// OpenAI 兼容网关配置。
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// 网关基地址（形如 `http://host:port`）。
    pub base_url: String,
    /// 模型名。默认走网关的 `auto` 路由。
    pub model: String,
    /// 可选鉴权 token。
    pub api_key: Option<String>,
    /// 单次请求超时（秒）。
    pub timeout_secs: u64,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "http://192.168.1.151:18080".to_string(),
            model: "auto".to_string(),
            api_key: None,
            timeout_secs: 60,
        }
    }
}
