use std::time::Duration;

use crate::ai::{
    config::Config,
    dto::openai_chat::{request::Request, response::Response},
};

#[derive(Debug)]
pub enum ChatError {
    /// 传输层失败（网络、超时、响应解析）
    Http(reqwest::Error),
    /// 服务端返回非 2xx，携带原始状态码与响应体
    Api {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatError::Http(e) => write!(f, "HTTP 请求失败: {e}"),
            ChatError::Api { status, body } => write!(f, "服务端返回 {status}: {body}"),
        }
    }
}

impl std::error::Error for ChatError {}

impl From<reqwest::Error> for ChatError {
    fn from(e: reqwest::Error) -> Self {
        ChatError::Http(e)
    }
}

/// OpenAI chat 兼容客户端，对任何提供 `/chat/completions` 的网关通用
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Client {
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

    /// 非流式对话：POST {base_url}/chat/completions
    pub async fn chat(&self, req: &Request) -> Result<Response, ChatError> {
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(req)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Api { status, body });
        }
        Ok(resp.json().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::dto::openai_chat::request::Message;

    /// 真实请求测试：先按 config.example.toml 填好 config.local.toml，再运行
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "需要 config.local.toml 中的真实凭据"]
    async fn chat_roundtrip() {
        let cfg = Config::from_file("config.local.toml")
            .expect("缺少 config.local.toml，请复制 config.example.toml 并填写");
        let client = Client::from_config(&cfg);
        let req = Request {
            model: cfg.model.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: "1+1 等于几？只回答数字。".into(),
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
            .clone();
        assert!(!content.trim().is_empty(), "返回内容为空");
        println!("模型回复: {content}");
    }
}
