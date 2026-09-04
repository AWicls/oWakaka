use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub choices: Vec<Choice>,
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
