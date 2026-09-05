//! Responses 族（`/responses`）传输实现：非流式、语义事件 SSE 与非流式事件折算。
//! 线格式见 [`crate::ai::dto::openai_response`]。

use crate::ai::{
    client::{ChatError, Client, StreamEvent},
    dto::{
        openai_chat::request::Request,
        openai_response::{
            event::Event as RespEvent,
            request::{Input, Request as RespRequest},
            response::Response as RespResponse,
        },
    },
};

impl Client {
    /// 非流式对话：`POST {base_url}/responses`，
    /// 请求体由公共兼容 DTO [`Request`] 经本模块 `to_resp_request` 转换而来。
    ///
    /// 正文提取用 [`Response::assistant_text`](RespResponse::assistant_text)，
    /// 思考摘要用 [`Response::reasoning_text`](RespResponse::reasoning_text)。
    pub async fn respond(&self, req: &Request) -> Result<RespResponse, ChatError> {
        let mut body =
            serde_json::to_value(to_resp_request(req)).map_err(|e| ChatError::Decode {
                error: e.to_string(),
                body: String::new(),
            })?;
        self.decorated(&mut body);
        let resp = self
            .authed(self.http.post(format!("{}/responses", self.base_url)))
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

    /// 流式对话：`stream: true` 请求 `/responses`，按语义事件转 [`StreamEvent`] 同步回调。
    ///
    /// 与 chat 族流式的差异只在事件协议（见 [`handle_resp_sse_line`]），网络骨架相同：
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
        self.decorated(&mut body);

        let resp = self
            .authed(self.http.post(format!("{}/responses", self.base_url)))
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

    /// Responses 族对话入口：按配置 `stream` 分流式 / 非流式（由 [`Client::generate`] 调用）。
    ///
    /// 非流式把整段回复折算成事件现场回调，语义与流式一致（见 [`generate`] 的说明）。
    ///
    /// [`generate`]: Client::generate
    pub(super) async fn generate_responses(
        &self,
        req: &Request,
        mut on_event: impl FnMut(StreamEvent),
    ) -> Result<(), ChatError> {
        if self.stream {
            return self.respond_stream(req, on_event).await;
        }
        let resp = self.respond(req).await?;
        for ev in resp_events(&resp) {
            on_event(ev);
        }
        Ok(())
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
    use crate::ai::{
        config::{Config, load_toml},
        dto::openai_chat::request::Message,
    };

    /// 真实请求测试（非流式）：需支持 `/responses` 的凭据，运行 `cargo test -- --ignored`
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

    /// 真实流式测试：断言回答增量非空；思考摘要按模型能力计数
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

    /// 非流式响应折算事件：思考摘要在前、空段不发
    #[test]
    fn resp_nonstream_events() {
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
