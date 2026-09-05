//! OpenAI 兼容对话客户端：chat（`/chat/completions`，保底兼容）与
//! Responses（`/responses`，配置手动启用）两个端点族并存，
//! 默认入口 [`Client::generate`] 按配置 `api`（缺省 `chat`）与
//! `stream`（缺省 `true`）分发到四者之一。
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

use std::time::Duration;

use crate::ai::{
    config::{Api, Config},
    dto::{
        openai_chat::{chunk::Chunk, request::Request, response::Response},
        openai_response::{
            event::Event as RespEvent,
            request::{Input, Request as RespRequest},
            response::Response as RespResponse,
        },
    },
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

/// OpenAI 兼容客户端，同一实例按配置在 chat 与 Responses 两个端点族间分发。
///
/// 无状态：可跨任务克隆共享的是内部 `reqwest::Client`（自带连接池），
/// 因此同一 `Config` 建一个实例长期使用即可。
pub struct Client {
    http: reqwest::Client,
    /// 已去掉末尾斜杠的 API 根地址
    base_url: String,
    api_key: String,
    /// 配置的端点族，决定 [`generate`](Self::generate) 分发去向
    api: Api,
    /// 配置的流式开关，[`generate`](Self::generate) 消费
    stream: bool,
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
            api: cfg.api,
            stream: cfg.stream,
        }
    }

    /// 非流式对话（chat 族）：`POST {base_url}/chat/completions`，Bearer 鉴权，JSON 收发。
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

    /// 流式对话（chat 族）：注入 `stream: true` 后请求同一端点，
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

    /// 非流式对话（Responses 族）：`POST {base_url}/responses`，
    /// 请求体由公共兼容 DTO [`Request`] 经本模块 `to_resp_request` 转换而来。
    ///
    /// 正文提取用 [`Response::assistant_text`](RespResponse::assistant_text)，
    /// 思考摘要用 [`Response::reasoning_text`](RespResponse::reasoning_text)。
    pub async fn respond(&self, req: &Request) -> Result<RespResponse, ChatError> {
        let resp = self
            .http
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&to_resp_request(req))
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

    /// 流式对话（Responses 族）：`stream: true` 请求 `/responses`，
    /// 按语义事件转 [`StreamEvent`] 同步回调。
    ///
    /// 与 chat 族流式的差异只在事件协议，网络骨架相同：
    /// - 终止事件 `response.completed` / `response.incomplete` 或 `[DONE]` 哨兵
    ///   （部分网关照 chat 习惯追加）即返回；EOF 同样视为正常结束
    /// - `error` / `response.failed` 事件报 [`ChatError::Stream`]，
    ///   之前已回调的增量不回滚
    pub async fn respond_stream(
        &self,
        req: &Request,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        let mut body =
            serde_json::to_value(to_resp_request(req)).map_err(|e| ChatError::Decode {
                error: e.to_string(),
                body: String::new(),
            })?;
        body["stream"] = serde_json::Value::Bool(true);

        let resp = self
            .http
            .post(format!("{}/responses", self.base_url))
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
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&raw);
                if handle_resp_sse_line(line.trim_end(), &mut on_event)? {
                    return Ok(());
                }
            }
            match resp.chunk().await? {
                Some(bytes) => buf.extend_from_slice(&bytes),
                None => break,
            }
        }
        let tail = String::from_utf8_lossy(&buf).trim().to_string();
        if !tail.is_empty() {
            handle_resp_sse_line(&tail, &mut on_event)?;
        }
        Ok(())
    }

    /// 统一对话入口（UI 默认路径）：按配置的端点族与流式开关分发。
    ///
    /// 非流式分支把整段回复折算成一至两个事件（先思考后回答）现场回调，
    /// 事件语义与流式一致，调用方无需区分。
    pub async fn generate(
        &self,
        req: &Request,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        match (self.api, self.stream) {
            (Api::Responses, true) => self.respond_stream(req, on_event).await,
            (Api::Responses, false) => {
                let resp = self.respond(req).await?;
                for ev in resp_events(&resp) {
                    on_event(ev);
                }
                Ok(())
            }
            (Api::Chat, true) => self.chat_stream(req, on_event).await,
            (Api::Chat, false) => {
                let resp = self.chat(req).await?;
                for ev in chat_events(&resp) {
                    on_event(ev);
                }
                Ok(())
            }
        }
    }
}

/// 公共兼容请求 DTO → Responses 请求体：`messages` → `input`，
/// 正文 `None` 折成空串；两侧 `extra` 原样透传。
fn to_resp_request(req: &Request) -> RespRequest {
    RespRequest {
        model: req.model.clone(),
        input: req
            .messages
            .iter()
            .map(|m| Input {
                role: m.role.clone(),
                content: m.content.clone().unwrap_or_default(),
                extra: m.extra.clone(),
            })
            .collect(),
        extra: req.extra.clone(),
    }
}

/// 处理一条 SSE 行（chat 族）。返回 `true` 表示收到 `[DONE]` 终止哨兵。
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

/// 处理一条 Responses 族的 SSE 行。返回 `true` 表示流已终止。
///
/// 事件按 `data:` JSON 的 `type` 分发（`event:` 行与 chat 族一样忽略）；
/// 未知事件类型忽略，保证网关新增事件不破坏解析。
fn handle_resp_sse_line(
    line: &str,
    on_event: &mut impl FnMut(StreamEvent),
) -> Result<bool, ChatError> {
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
    let event: RespEvent = serde_json::from_str(data).map_err(|e| ChatError::Decode {
        error: e.to_string(),
        body: data.to_string(),
    })?;
    match event.kind.as_str() {
        "response.output_text.delta" => {
            if let Some(t) = event.delta.as_deref().filter(|t| !t.is_empty()) {
                on_event(StreamEvent::Content(t.to_string()));
            }
        }
        // 思考增量：官方为摘要文本事件，部分网关发原始推理事件，二者都收
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(t) = event.delta.as_deref().filter(|t| !t.is_empty()) {
                on_event(StreamEvent::Reasoning(t.to_string()));
            }
        }
        // 终止事件：completed/incomplete 正常收尾；failed 带服务端错误，报出去
        "response.completed" | "response.incomplete" => return Ok(true),
        "response.failed" => {
            return Err(ChatError::Stream {
                message: resp_error_message(&event),
            });
        }
        "error" => {
            return Err(ChatError::Stream {
                message: resp_error_message(&event),
            });
        }
        _ => {}
    }
    Ok(false)
}

/// 从 `error` / `response.failed` 事件提取服务端错误文案。
///
/// 规范里 error 事件的 message 在顶层；`response.failed` 嵌在
/// `response.error.message`，部分网关再把 error 事件嵌一层，逐一兜底。
fn resp_error_message(event: &RespEvent) -> String {
    let nested = |keys: &[&str]| {
        event
            .extra
            .as_ref()
            .and_then(|v| {
                keys.iter()
                    .try_fold(v as &serde_json::Value, |acc, k| acc.get(*k))
            })
            .and_then(|m| m.as_str())
            .map(str::to_string)
    };
    event
        .message
        .clone()
        .or_else(|| nested(&["error", "message"]))
        .or_else(|| nested(&["response", "error", "message"]))
        .unwrap_or_else(|| "服务端未提供错误详情".to_string())
}

/// 非流式 chat 响应 → 事件序列（先思考后回答，空段不发）。
///
/// 思考文本走非标扩展字段，取值键与流式 [`handle_sse_line`] 一致。
fn chat_events(resp: &Response) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let Some(choice) = resp.choices.first() else {
        return events;
    };
    if let Some(r) = choice
        .message
        .extra
        .as_ref()
        .and_then(|v| v.get("reasoning_content").or_else(|| v.get("reasoning")))
        .and_then(|x| x.as_str())
        && !r.is_empty()
    {
        events.push(StreamEvent::Reasoning(r.to_string()));
    }
    if let Some(t) = choice.message.content.as_deref()
        && !t.is_empty()
    {
        events.push(StreamEvent::Content(t.to_string()));
    }
    events
}

/// 非流式 Responses 响应 → 事件序列（先思考摘要后正文，空段不发）。
fn resp_events(resp: &RespResponse) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    let reasoning = resp.reasoning_text();
    if !reasoning.is_empty() {
        events.push(StreamEvent::Reasoning(reasoning));
    }
    let content = resp.assistant_text();
    if !content.is_empty() {
        events.push(StreamEvent::Content(content));
    }
    events
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

    /// 真实流式测试（chat 族）：断言回答增量非空；思考增量按模型能力计数
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

    /// 真实请求测试（Responses 非流式）：需支持 `/responses` 的凭据，
    /// 运行 `cargo test -- --ignored`
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn respond_roundtrip() {
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
        let resp = client.respond(&req).await.expect("responses 请求失败");
        let content = resp.assistant_text();
        assert!(!content.trim().is_empty(), "返回内容为空");
        println!("模型回复: {content}");
    }

    /// 真实流式测试（Responses 族）：断言回答增量非空；思考摘要按模型能力计数
    #[tokio::test]
    #[ignore = "需要 config.test.toml 中的真实凭据"]
    async fn respond_stream_roundtrip() {
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
            .respond_stream(&req, |ev| match ev {
                StreamEvent::Reasoning(t) => thinking.push(t),
                StreamEvent::Content(t) => answer.push(t),
            })
            .await
            .expect("responses 流式请求失败");
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

    /// 公共 DTO → Responses 请求体：消息映射与 extra 透传
    #[test]
    fn to_resp_request_maps_messages() {
        let req = Request {
            model: "gpt-4o".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some("简洁".into()),
                    extra: None,
                },
                Message {
                    role: "assistant".into(),
                    content: None, // 无正文消息折成空串
                    extra: None,
                },
            ],
            extra: Some(serde_json::json!({ "temperature": 0.5 })),
        };
        let json = serde_json::to_value(to_resp_request(&req)).unwrap();
        assert_eq!(json["input"][0]["role"], "system");
        assert_eq!(json["input"][0]["content"], "简洁");
        assert_eq!(json["input"][1]["content"], "");
        assert_eq!(json["temperature"], 0.5);
        assert!(json.get("messages").is_none());
    }

    /// 离线验证 Responses SSE 事件分发：增量、思考、忽略、终止、错误
    #[test]
    fn resp_sse_event_dispatch() {
        let mut got: Vec<StreamEvent> = Vec::new();
        // event: 行与非 delta 生命周期事件：忽略且不终止
        assert!(
            !handle_resp_sse_line("event: response.output_text.delta", &mut |e| got.push(e))
                .unwrap()
        );
        assert!(
            !handle_resp_sse_line(r#"data: {"type":"response.created"}"#, &mut |e| got.push(e))
                .unwrap()
        );
        assert!(got.is_empty());
        // 思考增量先于回答增量（同一条测试流内）
        assert!(
            !handle_resp_sse_line(
                r#"data: {"type":"response.reasoning_summary_text.delta","delta":"想"}"#,
                &mut |e| got.push(e)
            )
            .unwrap()
        );
        assert!(
            !handle_resp_sse_line(
                r#"data: {"type":"response.output_text.delta","delta":"答","sequence_number":9}"#,
                &mut |e| got.push(e)
            )
            .unwrap()
        );
        assert_eq!(
            got,
            vec![
                StreamEvent::Reasoning("想".into()),
                StreamEvent::Content("答".into())
            ]
        );
        // 空 delta 不发事件
        assert!(
            !handle_resp_sse_line(
                r#"data: {"type":"response.output_text.delta","delta":""}"#,
                &mut |e| got.push(e)
            )
            .unwrap()
        );
        assert_eq!(got.len(), 2);
        // 终止：completed / incomplete / [DONE] 哨兵
        assert!(
            handle_resp_sse_line(r#"data: {"type":"response.completed"}"#, &mut |_| {}).unwrap()
        );
        assert!(
            handle_resp_sse_line(r#"data: {"type":"response.incomplete"}"#, &mut |_| {}).unwrap()
        );
        assert!(handle_resp_sse_line("data: [DONE]", &mut |_| {}).unwrap());
        // 顶层 message 的 error 事件
        assert!(matches!(
            handle_resp_sse_line(
                r#"data: {"type":"error","message":"boom"}"#,
                &mut |_| {}
            ),
            Err(ChatError::Stream { message }) if message == "boom"
        ));
        // 嵌套变体：error.message 与 response.error.message（response.failed）
        assert!(matches!(
            handle_resp_sse_line(
                r#"data: {"type":"error","error":{"message":"nested"}}"#,
                &mut |_| {}
            ),
            Err(ChatError::Stream { message }) if message == "nested"
        ));
        assert!(matches!(
            handle_resp_sse_line(
                r#"data: {"type":"response.failed","response":{"error":{"message":"failed!"}}}"#,
                &mut |_| {}
            ),
            Err(ChatError::Stream { message }) if message == "failed!"
        ));
        // 兜底文案与坏 JSON
        assert!(matches!(
            handle_resp_sse_line(r#"data: {"type":"error"}"#, &mut |_| {}),
            Err(ChatError::Stream { message }) if message == "服务端未提供错误详情"
        ));
        assert!(matches!(
            handle_resp_sse_line("data: {broken", &mut |_| {}),
            Err(ChatError::Decode { .. })
        ));
    }

    /// 非流式响应折算事件：顺序、空段过滤、非标思考字段
    #[test]
    fn nonstream_events() {
        // chat 族：思考（扩展字段）在前，正文在后
        let resp: Response = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"2","reasoning_content":"1+1"}}]}"#,
        )
        .unwrap();
        assert_eq!(
            chat_events(&resp),
            vec![
                StreamEvent::Reasoning("1+1".into()),
                StreamEvent::Content("2".into())
            ]
        );
        // 正文为 null（纯思考/纯工具）：只发思考
        let only_thinking: Response = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":null,"reasoning":"hm"}}]}"#,
        )
        .unwrap();
        assert_eq!(
            chat_events(&only_thinking),
            vec![StreamEvent::Reasoning("hm".into())]
        );
        // 空 choices：空序列
        let empty: Response = serde_json::from_str(r#"{"choices":[]}"#).unwrap();
        assert!(chat_events(&empty).is_empty());

        // Responses 族：reasoning 摘要 + message 正文
        let resp: RespResponse = serde_json::from_str(
            r#"{"output":[
                {"type":"reasoning","summary":[{"type":"summary_text","text":"想一想"}]},
                {"type":"message","content":[{"type":"output_text","text":"答案"}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            resp_events(&resp),
            vec![
                StreamEvent::Reasoning("想一想".into()),
                StreamEvent::Content("答案".into())
            ]
        );
        // 无推理条目：只有正文
        let plain: RespResponse = serde_json::from_str(
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}]}"#,
        )
        .unwrap();
        assert_eq!(resp_events(&plain), vec![StreamEvent::Content("hi".into())]);
    }
}
