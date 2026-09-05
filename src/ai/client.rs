//! OpenAI 兼容对话客户端：chat（`/chat/completions`，保底兼容）与
//! Responses（`/responses`，配置手动启用）两个端点族并存，
//! 默认入口 [`Client::generate`] 按配置 `api`（缺省 `chat`）与
//! `stream`（缺省 `true`）分发。各端点族的请求与 SSE 解析按族拆分：
//! 实现见 `client/chat.rs` 与 `client/responses.rs`（均为 [`Client`] 的 `impl` 块），
//! 一轮对话的投送/取消见 `client/turn.rs`（[`Client::spawn_turn`]），
//! 本文件只保留共享类型与分发入口。
//!
//! 端到端用法，演示真实路径"配置文件 → `load_toml` → `Client`"
//! （`chat()` 需真实凭据，此处用临时文件自给自足）：
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use o_wakaka::ai::{
//!     client::Client,
//!     config::{Config, load_toml, store_toml},
//!     dto::openai_chat::request::{Message, Request},
//! };
//!
//! let path = std::env::temp_dir().join("o_wakaka_client_doctest.toml");
//! store_toml(
//!     &Config {
//!         base_url: "https://api.openai.com/v1".into(),
//!         api_key: "sk-test".into(),
//!         model: "gpt-4o-mini".into(),
//!         ..Config::default()
//!     },
//!     &path,
//! )?;
//! let cfg: Config = load_toml(&path)?;
//! std::fs::remove_file(&path).ok();
//!
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
//! # Ok(())
//! # }
//! ```

mod chat;
mod responses;
mod turn;

pub use turn::{TurnEvent, TurnHandle, TurnOptions};

use std::time::Duration;

use crate::ai::{
    config::{Api, Config},
    dto::{models::ModelList, openai_chat::request::Request},
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
    /// 2xx 但响应体不是合法的接口 JSON（网关返回了 HTML 错误页等意外内容）
    Decode {
        /// serde 解析错误描述
        error: String,
        /// 原始响应体，供定位厂家实际返回了什么
        body: String,
    },
    /// Responses 流已开始（HTTP 2xx）后服务端报错（`error` / `response.failed` 事件），
    /// 携带可直接展示的文案；之前已回调的增量不回滚
    Stream {
        /// 服务端给出的错误文本
        message: String,
    },
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatError::Http(e) => write!(f, "HTTP 请求失败: {e}"),
            ChatError::Api { status, body } => write!(f, "服务端返回 {status}: {body}"),
            ChatError::Decode { error, body } => write!(f, "响应解析失败: {error}: {body}"),
            ChatError::Stream { message } => write!(f, "流式响应出错: {message}"),
        }
    }
}

impl std::error::Error for ChatError {}

impl From<reqwest::Error> for ChatError {
    fn from(e: reqwest::Error) -> Self {
        ChatError::Http(e)
    }
}

/// 流式对话的事件：思考增量与回答增量，思考天然先于回答到达。
///
/// chat 族思考来自 `delta` 的非标字段 `reasoning_content` 或 `reasoning`
/// （经 [`Delta`](crate::ai::dto::openai_chat::chunk::Delta) 的 `extra` 平铺捕获）；
/// Responses 族来自思考摘要增量事件。不支持思考的模型只会发
/// [`Content`](StreamEvent::Content)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// 思考过程文本增量
    Reasoning(String),
    /// 回答正文文本增量
    Content(String),
}

/// OpenAI 兼容客户端，同一实例按配置在 chat 与 Responses 两个端点族间分发。
///
/// 定制提供商（如 [`Provider::XiaomiMimo`](crate::ai::provider::Provider::XiaomiMimo)）
/// 不另建客户端：端点与接口族已在 [`Config`](crate::ai::config::Config) 解析出生效值，
/// 线格式仍复用通用实现。
///
/// 无状态：可跨任务克隆共享的是内部 `reqwest::Client`（自带连接池），
/// 因此同一 `Config` 建一个实例长期使用即可。
pub struct Client {
    http: reqwest::Client,
    /// 已去掉末尾斜杠的 API 根地址
    base_url: String,
    api_key: String,
    /// 配置的默认模型名，[`spawn_turn`](Self::spawn_turn) 组装请求时使用
    model: String,
    /// 配置的端点族，决定 [`generate`](Self::generate) 分发去向
    api: Api,
    /// 配置的流式开关，各端点族入口消费
    stream: bool,
}

impl Client {
    /// 读默认凭据文件（`config.toml`）构建客户端：`Config::load` +
    /// [`validate`](crate::ai::config::Config::validate) +
    /// [`from_config`](Self::from_config) 一步到位，错误原样上抛供调用方呈现。
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let cfg = Config::load()?;
        cfg.validate().map_err(Box::<dyn std::error::Error>::from)?;
        Ok(Self::from_config(&cfg))
    }

    /// 由配置构建客户端。端点/接口族取配置的**生效值**（定制提供商的内置默认在此解析）；
    /// `base_url` 末尾多余的 `/` 会被归一化。完备性检查见 [`Config::validate`]。
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            // 超时仅防挂死，builder 失败时退回默认客户端
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
            base_url: cfg.effective_base_url().trim_end_matches('/').to_string(),
            api_key: cfg.api_key.clone(),
            model: cfg.model.clone(),
            api: cfg.effective_api(),
            stream: cfg.stream,
        }
    }

    /// 统一对话入口（UI 默认路径）：按配置 `api` 分发端点族，
    /// 流式/非流式在各族入口内按配置 `stream` 二次分流。
    ///
    /// 非流式分支把整段回复折算成一至两个事件（先思考后回答）现场回调，
    /// 事件语义与流式一致，调用方无需区分。
    pub async fn generate(
        &self,
        req: &Request,
        on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        match self.api {
            Api::Chat => self.generate_chat(req, on_event).await,
            Api::Responses => self.generate_responses(req, on_event).await,
        }
    }

    /// 拉取远端可用模型 ID 列表（`GET {base_url}/models`，OpenAI 兼容格式）。
    ///
    /// UI 模型切换下拉的数据源之一；网关不支持该端点时经 [`ChatError`] 上抛，
    /// 由调用方降级为仅用配置清单。
    pub async fn list_models(&self) -> Result<Vec<String>, ChatError> {
        let resp = self
            .http
            .get(format!("{}/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(ChatError::Api { status, body });
        }
        let list: ModelList = serde_json::from_str(&body).map_err(|e| ChatError::Decode {
            error: e.to_string(),
            body,
        })?;
        Ok(list.data.into_iter().map(|m| m.id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::config::load_toml;

    /// 定制提供商：配置缺省时解析出内置端点 + 锁定的 Responses 接口族
    #[test]
    fn from_config_resolves_mimo_defaults() {
        let cfg: Config =
            toml::from_str("provider=\"xiaomi_mimo\"\napi_key=\"k\"\nmodel=\"mimo-v2.5\"\n")
                .unwrap();
        let client = Client::from_config(&cfg);
        assert_eq!(client.base_url, "https://api.xiaomimimo.com/v1");
        assert_eq!(client.api, Api::Responses);
    }

    /// 真实请求测试：拉取模型清单，运行 `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn list_models_roundtrip() {
        let cfg = load_toml::<Config>("config.test.toml")
            .expect("缺少 config.test.toml，请复制 config.example.toml 并填写");
        let client = Client::from_config(&cfg);
        let models = client.list_models().await.expect("models 请求失败");
        assert!(!models.is_empty(), "模型清单为空");
        println!("远端模型 {models:?}");
    }
}
