//! 定制提供商注册表：一家 = 一个子模块文件，内置端点、锁定接口族、鉴权方式、
//! 请求体定制字段等厂家事实归各家实现 [`Customization`]；本文件只做枚举分发。
//!
//! 通用兼容（[`Provider::Custom`]）没有独立实现——`spec()` 返回 `None`，
//! 客户端走与定制引入前完全一致的通用路径（Bearer + 请求体原样发送）；
//! 定制提供商复用通用线格式，另在发送前经 trait 注入厂家差异。
//!
//! ```
//! use o_wakaka::ai::config::Api;
//! use o_wakaka::ai::provider::Provider;
//!
//! // custom：无内置端点、无接口族锁定、无定制实现
//! assert_eq!(Provider::Custom.default_base_url(), None);
//! assert_eq!(Provider::Custom.locked_api(), None);
//! assert!(Provider::Custom.spec().is_none());
//! // xiaomi_mimo：内置官方端点、锁 Responses 族、有定制实现
//! assert_eq!(
//!     Provider::XiaomiMimo.default_base_url(),
//!     Some("https://api.xiaomimimo.com/v1")
//! );
//! assert_eq!(Provider::XiaomiMimo.locked_api(), Some(Api::Responses));
//! assert_eq!(
//!     Provider::XiaomiMimo.spec().expect("MiMo 应有定制实现").display_name(),
//!     "小米 MiMo"
//! );
//! ```

pub mod xiaomi_mimo;

use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};

use crate::ai::config::Api;

/// 厂家定制接口：通用组装之后、发送之前的所有厂商差异注入点。
///
/// 新提供商实现本 trait（一家一个 `static SPEC` 单例），并在
/// [`Provider::spec`] 的分发中登记；通用路径不经此处。
pub trait Customization: std::fmt::Debug + Send + Sync {
    /// UI 展示名（设置弹窗提供商选项等）
    fn display_name(&self) -> &'static str;

    /// 定制鉴权：按厂商文档推荐方式给请求加认证头。
    /// 通用默认是 `Authorization: Bearer`，厂商另有首选方式时在此覆写
    fn authenticate(&self, rb: RequestBuilder, api_key: &str) -> RequestBuilder;

    /// 请求体出厂定制：注入厂家特有字段、移除厂商不支持的字段。
    /// 由客户端在通用组装（含 `stream` 注入）完成后、发送前调用
    fn decorate_body(&self, family: Api, body: &mut serde_json::Value);
}

/// 提供商：通用兼容端点（默认）或厂家定制。TOML 取值 snake_case。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// 通用 OpenAI 兼容网关：`base_url` 必填，接口族自由配（缺省 chat）
    #[default]
    Custom,
    /// 小米 MiMo 开放平台：内置端点、锁 Responses 族，事实见 [`xiaomi_mimo`]
    XiaomiMimo,
}

impl Provider {
    /// 内置默认根地址（`None` = 配置必须显式提供）
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Provider::Custom => None,
            Provider::XiaomiMimo => Some(xiaomi_mimo::BASE_URL),
        }
    }

    /// 锁定的接口族（`None` = 用户可自由配）：定制提供商只走厂商原生支持的族
    pub fn locked_api(self) -> Option<Api> {
        match self {
            Provider::Custom => None,
            Provider::XiaomiMimo => Some(xiaomi_mimo::LOCKED_API),
        }
    }

    /// 定制实现（`None` = 通用兼容路径，不注入任何厂家差异）
    pub fn spec(self) -> Option<&'static dyn Customization> {
        match self {
            Provider::Custom => None,
            Provider::XiaomiMimo => Some(&xiaomi_mimo::SPEC),
        }
    }

    /// UI 展示名
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Custom => "通用兼容",
            Provider::XiaomiMimo => "小米 MiMo",
        }
    }
}
