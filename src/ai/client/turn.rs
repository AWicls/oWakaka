//! 一轮对话的生命周期：请求组装（默认模型 + Thinking 扩展字段）、流式转发、
//! 收尾事件合成与 [`TurnHandle`] 取消，全部在 ai 层内完成；
//! UI 只提供历史与语义开关，消费 [`TurnEvent`]。
//!
//! ```
//! use o_wakaka::ai::client::{TurnEvent, TurnOptions};
//!
//! // 仅演示类型形状；真实发起见 Client::spawn_turn
//! let opts = TurnOptions { thinking: false };
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
#[derive(Debug, Clone, Copy)]
pub struct TurnOptions {
    /// 思考模式：开→不发字段跟随网关默认；关→按端点族发送关闭字段
    pub thinking: bool,
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
        let req = Request {
            model: self.model.clone(),
            messages: history,
            extra: thinking_extra(self.api, opts.thinking),
        };
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

/// Thinking 开关 → 请求扩展字段（经 `Request.extra` 平铺透传）。
/// 目前两端点族同用 OpenAI `reasoning.effort` 语义；后续定制厂家字段时，
/// 在本函数按 [`Api`] 族分叉即可。
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
}
