//! 流式（SSE）响应的分块线格式：`object = "chat.completion.chunk"`。
//!
//! 流式期间每个 `data:` 行携带一条本结构的 JSON；文本增量在
//! `choices[0].delta.content`。流的终止哨兵 `data: [DONE]` 不是 JSON，
//! 由客户端单独处理，不经本类型解析。

use serde::{Deserialize, Serialize};

/// 一条 SSE `data:` 分块。
///
/// ```
/// use o_wakaka::ai::dto::openai_chat::chunk::Chunk;
///
/// // 首块：delta 携带 role
/// let first: Chunk = serde_json::from_str(
///     r#"{"id":"c1","object":"chat.completion.chunk","choices":[
///         {"index":0,"delta":{"role":"assistant","content":"你"},"finish_reason":null}]}"#,
/// )
/// .unwrap();
/// assert_eq!(first.choices[0].delta.content.as_deref(), Some("你"));
///
/// // 后续块：delta 只有 content，甚至只有 finish_reason，均可解析
/// let last: Chunk = serde_json::from_str(
///     r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
/// )
/// .unwrap();
/// assert_eq!(last.choices[0].delta.content, None);
/// assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Chunk {
    /// 本块的候选增量，通常仅一项；网关心跳块可能为空
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// `id`/`object`/`created`/`model` 等未声明字段平铺收集
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// 单个候选的增量项。
#[derive(Debug, Serialize, Deserialize)]
pub struct ChunkChoice {
    /// 增量内容
    pub delta: Delta,
    /// 结束原因，仅最后一块携带；缺字段（部分网关省略）也可解析
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// 未声明字段（index、logprobs 等）平铺收集
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// `delta` 对象：每块通常只含 `role`、`content`、工具调用增量中的部分字段。
///
/// 与请求侧 `Message` 不同构（消息字段可缺省），故独立建模而非复用。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Delta {
    /// 角色，仅首块出现
    #[serde(default)]
    pub role: Option<String>,
    /// 文本增量；`None` = 本块无文本（如仅含 role 或工具调用）
    #[serde(default)]
    pub content: Option<String>,
    /// 工具调用等扩展字段平铺收集
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
