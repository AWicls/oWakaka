use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub input: Vec<Input>,
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Input {
    pub role: String,
    pub content: String,
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
