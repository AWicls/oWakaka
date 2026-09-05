//! chat 族（`/chat/completions`）传输实现：非流式、流式 SSE 与非流式事件折算。
//! 线格式见 [`crate::ai::dto::openai_chat`]。

use crate::ai::{
    client::{ChatError, Client, StreamEvent},
    dto::openai_chat::{chunk::Chunk, request::Request, response::Response},
};

impl Client {
    /// 非流式对话：`POST {base_url}/chat/completions`，鉴权经 [`Client::authed`]
    /// （通用 Bearer / 定制提供商按厂家方式），JSON 收发前过 [`Client::decorated`] 定制钩子。
    ///
    /// 错误语义见 [`ChatError`]：先取响应体文本再解析，
    /// 保证非 2xx 与"2xx 但非合法 JSON"两种情况都带原始响应体，厂家信息不丢。
    pub async fn chat(&self, req: &Request) -> Result<Response, ChatError> {
        let mut body = serde_json::to_value(req).map_err(|e| ChatError::Decode {
            error: e.to_string(),
            body: String::new(),
        })?;
        self.decorated(&mut body);
        let resp = self
            .authed(
                self.http
                    .post(format!("{}/chat/completions", self.base_url)),
            )
            .json(&body)
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

    /// 流式对话：注入 `stream: true` 后请求同一端点，
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
        self.decorated(&mut body);

        let resp = self
            .authed(
                self.http
                    .post(format!("{}/chat/completions", self.base_url)),
            )
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

    /// chat 族对话入口：按配置 `stream` 分流式 / 非流式（由 [`Client::generate`] 调用）。
    ///
    /// 非流式把整段回复折算成事件现场回调，语义与流式一致（见 [`generate`] 的说明）。
    ///
    /// [`generate`]: Client::generate
    pub(super) async fn generate_chat(
        &self,
        req: &Request,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        if self.stream {
            return self.chat_stream(req, on_event).await;
        }
        let resp = self.chat(req).await?;
        for ev in chat_events(&resp) {
            on_event(ev);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{
        config::{Config, load_toml},
        dto::openai_chat::request::Message,
    };

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

    /// 真实流式测试：断言回答增量非空；思考增量按模型能力计数
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

    /// 非流式响应折算事件：顺序、空段过滤、非标思考字段
    #[test]
    fn chat_nonstream_events() {
        // 思考（扩展字段）在前，正文在后
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
    }
}
