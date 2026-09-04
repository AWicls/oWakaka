//! OpenAI chat 兼容客户端。
//!
//! 默认推荐入口是流式 [`Client::chat_stream`]（对话 UI 逐字显示的基础）；
//! 非流式 [`Client::chat`] 保留为简单兜底。
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

use std::time::Duration;

use crate::ai::{
    config::Config,
    dto::openai_chat::{chunk::Chunk, request::Request, response::Response},
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

/// 流式对话的事件：思考增量与回答增量，思考天然先于回答到达。
///
/// 推理模型（DeepSeek-R1、QwQ、Kimi 等及其网关）在 `delta` 中以非标字段
/// `reasoning_content` 或 `reasoning` 携带思考文本，经 [`Delta`](crate::ai::dto::openai_chat::chunk::Delta)
/// 的 `extra` 平铺捕获后转成本事件；不支持思考的模型只会发 [`Content`](StreamEvent::Content)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// 思考过程文本增量
    Reasoning(String),
    /// 回答正文文本增量
    Content(String),
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

    /// 流式对话（默认推荐）：注入 `stream: true` 后请求同一端点，
    /// 每收到一段思考/回答文本增量就同步回调 [`StreamEvent`]，
    /// 读到 `data: [DONE]` 或连接自然关闭即返回 `Ok(())`。
    ///
    /// 语义约定：
    /// - 错误同 [`chat`](Self::chat)；流中途出错时，之前已回调的增量不回滚
    /// - `on_event` 在读循环内同步执行，回调做重活会背压网络读取（UI 刷新场景通常正是期望行为）
    /// - 个别网关不发 `[DONE]`，EOF 视为正常结束而非错误
    pub async fn chat_stream(
        &self,
        req: &Request,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        // stream 标志注入序列化后的请求体，不污染 Request DTO
        let mut body = serde_json::to_value(req).map_err(|e| ChatError::Decode {
            error: e.to_string(),
            body: String::new(),
        })?;
        body["stream"] = serde_json::Value::Bool(true);

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await?;
            return Err(ChatError::Api { status, body });
        }

        let mut resp = resp;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            // 先消费缓冲区里已完整的行（UTF-8 序列不含 0x0A，按 \n 切分安全）
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw);
                // 收到终止哨兵即返回：同块残留字节不再消费
                if handle_sse_line(line.trim_end(), &mut on_event)? {
                    return Ok(());
                }
            }
            match resp.chunk().await? {
                Some(bytes) => buf.extend_from_slice(&bytes),
                None => break, // EOF：处理末尾无换行行的残段后正常结束
            }
        }
        let tail = String::from_utf8_lossy(&buf).trim().to_string();
        if !tail.is_empty() {
            handle_sse_line(&tail, &mut on_event)?;
        }
        Ok(())
    }
}

/// 处理一条 SSE 行。返回 `true` 表示收到 `[DONE]` 终止哨兵。
///
/// 忽略规则：空行（事件边界）、`:` 注释行、非 `data:` 字段行；
/// `data:` 行 JSON 解析失败视为协议错误，立即中止。
/// 同一块内先发思考增量（若有）再发回答增量，保证"先思考后作答"的事件顺序。
fn handle_sse_line(line: &str, on_event: &mut impl FnMut(StreamEvent)) -> Result<bool, ChatError> {
    let line = line.trim_end_matches('\r').trim();
    if line.is_empty() || line.starts_with(':') {
        return Ok(false);
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(true);
    }
    let chunk: Chunk = serde_json::from_str(data).map_err(|e| ChatError::Decode {
        error: e.to_string(),
        body: data.to_string(),
    })?;
    let Some(choice) = chunk.choices.first() else {
        return Ok(false);
    };
    // 思考文本：非标字段，各厂家取 reasoning_content 或 reasoning
    if let Some(r) = choice
        .delta
        .extra
        .as_ref()
        .and_then(|v| v.get("reasoning_content").or_else(|| v.get("reasoning")))
        .and_then(|x| x.as_str())
        && !r.is_empty()
    {
        on_event(StreamEvent::Reasoning(r.to_string()));
    }
    if let Some(text) = choice.delta.content.as_deref()
        && !text.is_empty()
    {
        on_event(StreamEvent::Content(text.to_string()));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{config::load_toml, dto::openai_chat::request::Message};

    /// 真实请求测试：凭据填在 config.test.toml（见 config.example.toml 模板），运行
    /// `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn chat_roundtrip() {
        let cfg = load_toml::<Config>("config.test.toml")
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

    /// 真实流式测试（默认推荐路径）：断言回答增量非空；思考增量按模型能力计数
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn chat_stream_roundtrip() {
        let cfg = load_toml::<Config>("config.test.toml")
            .expect("缺少 config.test.toml，请复制 config.example.toml 并填写");
        let client = Client::from_config(&cfg);
        let req = Request {
            model: cfg.model.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: Some("用一句话开头讲个故事，从'天'字开始".into()),
                extra: None,
            }],
            extra: None,
        };
        let mut thinking: Vec<String> = Vec::new();
        let mut answer: Vec<String> = Vec::new();
        client
            .chat_stream(&req, |ev| match ev {
                StreamEvent::Reasoning(t) => thinking.push(t),
                StreamEvent::Content(t) => answer.push(t),
            })
            .await
            .expect("流式请求失败");
        let full = answer.concat();
        assert!(!full.trim().is_empty(), "未收到任何回答增量");
        println!(
            "思考 {} 段/{} 字，回答 {} 段/{} 字: {full}",
            thinking.len(),
            thinking.concat().chars().count(),
            answer.len(),
            full.chars().count()
        );
    }

    /// 离线验证 SSE 行分发规则：哨兵、注释、心跳、思考+回答顺序、坏 JSON
    #[test]
    fn sse_line_dispatch() {
        let mut got: Vec<StreamEvent> = Vec::new();
        // 空行 / 注释行 / 非 data 字段：忽略且不终止
        assert!(!handle_sse_line("", &mut |e| got.push(e)).unwrap());
        assert!(!handle_sse_line(": ping", &mut |e| got.push(e)).unwrap());
        assert!(!handle_sse_line("event: message", &mut |e| got.push(e)).unwrap());
        assert!(got.is_empty());
        // 同一块内：思考增量必须先于回答增量
        let data =
            r#"{"choices":[{"index":0,"delta":{"content":"Hi","reasoning_content":"think"}}]}"#;
        assert!(!handle_sse_line(&format!("data: {data}"), &mut |e| got.push(e)).unwrap());
        assert_eq!(
            got,
            vec![
                StreamEvent::Reasoning("think".into()),
                StreamEvent::Content("Hi".into())
            ]
        );
        // 终止哨兵
        assert!(handle_sse_line("data: [DONE]", &mut |e| got.push(e)).unwrap());
        // 坏 JSON：报 Decode 而不是静默丢块
        assert!(matches!(
            handle_sse_line("data: {broken", &mut |e| got.push(e)),
            Err(ChatError::Decode { .. })
        ));
    }
}
