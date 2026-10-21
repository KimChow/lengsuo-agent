//! 适配器配置。
//!
//! 本卡**不新建**任何配置文件格式，也不动 `agent_core::config::RuntimeConfig`：
//! 这里只给出一个结构体 + `Default`，由调用方注入。

/// OpenAI 兼容网关配置。
#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// 网关基地址（形如 `http://host:port`）。
    pub base_url: String,
    /// 模型名。默认走网关的 `auto` 路由。
    pub model: String,
    /// 可选鉴权 token（带上后作 `Authorization: Bearer <key>` 送出）。
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

impl OpenAiConfig {
    /// Chat Completions 端点（OpenAI 规范路径 `/v1/chat/completions`）。
    ///
    /// 基地址的尾斜杠先去掉，避免拼出 `//v1/...`。
    pub fn chat_completions_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_points_at_auto_route() {
        let config = OpenAiConfig::default();
        assert_eq!(config.model, "auto");
        assert_eq!(config.timeout_secs, 60);
        assert!(config.api_key.is_none());
    }

    #[test]
    fn endpoint_normalizes_trailing_slash() {
        let config = OpenAiConfig {
            base_url: "http://host:8080/".to_string(),
            ..OpenAiConfig::default()
        };
        assert_eq!(
            config.chat_completions_url(),
            "http://host:8080/v1/chat/completions"
        );
    }
}
