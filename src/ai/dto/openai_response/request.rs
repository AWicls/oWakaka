//! OpenAI Responses API 请求线格式（`POST /responses`）。
//!
//! `input` 收窄到对话场景需要的"角色 + 纯文本"消息项（EasyInputMessage）；
//! 更复杂的输入条目（图像、函数调用等）与厂家扩展参数经 `extra` 平铺进出。

use serde::{Deserialize, Serialize};

/// `POST /responses` 请求体。
///
/// `extra` 中的自定义字段与标准字段一起平铺序列化：
///
/// ```
/// use o_wakaka::ai::dto::openai_response::request::{Input, Request};
///
/// let req = Request {
///     model: "gpt-4o".into(),
///     input: vec![Input {
///         role: "user".into(),
///         content: "你好".into(),
///         extra: None,
///     }],
///     extra: Some(serde_json::json!({ "instructions": "简洁作答" })),
/// };
/// let json = serde_json::to_value(&req).unwrap();
/// assert_eq!(json["instructions"], "简洁作答");
/// assert_eq!(json["input"][0]["content"], "你好");
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    /// 模型名，如 `gpt-4o`
    pub model: String,
    /// 会话历史（消息形输入项），按时间顺序排列
    pub input: Vec<Input>,
    /// 请求级自定义字段（`instructions`、`temperature` 等），平铺进请求体顶层
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// `input` 中的一条消息输入项。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Input {
    /// 角色：`system` / `developer` / `user` / `assistant`
    pub role: String,
    /// 纯文本正文，空字符串原样发送（由服务端裁定是否接受）
    pub content: String,
    /// 消息级自定义字段（`type`、`phase` 等），平铺进出
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
