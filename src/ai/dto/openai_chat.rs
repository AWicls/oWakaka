//! OpenAI `/chat/completions` 线格式模块。
//!
//! 该接口族是行业事实兼容标准，本项目以它作为"公共兼容 DTO"：
//! 任何暴露此端点的网关（OpenAI 官方、OneAPI、vLLM、国产兼容层）均可直接复用。

pub mod request;
pub mod response;
