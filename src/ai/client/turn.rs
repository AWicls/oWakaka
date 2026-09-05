//! 一轮对话的生命周期：请求组装（默认模型 + Thinking 扩展字段）、流式转发、
//! 收尾事件合成与 [`TurnHandle`] 取消，全部在 ai 层内完成；
//! UI 只提供历史与语义开关，消费 [`TurnEvent`]。
//!
//! ```
//! use o_wakaka::ai::client::{TurnEvent, TurnOptions};
//!
//! // 仅演示类型形状；真实发起见 Client::spawn_turn
//! let opts = TurnOptions {
//!     thinking: false,
//!     model: None,
//!     persona: None,
//! };
//! let ev = TurnEvent::Content("你好".into());
//! matches!(ev, TurnEvent::Content(t) if t == "你好");
//! ```

use std::sync::Arc;

use tokio::sync::oneshot;

use super::{Client, StreamEvent};
use crate::ai::{
    config::Api,
    dto::openai_chat::request::{Message, Request},
};

/// 一轮对话内的事件：内容增量两类 + 收尾一类，收尾恰好一次。
#[derive(Debug)]
pub enum TurnEvent {
    /// 思考过程文本增量
    Reasoning(String),
    /// 回答正文文本增量
    Content(String),
    /// 本轮干净结束（用户经 [`TurnHandle::cancel`] 停止也折算成 Done）
    Done,
    /// 本轮失败，携带可直接展示的简述（截断/呈现策略归调用方）
    Error(String),
}

/// 一轮对话的语义参数：UI 侧开关进这里，线格式映射归 ai 层。
#[derive(Debug, Clone)]
pub struct TurnOptions {
    /// 思考模式：开→不发字段跟随网关默认；关→按端点族发送关闭字段
    pub thinking: bool,
    /// 本轮使用的模型；`None` 用客户端配置的默认模型。UI 切换模型后随发送传入
    pub model: Option<String>,
    /// 角色设定（DB-3）：`Some` 时把系统提示词作首条 system 消息、温度注入请求体；
    /// `None` 或全空 = 行为与旧一致（无 system 前缀、不发 temperature）
    pub persona: Option<TurnPersona>,
}

/// 轮次角色设定投影（[`Persona`](crate::db::Persona) 活跃两行的可发送合并视图，UI 侧读取）。
#[derive(Debug, Clone, Default)]
pub struct TurnPersona {
    /// 助手系统提示词（system 消息主体；空 = 不注入该段）
    pub system_prompt: String,
    /// 用户人设描述（非空则并入 system 消息附段；空 = 不注入该段）
    pub user_persona: String,
    /// 温度（取助手设定）；`None` = 未设定，不发字段
    pub temperature: Option<f64>,
}

impl TurnPersona {
    /// 全空判定（UI 侧：空则整体按无角色处理）
    pub fn is_empty(&self) -> bool {
        self.system_prompt.trim().is_empty()
            && self.user_persona.trim().is_empty()
            && self.temperature.is_none()
    }
}

/// 已投出对话轮次的控制柄：消费即请求停止本轮生成。
pub struct TurnHandle {
    cancel: oneshot::Sender<()>,
}

impl TurnHandle {
    /// 请求停止本轮：任务内 `select!` 丢弃请求 future、连接即断；
    /// 任务仍会以 [`TurnEvent::Done`] 收尾一次，调用方无需区分取消与正常结束。
    pub fn cancel(self) {
        let _ = self.cancel.send(());
    }
}

impl Client {
    /// 在 `rt` 上投一个后台任务跑一轮对话：内容增量与收尾事件按序交给
    /// `on_event`（任务线程回调，实现须自行转投 UI 通道，不得触碰 Slint）。
    ///
    /// `on_event` 需 `Clone`：增量转发闭包与收尾各持一份。多轮可并行、
    /// 互不干扰；返回值 [`TurnHandle`] 取消其中任意一轮。
    pub fn spawn_turn(
        self: &Arc<Client>,
        rt: &tokio::runtime::Runtime,
        history: Vec<Message>,
        opts: TurnOptions,
        mut on_event: impl FnMut(TurnEvent) + Send + Clone + 'static,
    ) -> TurnHandle {
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let req = build_request(&self.model, self.api, &opts, history);
        let client = self.clone();
        let mut deltas = on_event.clone();
        rt.spawn(async move {
            let result = tokio::select! {
                r = client.generate(&req, move |ev| {
                    deltas(match ev {
                        StreamEvent::Reasoning(t) => TurnEvent::Reasoning(t),
                        StreamEvent::Content(t) => TurnEvent::Content(t),
                    });
                }) => r,
                // 停止按钮触发取消、或句柄随进程释放：均按干净收尾处理
                _ = &mut cancel_rx => Ok(()),
            };
            on_event(match result {
                Ok(()) => TurnEvent::Done,
                Err(e) => TurnEvent::Error(e.to_string()),
            });
        });
        TurnHandle { cancel: cancel_tx }
    }
}

/// 轮次请求组装：模型取 [`TurnOptions::model`]（非空覆盖）否则客户端配置的默认模型；
/// 角色设定折进请求（[`persona_system`] + `temperature` 并入 `extra` 平铺透传）。
fn build_request(
    default_model: &str,
    api: Api,
    opts: &TurnOptions,
    mut history: Vec<Message>,
) -> Request {
    let mut extra = thinking_extra(api, opts.thinking);
    if let Some(p) = &opts.persona {
        if let Some(system) = persona_system(p) {
            history.insert(
                0,
                Message {
                    role: "system".into(),
                    content: Some(system),
                    extra: None,
                },
            );
        }
        if let Some(t) = p.temperature {
            let mut obj = extra.unwrap_or_else(|| serde_json::json!({}));
            obj["temperature"] = serde_json::Value::from(t);
            extra = Some(obj);
        }
    }
    Request {
        model: opts
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| default_model.to_string()),
        messages: history,
        extra,
    }
}

/// 组装首条 system 消息：助手提示词为主体，用户人设作附段；两段全空 = `None` 不发。
fn persona_system(p: &TurnPersona) -> Option<String> {
    let (a, u) = (p.system_prompt.trim(), p.user_persona.trim());
    match (a.is_empty(), u.is_empty()) {
        (true, true) => None,
        (false, true) => Some(a.to_string()),
        (true, false) => Some(format!("【用户人设】{u}")),
        (false, false) => Some(format!("{a}\n\n【用户人设】{u}")),
    }
}

/// Thinking 开关 → 请求扩展字段（经 `Request.extra` 平铺透传）。
/// 目前两端点族同用 OpenAI `reasoning.effort` 语义；厂家差异字段
/// （如 MiMo 的思考开启档位）由 `provider::Customization::decorate_body` 在发送前定制。
fn thinking_extra(api: Api, on: bool) -> Option<serde_json::Value> {
    if on {
        return None;
    }
    match api {
        Api::Chat | Api::Responses => {
            Some(serde_json::json!({ "reasoning": { "effort": "none" } }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_extra_only_off_emits_field() {
        assert!(thinking_extra(Api::Chat, true).is_none());
        assert!(thinking_extra(Api::Responses, true).is_none());
        let off = thinking_extra(Api::Responses, false).expect("关闭思考应有扩展字段");
        assert_eq!(off["reasoning"]["effort"], "none");
    }

    /// 模型覆盖：非空 opts.model 优先；None/空串回落配置默认
    #[test]
    fn build_request_model_override_falls_back() {
        let base =
            |opts: TurnOptions| build_request("cfg-model", Api::Responses, &opts, vec![]).model;
        assert_eq!(
            base(TurnOptions {
                thinking: true,
                model: None,
                persona: None
            }),
            "cfg-model"
        );
        assert_eq!(
            base(TurnOptions {
                thinking: true,
                model: Some("mimo-v2.5".into()),
                persona: None
            }),
            "mimo-v2.5"
        );
        assert_eq!(
            base(TurnOptions {
                thinking: true,
                model: Some(String::new()),
                persona: None
            }),
            "cfg-model"
        );
    }

    fn persona(system: &str, user: &str, temp: Option<f64>) -> Option<TurnPersona> {
        let p = TurnPersona {
            system_prompt: system.into(),
            user_persona: user.into(),
            temperature: temp,
        };
        (!p.is_empty()).then_some(p)
    }

    fn one_history() -> Vec<Message> {
        vec![Message {
            role: "user".into(),
            content: Some("hi".into()),
            extra: None,
        }]
    }

    #[test]
    fn persona_prepends_system_and_sets_temperature() {
        let opts = TurnOptions {
            thinking: true,
            model: None,
            persona: persona("你言简意赅", "资深 Rust 工程师", Some(0.2)),
        };
        let req = build_request("def", Api::Chat, &opts, one_history());
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(
            req.messages[0].content.as_deref(),
            Some("你言简意赅\n\n【用户人设】资深 Rust 工程师")
        );
        assert_eq!(req.messages.len(), 2);
        assert_eq!(
            req.extra.unwrap()["temperature"],
            serde_json::Value::from(0.2)
        );
    }

    #[test]
    fn temperature_merges_into_existing_reasoning_extra() {
        let opts = TurnOptions {
            thinking: false,
            model: None,
            persona: persona("", "", Some(1.0)),
        };
        let req = build_request("def", Api::Chat, &opts, one_history());
        let extra = req.extra.unwrap();
        assert_eq!(extra["temperature"], serde_json::Value::from(1.0));
        assert!(extra.get("reasoning").is_some(), "reasoning 键不被顶掉");
        // 人设两段全空则不掺 system 消息
        assert_eq!(req.messages.len(), 1);
    }

    #[test]
    fn no_persona_is_bit_for_bit_old_behavior() {
        let opts = TurnOptions {
            thinking: true,
            model: None,
            persona: None,
        };
        let req = build_request("def", Api::Chat, &opts, one_history());
        assert_eq!(req.messages.len(), 1);
        assert!(req.extra.is_none());
    }
}
