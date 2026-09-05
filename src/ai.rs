//! LLM 集成层：配置（`config`）→ 客户端（`client`）→ 线格式 DTO（`dto`）；
//! 厂家定制（`provider`）一家一个子模块，只在配置解析注入生效值，不另建客户端。
//!
//! 分层原则：DTO 只描述数据形状，客户端只负责传输与错误映射，配置只负责加载。

pub mod client;
pub mod config;
pub mod dto;
pub mod provider;
