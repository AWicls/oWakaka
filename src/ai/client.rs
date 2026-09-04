//! OpenAI chat 兼容客户端（非流式）。
//!
//! 端到端用法（构造部分可离线运行，`chat()` 需真实凭据）：
//!
//! ```
//! use o_wakaka::ai::{
//!     client::Client,
//!     config::Config,
//!     dto::openai_chat::request::{Message, Request},
//! };
//!
//! let cfg = Config {
//!     base_url: "https://api.openai.com/v1".into(),
//!     api_key: "sk-test".into(),
//!     model: "gpt-4o-mini".into(),
//! };
//! let client = Client::from_config(&cfg);
//! let req = Request {
//!     model: cfg.model.clone(),
//!     messages: vec![Message {
//!         role: "user".into(),
//!         content: Some("你好".into()),
//!         extra: None,
//!     }],
//!     extra: None,
//! };
//! // 真实调用：let resp = client.chat(&req).await?;（见 #[ignore] 集成测试）
//! ```

use std::time::Duration;

use crate::ai::{
    config::Config,
    dto::openai_chat::{request::Request, response::Response},
};

/// 对话请求的失败类型：区分"传输层问题"、"服务端业务错误"与"响应形状不符"。
#[derive(Debug)]
pub enum ChatError {
    /// 传输层失败（连不上、超时、响应非 UTF-8 等）
    Http(reqwest::Error),
    /// 服务端返回非 2xx，原样携带状态码与响应体，便于排查厂家错误格式
    Api {
        /// HTTP 状态码，如 401 / 429 / 500
        status: reqwest::StatusCode,
        /// 原始响应体（通常是厂家的 JSON 错误详情）
        body: String,
    },
    /// 2xx 但响应体不是合法的 chat JSON（网关返回了 HTML 错误页等意外内容）
    Decode {
        /// serde 解析错误描述
        error: String,
        /// 原始响应体，供定位厂家实际返回了什么
        body: String,
    },
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatError::Http(e) => write!(f, "HTTP 请求失败: {e}"),
            ChatError::Api { status, body } => write!(f, "服务端返回 {status}: {body}"),
            ChatError::Decode { error, body } => write!(f, "响应解析失败: {error}: {body}"),
        }
    }
}

impl std::error::Error for ChatError {}

impl From<reqwest::Error> for ChatError {
    fn from(e: reqwest::Error) -> Self {
        ChatError::Http(e)
    }
}

/// OpenAI chat 兼容客户端，对任何提供 `/chat/completions` 的网关通用。
///
/// 无状态：可跨任务克隆共享的是内部 `reqwest::Client`（自带连接池），
/// 因此同一 `Config` 建一个实例长期使用即可。
pub struct Client {
    http: reqwest::Client,
    /// 已去掉末尾斜杠的 API 根地址
    base_url: String,
    api_key: String,
}

impl Client {
    /// 由配置构建客户端。`base_url` 末尾多余的 `/` 会被归一化。
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            // 超时仅防挂死，builder 失败时退回默认客户端
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            api_key: cfg.api_key.clone(),
        }
    }

    /// 非流式对话：`POST {base_url}/chat/completions`，Bearer 鉴权，JSON 收发。
    ///
    /// 错误语义见 [`ChatError`]：先取响应体文本再解析，
    /// 保证非 2xx 与"2xx 但非合法 JSON"两种情况都带原始响应体，厂家信息不丢。
    pub async fn chat(&self, req: &Request) -> Result<Response, ChatError> {
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(req)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(ChatError::Api { status, body });
        }
        serde_json::from_str(&body).map_err(|e| ChatError::Decode {
            error: e.to_string(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::dto::openai_chat::request::Message;

    /// 真实请求测试：凭据填在 config.test.toml（见 config.example.toml 模板），运行
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn chat_roundtrip() {
        let cfg = Config::from_file("config.test.toml")
            .expect("缺少 config.test.toml，请复制 config.example.toml 并填写");
        let client = Client::from_config(&cfg);
        let req = Request {
            model: cfg.model.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: Some("1+1 等于几？只回答数字。".into()),
                extra: None,
            }],
            extra: None,
        };
        let resp = client.chat(&req).await.expect("对话请求失败");
        let content = resp
            .choices
            .first()
            .expect("choices 为空")
            .message
            .content
            .clone()
            .unwrap_or_default();
        assert!(!content.trim().is_empty(), "返回内容为空");
        println!("模型回复: {content}");
    }
}
