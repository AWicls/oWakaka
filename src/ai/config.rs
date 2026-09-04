//! 运行配置：从 TOML 文件加载 LLM 端点凭据。
//! 凭据文件 `config.local.toml` 已被 git 忽略，模板见项目根 `config.example.toml`。

use std::{error::Error, fs, path::Path};

use serde::Deserialize;

/// OpenAI 兼容端点配置。
///
/// `base_url` 可指向任何实现 `/chat/completions` 的网关，
/// 三要素之外暂不收其他配置（YAGNI，出现真实需求再加）。
#[derive(Debug, Deserialize)]
pub struct Config {
    /// API 根地址，如 `https://api.openai.com/v1`（末尾斜杠可选，客户端会归一化）
    pub base_url: String,
    /// Bearer 鉴权密钥，仅存在于本地配置文件，不进代码库
    pub api_key: String,
    /// 默认模型名，随请求体 `model` 字段发送
    pub model: String,
}

impl Config {
    /// 从 TOML 文件加载配置。
    ///
    /// 示例直接读仓库内的模板文件，可离线运行：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oWakaka::ai::config::Config;
    /// let cfg = Config::from_file("config.example.toml")?;
    /// assert_eq!(cfg.base_url, "https://api.openai.com/v1");
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }
}
