//! OpenAI 兼容 `GET /models` 响应线格式（模型清单）。
//!
//! 通用网关与小米 MiMo 同格式，只收模型 ID，`created`/`owned_by` 等字段忽略。

use serde::Deserialize;

/// `GET /models` 响应体。
///
/// ```
/// use o_wakaka::ai::dto::models::ModelList;
///
/// let list: ModelList = serde_json::from_str(
///     r#"{"object":"list","data":[{"id":"mimo-v2.5","object":"model","owned_by":"xiaomi"}]}"#,
/// )
/// .unwrap();
/// let ids: Vec<&str> = list.data.iter().map(|m| m.id.as_str()).collect();
/// assert_eq!(ids, ["mimo-v2.5"]);
/// ```
#[derive(Debug, Deserialize)]
pub struct ModelList {
    /// 可用模型条目（网关缺 `data` 字段时按空清单处理）
    #[serde(default)]
    pub data: Vec<ModelInfo>,
}

/// 单个模型条目。
#[derive(Debug, Deserialize)]
pub struct ModelInfo {
    /// 模型 ID，请求体 `model` 字段可直接引用
    pub id: String,
}
