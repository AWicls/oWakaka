//! 运行配置与通用 TOML 配置读写工具。
//!
//! 凭据文件约定（均含密钥、已被 git 忽略，模板见项目根 `config.example.toml`）：
//! - `config.toml`：正式代码使用，经 [`Config::load`] 读取
//! - `config.test.toml`：需真实凭据的测试使用（`#[ignore]`，`cargo test -- --ignored`）
//!
//! 文档测试不依赖上述本地文件：用 [`load_toml`]/[`store_toml`] 在临时目录自造配置。

use std::{collections::BTreeMap, error::Error, fs, path::Path};

use serde::{Deserialize, Serialize};

/// 从 TOML 文件读取任意配置类型（通用工具，不限于 [`Config`]）。
pub fn load_toml<T: serde::de::DeserializeOwned>(
    path: impl AsRef<Path>,
) -> Result<T, Box<dyn Error>> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

/// 将任意可序列化配置以 pretty TOML 写入文件（通用工具）。
///
/// 读写往返与 Debug 脱敏一起验证，无需本地凭据文件：
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::collections::BTreeMap;
/// use o_wakaka::ai::config::{Api, Config, load_toml, store_toml};
///
/// let path = std::env::temp_dir().join("o_wakaka_cfg_doctest.toml");
/// let cfg = Config {
///     base_url: "https://api.openai.com/v1".into(),
///     api_key: "sk-test".into(),
///     model: "gpt-4o-mini".into(),
///     api: Some(Api::Responses),
///     models: BTreeMap::from([("gpt-4o-mini".to_string(), "主力模型".to_string())]),
///     ..Config::default()
/// };
/// store_toml(&cfg, &path)?;
/// let back: Config = load_toml(&path)?;
/// std::fs::remove_file(&path).ok();
///
/// assert_eq!(back.base_url, cfg.base_url);
/// assert_eq!(back.model, cfg.model);
/// assert_eq!(back.api, Some(Api::Responses));
/// assert_eq!(back.models["gpt-4o-mini"], "主力模型");
/// // Debug 输出必须脱敏（防止日志泄漏密钥）
/// assert!(!format!("{cfg:?}").contains(&cfg.api_key));
/// # Ok(())
/// # }
/// ```
pub fn store_toml<T: Serialize>(value: &T, path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
    fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

/// 对话接口族：决定请求发往哪个端点族与按哪种线格式收发。
///
/// 配置取值小写字符串；缺省（字段缺失）为 [`Chat`](Self::Chat) 兼容接口，
/// 手动写 `api = "responses"` 才启用 OpenAI Responses API。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Api {
    /// OpenAI Responses API（`POST /responses`）
    #[serde(alias = "response")]
    Responses,
    /// OpenAI 兼容 Chat Completions（`POST /chat/completions`，默认保底）
    #[default]
    Chat,
}

/// 提供商：通用兼容端点或厂家定制。定义与各家事实已独立至 [`crate::ai::provider`]。
pub use crate::ai::provider::Provider;

/// `stream` 字段缺省值：流式（serde 要求独立函数）。
fn default_stream() -> bool {
    true
}

/// 运行配置（`config.toml` 顶级表）。
///
/// `provider` 选通用兼容（custom，默认）或厂家定制；`base_url`/`api` 可缺省，
/// 缺省按提供商内置默认解析（见 [`effective_base_url`](Self::effective_base_url)），
/// 老配置零改动仍然可用。`models` 别名表与 `model` 激活项由 UI 读写回存：
///
/// ```
/// use o_wakaka::ai::config::{Api, Config, Provider};
///
/// // custom（默认）：base_url 必填，api 缺省 = chat 保底，stream 缺省 = true
/// let cfg: Config =
///     toml::from_str("base_url=\"u\"\napi_key=\"k\"\nmodel=\"m\"\n").unwrap();
/// assert_eq!(cfg.provider, Provider::Custom);
/// assert_eq!(cfg.effective_api(), Api::Chat);
/// assert!(cfg.stream);
/// assert!(cfg.validate().is_ok());
///
/// // xiaomi_mimo：端点缺省 = 官方内置、接口族锁 responses；显式写 base_url 可覆盖为专属网关
/// let mimo: Config = toml::from_str(
///     "provider=\"xiaomi_mimo\"\napi_key=\"k\"\nmodel=\"mimo-v2.5\"\n",
/// )
/// .unwrap();
/// assert_eq!(mimo.effective_base_url(), "https://api.xiaomimimo.com/v1");
/// assert_eq!(mimo.effective_api(), Api::Responses);
/// let bad: Config = toml::from_str(
///     "provider=\"xiaomi_mimo\"\napi_key=\"k\"\nmodel=\"m\"\napi=\"chat\"\n",
/// )
/// .unwrap();
/// assert!(bad.validate().is_err()); // 定制提供商接口族锁死
///
/// // [models] 别名表：键 = 模型 ID，值 = UI 显示名
/// let aliased: Config = toml::from_str(
///     "base_url=\"u\"\napi_key=\"k\"\nmodel=\"m\"\n[models]\nm = \"主力模型\"\n",
/// )
/// .unwrap();
/// assert_eq!(aliased.models["m"], "主力模型");
/// ```
#[derive(Serialize, Deserialize, Default)]
pub struct Config {
    /// 提供商：`"custom"`（默认）| `"xiaomi_mimo"`（小米 MiMo，端点内置、接口族锁死）
    #[serde(default)]
    pub provider: Provider,
    /// API 根地址，如 `https://api.openai.com/v1`（末尾斜杠可选，客户端会归一化）。
    /// custom 必填；定制提供商缺省用内置端点，显式写了以显式为准
    #[serde(default)]
    pub base_url: String,
    /// Bearer 鉴权密钥，仅存在于本地配置文件，不进代码库
    pub api_key: String,
    /// 当前激活模型名，随请求体 `model` 字段发送；UI 切换后回写此字段
    pub model: String,
    /// 对话接口族（仅 custom 可自由配）：缺省 `chat`，手动写 `"responses"` 启用新接口
    #[serde(default)]
    pub api: Option<Api>,
    /// 是否流式输出，缺省 `true`；`false` 时整段一次性返回
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// 可用模型别名表（可选）：模型 ID → UI 显示名；未列的 ID 原样显示
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
}

/// 手动实现：`api_key` 恒为脱敏占位，防止 `{:?}`/日志/panic 输出泄漏密钥。
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("api", &self.api)
            .field("stream", &self.stream)
            .field("models", &self.models)
            .finish()
    }
}

impl Config {
    /// 读取正式配置 `config.toml`（相对于运行时工作目录）。
    ///
    /// 需真实凭据的测试请改用 [`load_toml`] 读取 `config.test.toml`。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        load_toml("config.toml")
    }

    /// 生效根地址：显式写的 `base_url` 优先，其次提供商内置默认；空 = 未配置。
    pub fn effective_base_url(&self) -> &str {
        if self.base_url.is_empty() {
            self.provider.default_base_url().unwrap_or("")
        } else {
            &self.base_url
        }
    }

    /// 生效接口族：定制提供商锁死（[`Provider::locked_api`]）；custom 缺省 Chat。
    pub fn effective_api(&self) -> Api {
        if let Some(locked) = self.provider.locked_api() {
            return locked;
        }
        self.api.unwrap_or_default()
    }

    /// 配置完备性检查，`Err` 携带可直接展示的文案（[`Client::load`](super::client::Client::load) 建客户端前调用）。
    pub fn validate(&self) -> Result<(), String> {
        if self.effective_base_url().is_empty() {
            return Err("缺少 base_url：provider = \"custom\" 时必须显式配置".into());
        }
        if let Some(locked) = self.provider.locked_api()
            && let Some(api) = self.api
            && api != locked
        {
            return Err(format!(
                "定制提供商接口族锁死 {}, api 不能配 \"{}\"",
                api_name(locked),
                api_name(api)
            ));
        }
        Ok(())
    }
}

/// TOML 接口族名（与 [`Api`] 的 serde 改名保持一致），仅用于校验报错文案。
fn api_name(api: Api) -> &'static str {
    match api {
        Api::Chat => "chat",
        Api::Responses => "responses",
    }
}
