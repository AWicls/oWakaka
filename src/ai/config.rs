//! 运行配置与通用 TOML 配置读写工具。
//!
//! 凭据文件约定（均含密钥、已被 git 忽略，模板见项目根 `config.example.toml`）：
//! - `config.toml`：正式代码使用，经 [`Config::load`] 读取
//! - `config.test.toml`：需真实凭据的测试使用（`#[ignore]`，`cargo test -- --ignored`）
//!
//! 文档测试不依赖上述本地文件：用 [`load_toml`]/[`store_toml`] 在临时目录自造配置。

use std::{error::Error, fs, path::Path};

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
/// use o_wakaka::ai::config::{Config, load_toml, store_toml};
///
/// let path = std::env::temp_dir().join("o_wakaka_cfg_doctest.toml");
/// let cfg = Config {
///     base_url: "https://api.openai.com/v1".into(),
///     api_key: "sk-test".into(),
///     model: "gpt-4o-mini".into(),
/// };
/// store_toml(&cfg, &path)?;
/// let back: Config = load_toml(&path)?;
/// std::fs::remove_file(&path).ok();
///
/// assert_eq!(back.base_url, cfg.base_url);
/// assert_eq!(back.model, cfg.model);
/// // Debug 输出必须脱敏（防止日志泄漏密钥）
/// assert!(!format!("{cfg:?}").contains(&cfg.api_key));
/// # Ok(())
/// # }
/// ```
pub fn store_toml<T: Serialize>(value: &T, path: impl AsRef<Path>) -> Result<(), Box<dyn Error>> {
    fs::write(path, toml::to_string_pretty(value)?)?;
    Ok(())
}

/// OpenAI 兼容端点配置。
///
/// `base_url` 可指向任何实现 `/chat/completions` 的网关，
/// 三要素之外暂不收其他配置（YAGNI，出现真实需求再加）。
#[derive(Serialize, Deserialize)]
pub struct Config {
    /// API 根地址，如 `https://api.openai.com/v1`（末尾斜杠可选，客户端会归一化）
    pub base_url: String,
    /// Bearer 鉴权密钥，仅存在于本地配置文件，不进代码库
    pub api_key: String,
    /// 默认模型名，随请求体 `model` 字段发送
    pub model: String,
}

/// 手动实现：`api_key` 恒为脱敏占位，防止 `{:?}`/日志/panic 输出泄漏密钥。
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
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
}
