//! 运行配置：从 TOML 文件加载 LLM 端点凭据。
//!
//! 文件约定（两者均含密钥，已被 git 忽略，模板见项目根 `config.example.toml`）：
//! - `config.toml`：正式代码使用，经 [`Config::load`] 读取
//! - `config.test.toml`：测试（含文档测试）使用

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
    /// 读取正式配置 `config.toml`（相对于运行时工作目录）。
    pub fn load() -> Result<Self, Box<dyn Error>> {
        Self::from_file("config.toml")
    }

    /// 从指定 TOML 文件加载配置。
    ///
    /// 本地约定测试读 `config.test.toml`（缺失时从 `config.example.toml` 复制填写）：
    ///
    /// ```
    /// use oWakaka::ai::config::Config;
    /// let cfg = Config::from_file("config.test.toml")
    ///     .expect("缺少 config.test.toml，请复制 config.example.toml 并填写");
    /// assert!(!cfg.base_url.is_empty() && !cfg.api_key.is_empty() && !cfg.model.is_empty());
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }
}
