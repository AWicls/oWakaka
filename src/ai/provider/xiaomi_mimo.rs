//! 小米 MiMo 开放平台定制接入（文本生成，Responses 端点族）。
//!
//! 官方文档：<https://mimo.mi.com/docs/zh-CN/api/chat/responses>，厂家差异备忘：
//! - 线格式对齐 OpenAI Responses API，通用 responses 实现可直接复用；
//!   但**不支持** `background`/`previous_response_id`/`context_management` 字段，
//!   携带会被过滤甚至触发请求异常 → [`Customization::decorate_body`] 发送前剔除
//! - 鉴权：`api-key` 头与 `Authorization: Bearer` 二选一，按文档首选走 `api-key`
//! - `reasoning.effort`：none 关闭思考，low/medium/high 均开启且效果一致 →
//!   思考开启时显式发 `"low"`（不依赖厂商缺省），关闭时通用层已平铺 `effort=none`
//! - 模型清单：支持 OpenAI 兼容 `GET /v1/models`
//! - 限流（单账号全 key 合计）：mimo-v2.5 / mimo-v2.5-pro = 100 RPM / 10M TPM；
//!   429 经 `ChatError::Api` 原样展示，自动重试/退避不在本项目范围

use reqwest::RequestBuilder;
use serde_json::{Value, json};

use super::Customization;
use crate::ai::config::Api;

/// 内置官方根地址（配置显式写 `base_url` 可覆盖，如 token-plan 专属网关）
pub const BASE_URL: &str = "https://api.xiaomimimo.com/v1";

/// 锁定的接口族：定制只走 Responses（通用兼容路径可能与厂商实际行为有差异）
pub const LOCKED_API: Api = Api::Responses;

/// 厂商明确不支持的 Responses 请求字段（见模块头文档）
const UNSUPPORTED_FIELDS: [&str; 3] = ["background", "previous_response_id", "context_management"];

/// 定制实现单例（[`super::Provider::spec`] 引用）
pub static SPEC: XiaomiMimo = XiaomiMimo;

/// 小米 MiMo 定制实现：`api-key` 头鉴权 + Responses 请求体厂家字段定制
#[derive(Debug)]
pub struct XiaomiMimo;

impl Customization for XiaomiMimo {
    fn display_name(&self) -> &'static str {
        "小米 MiMo"
    }

    /// 官方文档首选鉴权方式：`api-key` 头（替代通用 Bearer）
    fn authenticate(&self, rb: RequestBuilder, api_key: &str) -> RequestBuilder {
        rb.header("api-key", api_key)
    }

    fn decorate_body(&self, family: Api, body: &mut Value) {
        // 接口族已锁死 Responses；非本族请求（防御）不定制
        if family != LOCKED_API {
            return;
        }
        let Some(obj) = body.as_object_mut() else {
            return;
        };
        for field in UNSUPPORTED_FIELDS {
            obj.remove(field);
        }
        // 思考语义收口：无 reasoning.effort = UI 思考开启 → 显式发最低档（MiMo 各档等效）；
        // 关闭时 turn.rs 已平铺 effort="none"，不覆写
        let reasoning = obj.entry("reasoning").or_insert_with(|| json!({}));
        if reasoning.get("effort").and_then(Value::as_str).is_none() {
            reasoning["effort"] = json!("low");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 鉴权定制：请求带 `api-key` 头（离线 build 检查，不发网络）
    #[test]
    fn authenticate_uses_api_key_header() {
        let rb = reqwest::Client::new().post("https://example.invalid/x");
        let req = SPEC
            .authenticate(rb, "tp-test")
            .build()
            .expect("RequestBuilder::build 应成功");
        assert_eq!(req.headers()["api-key"], "tp-test");
    }

    /// 思考开启（无 effort）→ 注入 low；关闭（effort=none）→ 不覆写
    #[test]
    fn decorate_sets_effort_only_when_absent() {
        let mut on = json!({ "model": "mimo-v2.5" });
        SPEC.decorate_body(Api::Responses, &mut on);
        assert_eq!(on["reasoning"]["effort"], "low");

        let mut off = json!({ "model": "mimo-v2.5", "reasoning": { "effort": "none" } });
        SPEC.decorate_body(Api::Responses, &mut off);
        assert_eq!(off["reasoning"]["effort"], "none");
    }

    /// 不支持字段发送前剔除；非锁定族请求不动
    #[test]
    fn decorate_strips_unsupported_and_skips_other_families() {
        let mut body = json!({
            "model": "mimo-v2.5",
            "background": true,
            "previous_response_id": "resp_x",
            "context_management": [],
        });
        SPEC.decorate_body(Api::Responses, &mut body);
        assert!(body.get("background").is_none());
        assert!(body.get("previous_response_id").is_none());
        assert!(body.get("context_management").is_none());
        assert_eq!(body["reasoning"]["effort"], "low");

        let mut chat = json!({ "model": "m", "background": true });
        SPEC.decorate_body(Api::Chat, &mut chat);
        assert!(chat.get("background").is_some()); // 锁死族外不定制（防御语义）
    }
}
