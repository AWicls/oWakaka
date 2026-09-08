//! 气泡模型操作：流式增量合并、思考展示态折叠、历史投影。
//!
//! 全部是纯数据变换（对 `VecModel<ChatMessage>` 的读写），不触碰窗口与网络，
//! 因此可在主线程 Timer 里被放心调用。

use std::rc::Rc;

use slint::{Model, SharedString, VecModel};

use crate::ai::dto::openai_chat::request::Message;

use crate::ui::ChatMessage;

/// 思考展示态取值，与 `ui/app.slint` 中 `ChatMessage.tstate` 的注释约定一致
const STATE_COLLAPSED: i32 = 0;
/// 默认态：最多 5 行、按内容自适应高度，思考完成后会被自动收起
pub(in crate::ui) const STATE_PARTIAL: i32 = 1;

/// 气泡列表即会话历史：转成请求消息列表（刚输入的 user 消息已含在内）。
///
/// 空文本转 `None`，与响应侧 `content: null` 的线格式约定对齐。
pub(in crate::ui) fn snapshot_history(messages: &Rc<VecModel<ChatMessage>>) -> Vec<Message> {
    messages
        .iter()
        .map(|m| Message {
            role: m.role.to_string(),
            content: (!m.text.is_empty()).then(|| m.text.to_string()),
            extra: None,
        })
        .collect()
}

/// 把一段文本增量并入"当前回答"：尾部是 assistant 气泡则合并，否则新开一条。
///
/// `thinking=true` 追加到思考过程，`false` 追加到回答正文；追加正文即视为
/// "思考已完成"，同步触发自动收起（见 [`fold_thinking`]）。
pub(in crate::ui) fn append_part(messages: &Rc<VecModel<ChatMessage>>, thinking: bool, text: &str) {
    let last = messages.row_count().saturating_sub(1);
    if let Some(mut row) = messages.row_data(last)
        && row.role == "assistant"
    {
        if thinking {
            row.thinking = SharedString::from(format!("{}{}", row.thinking, text));
        } else {
            row.text = SharedString::from(format!("{}{}", row.text, text));
            fold_thinking(&mut row);
        }
        messages.set_row_data(last, row);
    } else {
        messages.push(ChatMessage {
            role: "assistant".into(),
            text: if thinking {
                SharedString::default()
            } else {
                text.into()
            },
            thinking: if thinking {
                text.into()
            } else {
                SharedString::default()
            },
            tstate: STATE_PARTIAL,
            tauto: true,
        });
    }
}

/// 思考完成时的自动收起：仅"部分且未手动干预"生效；手动切换的状态固定不变。
fn fold_thinking(row: &mut ChatMessage) {
    if row.tauto && row.tstate == STATE_PARTIAL {
        row.tstate = STATE_COLLAPSED;
    }
}

/// 对尾部 assistant 气泡执行 [`fold_thinking`]（Done 兜底：只思考不答的流）。
pub(in crate::ui) fn fold_tail(messages: &Rc<VecModel<ChatMessage>>) {
    let last = messages.row_count().saturating_sub(1);
    if let Some(mut row) = messages.row_data(last)
        && row.role == "assistant"
    {
        fold_thinking(&mut row);
        messages.set_row_data(last, row);
    }
}
