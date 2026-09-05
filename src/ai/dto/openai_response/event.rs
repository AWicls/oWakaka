//! Responses API 流式（SSE）语义事件线格式。
//!
//! 每个 `data:` 行是一个带 `type` 的 JSON 对象；事件种类很多
//! （created / delta / done / completed…），本客户端只关心类型名与
//! `delta`/`message` 字段，故统一解析成本结构，其余（`sequence_number`、
//! `response` 快照等）平铺进 `extra`。
//! 部分网关在终止事件后仍发 `data: [DONE]` 哨兵，它不是 JSON，由客户端单独处理。

use serde::{Deserialize, Serialize};

/// 一条 SSE 语义事件。
///
/// ```
/// use o_wakaka::ai::dto::openai_response::event::Event;
///
/// let e: Event = serde_json::from_str(
///     r#"{"type":"response.output_text.delta","delta":"hi","sequence_number":3}"#,
/// )
/// .unwrap();
/// assert_eq!(e.kind, "response.output_text.delta");
/// assert_eq!(e.delta.as_deref(), Some("hi"));
///
/// // error 事件按规范 message 在顶层
/// let err: Event =
///     serde_json::from_str(r#"{"type":"error","code":"x","message":"boom"}"#).unwrap();
/// assert_eq!(err.message.as_deref(), Some("boom"));
///
/// // 无 delta 的生命周期事件（response 快照收集进 extra）也可解析
/// let done: Event =
///     serde_json::from_str(r#"{"type":"response.completed","response":{"id":"r1"}}"#).unwrap();
/// assert_eq!(done.delta, None);
/// assert_eq!(done.extra.unwrap()["response"]["id"], "r1");
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
    /// 事件类型名，如 `response.output_text.delta`
    #[serde(rename = "type", default)]
    pub kind: String,
    /// 文本增量，仅 `*.delta` 事件携带
    #[serde(default)]
    pub delta: Option<String>,
    /// 错误文本，仅 `error` 事件携带（规范要求顶层，部分网关嵌 `error.message`）
    #[serde(default)]
    pub message: Option<String>,
    /// 未声明字段的平铺收集（嵌套 error 对象、response 快照等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
