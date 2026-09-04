use serde::{Deserialize, Serialize};

use super::request::Message;

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub choices: Vec<Choice>,
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Choice {
    pub message: Message,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
