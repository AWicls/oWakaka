//! 小米 MiMo 开放平台定制接入（文本生成，Responses 端点族）。
//!
//! 官方文档：<https://mimo.mi.com/docs/zh-CN/api/chat/responses>，要点备忘：
//! - 线格式对齐 OpenAI Responses API，通用 responses 实现可直接复用；
//!   但**不支持** `background`/`previous_response_id`/`context_management` 等字段，
//!   携带会被过滤或报错——本项目当前请求形状不含这些字段，无需定制请求组装
//! - 鉴权：`Authorization: Bearer` 与 `api-key` 头二选一，走通用 bearer 路径
//! - `reasoning.effort` 仅 none/low/medium/high，档位间不区分强度（关思考的
//!   `effort:none` 与通用语义一致，见 `client/turn.rs` 的 `thinking_extra`）
//! - 限流（单账号全 key 合计）：mimo-v2.5 / mimo-v2.5-pro = 100 RPM / 10M TPM；
//!   429 经 `ChatError::Api` 原样展示，自动重试/退避暂不在本项目范围

use crate::ai::config::Api;

/// 内置官方根地址（配置显式写 `base_url` 可覆盖，如 token-plan 专属网关）
pub const BASE_URL: &str = "https://api.xiaomimimo.com/v1";

/// 锁定的接口族：定制只走 Responses（通用兼容路径可能与厂商实际行为有差异）
pub const LOCKED_API: Api = Api::Responses;
