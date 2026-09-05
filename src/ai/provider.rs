//! 定制提供商注册表：一家 = 一个子模块文件，内置端点、锁定接口族等厂家事实归各家；
//! 本文件只做枚举分发，不预建 trait。
//!
//! 通用兼容（[`Provider::Custom`]）没有独立文件——它就是 `client` 的通用路径本身；
//! 定制提供商同样复用通用线格式实现，差异只在配置解析阶段注入的生效值
//! （见 [`crate::ai::config::Config::effective_base_url`] 与
//! [`crate::ai::config::Config::effective_api`]）。
//!
//! ```
//! use o_wakaka::ai::config::Api;
//! use o_wakaka::ai::provider::Provider;
//!
//! // custom：无内置端点、接口族自由
//! assert_eq!(Provider::Custom.default_base_url(), None);
//! assert_eq!(Provider::Custom.locked_api(), None);
//! // xiaomi_mimo：内置官方端点、锁 Responses 族
//! assert_eq!(
//!     Provider::XiaomiMimo.default_base_url(),
//!     Some("https://api.xiaomimimo.com/v1")
//! );
//! assert_eq!(Provider::XiaomiMimo.locked_api(), Some(Api::Responses));
//! ```

pub mod xiaomi_mimo;

use serde::{Deserialize, Serialize};

use crate::ai::config::Api;

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
}
