//! OpenAI chat 响应线格式（公共兼容 DTO）。

use serde::{Deserialize, Serialize};

use super::request::Message;

/// `POST /chat/completions` 响应体。
///
/// 只显式声明本阶段用到的 `choices`；`id`/`created`/`usage`/`model` 等
/// 其余字段自动收集进 `extra`，不做二次解析：
///
/// ```
/// use o_wakaka::ai::dto::openai_chat::response::Response;
///
/// let raw = r#"{"id":"chatcmpl-9","created":1,"choices":[
///     {"index":0,"message":{"role":"assistant","content":"2"}}]}"#;
/// let resp: Response = serde_json::from_str(raw).unwrap();
/// assert_eq!(resp.choices[0].message.content.as_deref(), Some("2"));
/// assert_eq!(resp.extra.unwrap()["id"], "chatcmpl-9");
///
/// // tool_calls/纯推理消息的 content 为 null，必须可解析
/// let tool: Response = serde_json::from_str(
///     r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#,
/// )
/// .unwrap();
/// assert_eq!(tool.choices[0].message.content, None);
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    /// 候选回复列表，常规对话取第一个即可。
    /// `default` 容忍 content_filter/moderation 场景返回的空 `choices`
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// 未声明字段的平铺收集（id、created、usage 等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// 单个候选回复。
#[derive(Debug, Serialize, Deserialize)]
pub struct Choice {
    /// 助手回复消息（多轮回放时可直接作为 `assistant` 消息复用）
    pub message: Message,
    /// 结束原因：`stop` / `length` / `content_filter` 等，部分网关会省略
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// 未声明字段的平铺收集（index、logprobs 等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
