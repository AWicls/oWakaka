//! LLM 集成层：配置（`config`）→ 客户端（`client`）→ 线格式 DTO（`dto`）。
//!
//! 分层原则：DTO 只描述数据形状，客户端只负责传输与错误映射，配置只负责加载。
//! 后续厂家（Anthropic、定制 API）以同构方式并列生长，一家实现时不预建 trait。

pub mod client;
pub mod config;
pub mod dto;
