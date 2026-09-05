//! OpenAI Responses API（`/responses` 端点族）线格式。
//!
//! 与 chat 族三点不同：请求体是 `input`（非 `messages`）、
//! 非流式响应是 `output` 条目数组（非 `choices`）、
//! 流式是带 `type` 的语义事件（非 chunk 增量）。

pub mod event;
pub mod request;
pub mod response;
