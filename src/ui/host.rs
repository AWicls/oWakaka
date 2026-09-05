//! 多会话簿记：侧栏数据、每会话消息模型与生成轮次、可见/生成状态切换。
//!
//! 「可见」（窗口的 messages 属性）与「生成中轮次」（runs）彻底解耦，流式事件
//! 自带 (sid, gen_id) 路由：生成中切换/新建不串会话，多会话可并行生成。
//! 全部只在主线程触碰（Rc/RefCell 即够），由 `ui::run` 的各回调驱动。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use slint::{Model, SharedString, VecModel};

use crate::ai::client::{TurnEvent, TurnHandle};

use super::{AppWindow, ChatMessage, SessionItem};

/// 「哪个会话的哪一轮生成」定位对：回流事件据此写回原会话、并丢弃过期轮次。
#[derive(Clone, Copy)]
pub(super) struct RunRef {
    pub(super) sid: usize,
    pub(super) gen_id: u64,
}

/// 后台任务回流 UI 的单条事件：`run` 定位轮次，`event` 为透传的领域事件。
/// Timer 仅当 `run.gen_id` 与该会话当前代次一致才应用，从而丢弃"停止后重发"
/// 的旧轮迟到事件。收尾（Done/Error）恰一次。
pub(super) struct StreamMsg {
    pub(super) run: RunRef,
    pub(super) event: TurnEvent,
}

/// 新会话在侧栏的占位标题；该会话首条用户消息发出后被输入摘要替换。
const NEW_TITLE: &str = "新对话";

/// 一次会话的生成簿记：`gen_id` 每次发送递增（旧轮迟到事件据此丢弃）；
/// `cancel` 仅在该会话生成中为 `Some`（取出即 [`TurnHandle::cancel`]）。
pub(super) struct RunState {
    pub(super) gen_id: u64,
    pub(super) cancel: Option<TurnHandle>,
}

/// UI 侧多会话状态：各回调共享（[`Host`] 按引用克隆进闭包）。
#[derive(Clone)]
pub(super) struct Host {
    /// 侧栏数据源（标题 + 选中态），直接交给 Slint 模型
    pub(super) items: Rc<VecModel<SessionItem>>,
    /// 每会话一个消息模型，与 `items` 同下标
    pub(super) models: Rc<RefCell<Vec<Rc<VecModel<ChatMessage>>>>>,
    /// 每会话一个轮次簿记，与 `items` 同下标
    pub(super) runs: Rc<RefCell<Vec<RunState>>>,
    /// 当前可见会话下标
    pub(super) current: Rc<Cell<usize>>,
    /// 思考模式开关（发送时读取；进行中的生成不受影响）
    pub(super) thinking_on: Rc<Cell<bool>>,
}

impl Host {
    /// 初始即带一个空的「新对话」会话。
    pub(super) fn new() -> Self {
        Self {
            items: Rc::new(VecModel::from(vec![SessionItem {
                title: NEW_TITLE.into(),
                active: true,
            }])),
            models: Rc::new(RefCell::new(vec![Rc::new(
                VecModel::<ChatMessage>::default(),
            )])),
            runs: Rc::new(RefCell::new(vec![RunState {
                gen_id: 0,
                cancel: None,
            }])),
            current: Rc::new(Cell::new(0)),
            thinking_on: Rc::new(Cell::new(true)),
        }
    }

    pub(super) fn active_model(&self) -> Rc<VecModel<ChatMessage>> {
        self.models.borrow()[self.current.get()].clone()
    }

    /// 当前可见会话是否生成中（决定发送键显示为停止键）。
    fn is_generating(&self) -> bool {
        self.runs.borrow()[self.current.get()].cancel.is_some()
    }

    /// 新建会话并切为可见；进行中的流式增量按 sid 仍写回原会话。
    pub(super) fn new_session(&self, window: &AppWindow) {
        let cur = self.current.get();
        if let Some(old) = self.items.row_data(cur) {
            self.items.set_row_data(
                cur,
                SessionItem {
                    active: false,
                    ..old
                },
            );
        }
        let model = Rc::new(VecModel::<ChatMessage>::default());
        self.models.borrow_mut().push(model.clone());
        self.runs.borrow_mut().push(RunState {
            gen_id: 0,
            cancel: None,
        });
        self.items.push(SessionItem {
            title: NEW_TITLE.into(),
            active: true,
        });
        self.current.set(self.models.borrow().len() - 1);
        window.set_messages(model.into());
        window.set_generating(false);
    }

    /// 切换可见会话；越界或与当前相同则不动作。
    pub(super) fn select(&self, index: usize, window: &AppWindow) {
        let cur = self.current.get();
        if index == cur || index >= self.models.borrow().len() {
            return;
        }
        if let Some(old) = self.items.row_data(cur) {
            self.items.set_row_data(
                cur,
                SessionItem {
                    active: false,
                    ..old
                },
            );
        }
        if let Some(row) = self.items.row_data(index) {
            self.items.set_row_data(
                index,
                SessionItem {
                    active: true,
                    ..row
                },
            );
        }
        self.current.set(index);
        let model = self.models.borrow()[index].clone();
        window.set_messages(model.into());
        window.set_generating(self.is_generating());
    }

    /// 会话首次发送后，把占位标题换成输入摘要。
    pub(super) fn retitle_active(&self, text: &str) {
        let cur = self.current.get();
        if let Some(row) = self.items.row_data(cur)
            && row.title.as_str() == NEW_TITLE
        {
            self.items.set_row_data(
                cur,
                SessionItem {
                    title: derive_title(text),
                    ..row
                },
            );
        }
    }
}

/// 侧栏标题：压掉换行取前 24 字符，超长补省略号；空白输入回退占位标题。
fn derive_title(text: &str) -> SharedString {
    let flat: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\n' | '\r'))
        .collect();
    if flat.is_empty() {
        return NEW_TITLE.into();
    }
    let title: String = flat.chars().take(24).collect();
    if flat.chars().count() > title.chars().count() {
        format!("{title}…").into()
    } else {
        title.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_title_flattens_and_truncates() {
        assert_eq!(derive_title("你好\n世界").as_str(), "你好世界");
        let long: String = "字".repeat(30);
        assert_eq!(
            derive_title(&long).as_str(),
            format!("{}…", "字".repeat(24))
        );
        assert_eq!(derive_title("   \n  ").as_str(), NEW_TITLE);
    }
}
