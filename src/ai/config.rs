use std::{error::Error, fs, path::Path};

use serde::Deserialize;

/// OpenAI 兼容端点配置（base_url 可指向任何 chat 兼容网关）
#[derive(Debug, Deserialize)]
pub struct Config {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Config {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }
}
