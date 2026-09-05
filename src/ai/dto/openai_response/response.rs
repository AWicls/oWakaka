//! OpenAI Responses API 非流式响应线格式。
//!
//! 只显式声明对话用到的 `output` 条目数组；`id`/`status`/`usage` 等
//! 其余字段自动收集进 `extra`，不做二次解析。

use serde::{Deserialize, Serialize};

/// `POST /responses` 响应体。
///
/// ```
/// use o_wakaka::ai::dto::openai_response::response::Response;
///
/// let raw = r#"{"id":"resp_1","status":"completed","output":[
///   {"type":"reasoning","summary":[{"type":"summary_text","text":"先算1+1"}]},
///   {"type":"message","role":"assistant","content":[
///     {"type":"output_text","text":"2","annotations":[]}]}]}"#;
/// let resp: Response = serde_json::from_str(raw).unwrap();
/// assert_eq!(resp.assistant_text(), "2");
/// assert_eq!(resp.reasoning_text(), "先算1+1");
/// assert_eq!(resp.extra.unwrap()["status"], "completed");
///
/// // 无 reasoning 条目（非推理模型）时思考文本为空串
/// let plain: Response = serde_json::from_str(
///     r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"hi"}]}]}"#,
/// )
/// .unwrap();
/// assert!(plain.reasoning_text().is_empty());
/// assert_eq!(plain.assistant_text(), "hi");
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    /// 输出条目列表（`reasoning`/`message`/`function_call` 等混排）。
    /// `default` 容忍失败场景返回的空数组
    #[serde(default)]
    pub output: Vec<Item>,
    /// 未声明字段的平铺收集（id、status、usage 等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

impl Response {
    /// 按序拼接所有 `message` 条目的正文文本。
    ///
    /// 部分网关把输出部件标为 `text` 而非规范的 `output_text`，两者都收。
    pub fn assistant_text(&self) -> String {
        self.output
            .iter()
            .filter(|i| i.kind == "message")
            .flat_map(|i| i.content.iter())
            .filter(|p| matches!(p.kind.as_str(), "output_text" | "text"))
            .filter_map(|p| p.text.as_deref())
            .collect::<String>()
    }

    /// 按序拼接所有 `reasoning` 条目的摘要文本。
    ///
    /// 线上格式只暴露摘要（原始思维链不返回）；不支持摘要的模型得空串。
    pub fn reasoning_text(&self) -> String {
        self.output
            .iter()
            .filter(|i| i.kind == "reasoning")
            .flat_map(|i| i.summary.iter())
            .filter_map(|p| p.text.as_deref())
            .collect::<String>()
    }
}

/// 单个输出条目。
#[derive(Debug, Serialize, Deserialize)]
pub struct Item {
    /// 条目类型：`message` / `reasoning` / `function_call` 等
    #[serde(rename = "type", default)]
    pub kind: String,
    /// 角色，仅消息条目携带
    #[serde(default)]
    pub role: Option<String>,
    /// 消息条目的内容部件
    #[serde(default)]
    pub content: Vec<Part>,
    /// 推理条目的摘要部件
    #[serde(default)]
    pub summary: Vec<Part>,
    /// 未声明字段的平铺收集（id、工具调用参数等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}

/// 内容/摘要部件。
#[derive(Debug, Serialize, Deserialize)]
pub struct Part {
    /// 部件类型：`output_text` / `summary_text` / `refusal` 等
    #[serde(rename = "type", default)]
    pub kind: String,
    /// 文本；refusal 等无文本部件缺此字段
    #[serde(default)]
    pub text: Option<String>,
    /// 未声明字段的平铺收集（annotations 等）
    #[serde(flatten)]
    pub extra: Option<serde_json::Value>,
}
