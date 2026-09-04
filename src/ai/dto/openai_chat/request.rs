//! OpenAI chat 请求线格式（公共兼容 DTO）。
//!
//! 未知/自定义字段经 `extra` 平铺（flatten）进出：调用方附加厂家私有参数
//! 无需改结构体，兼容网关的扩展字段也不会丢失。

use serde::{Deserialize, Serialize};

/// `POST /chat/completions` 请求体。
///
/// `extra` 中的自定义字段与标准字段一起平铺序列化：
///
/// ```
/// use oWakaka::ai::dto::openai_chat::request::{Message, Request};
///
/// let req = Request {
///     model: "gpt-4o-mini".into(),
///     messages: vec![Message {
///         role: "user".into(),
///         content: "你好".into(),
///         extra: None,
///     }],
///     extra: Some(serde_json::json!({ "temperature": 0.7 })),
/// };
/// let json = serde_json::to_value(&req).unwrap();
/// assert_eq!(json["temperature"], 0.7);
///
/// // extra 为 None 时不产生多余字段
/// let minimal = serde_json::to_value(Request {
///     model: "m".into(),
///     messages: vec![],
///     extra: None,
/// })
/// .unwrap();
/// assert_eq!(minimal.as_object().unwrap().len(), 2);
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    /// 模型名，如 `gpt-4o-mini`
    pub model: String,
    /// 对话历史，按时间顺序排列
    pub messages: Vec<Message>,
    /// 请求级自定义字段（`temperature`、`tools` 等），平铺进请求体顶层
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// 请求中的一条消息。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    /// 角色：`system` / `user` / `assistant` / `tool` 等，不同网关取值略有差异
    pub role: String,
    /// 消息正文（本阶段仅支持纯文本字符串）
    pub content: String,
    /// 消息级自定义字段（`name`、`tool_calls` 等），平铺进出
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
