//! 多会话簿记：侧栏数据、每会话消息模型与生成轮次、可见/生成状态切换、SQLite 写穿。
//! 兼管会话栏接线 [`wire_sidebar`]（新建/切换/删除/回收站）与 [`refresh_trash`]。
//!
//! 「可见」（窗口的 messages 属性）与「生成中轮次」（runs）彻底解耦，流式事件
//! 自带 (sid, gen_id) 路由：生成中切换/新建不串会话，多会话可并行生成。
//! 全部只在主线程触碰（Rc/RefCell 即够），由 `ui::run` 的各回调驱动。
//! `db_ids` 与各列表平行下标，存放各行在 [`crate::db`] 中的 rowid；落库时机 =
//! 发送即写 user 消息、轮次收尾（Done/Error/停止）写 assistant 终稿，流式增量不落库。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use slint::{ComponentHandle, Model, SharedString, VecModel};

use crate::ai::client::{TurnEvent, TurnHandle};
use crate::db::{Db, LoadedMessage, LoadedSession, MessagePayload};

use super::models::ModelsState;
use super::{AppWindow, ChatMessage, SessionItem, TrashItem};

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
/// `cancel` 仅在该会话生成中为 `Some`（取出即 [`TurnHandle::cancel`]）；
/// `model` 为本轮发送时的激活模型（收尾终稿落库用，防生成中切模型记错）。
pub(super) struct RunState {
    pub(super) gen_id: u64,
    pub(super) cancel: Option<TurnHandle>,
    pub(super) model: String,
}

/// UI 侧多会话状态：各回调共享（[`Host`] 按引用克隆进闭包）。
#[derive(Clone)]
pub(super) struct Host {
    /// SQLite 句柄：主线程同步读写，失败只记日志不阻断 UI
    pub(super) db: Rc<Db>,
    /// 侧栏数据源（标题 + 选中态），直接交给 Slint 模型
    pub(super) items: Rc<VecModel<SessionItem>>,
    /// 每会话一个消息模型，与 `items` 同下标
    pub(super) models: Rc<RefCell<Vec<Rc<VecModel<ChatMessage>>>>>,
    /// 每会话一个轮次簿记，与 `items` 同下标
    pub(super) runs: Rc<RefCell<Vec<RunState>>>,
    /// 每会话的 DB rowid，与 `items` 同下标（落库定位键）
    pub(super) db_ids: Rc<RefCell<Vec<i64>>>,
    /// 每会话绑定的助手（persona 行 id），与 `items` 平行下标；
    /// `-1` = 未绑定（占位行/读取失败），发送与读侧回落默认助手
    pub(super) persona_ids: Rc<RefCell<Vec<i64>>>,
    /// 当前可见会话下标
    pub(super) current: Rc<Cell<usize>>,
    /// 思考模式开关（发送时读取；进行中的生成不受影响）
    pub(super) thinking_on: Rc<Cell<bool>>,
}

impl Host {
    /// 从 DB 重建簿记（重启不丢）；无历史则建一个空「新对话」会话。
    ///
    /// 建会话行失败降级为占位 id `-1`（本会话内存照常、持久化报错仅记日志），
    /// 侧栏永远至少有一个可选会话。
    pub(super) fn new(db: Rc<Db>) -> Self {
        let host = Self {
            db,
            items: Rc::new(VecModel::default()),
            models: Rc::new(RefCell::new(Vec::new())),
            runs: Rc::new(RefCell::new(Vec::new())),
            db_ids: Rc::new(RefCell::new(Vec::new())),
            persona_ids: Rc::new(RefCell::new(Vec::new())),
            current: Rc::new(Cell::new(0)),
            thinking_on: Rc::new(Cell::new(true)),
        };
        let loaded = host.db.load_all().unwrap_or_else(|e| {
            eprintln!("读取历史会话失败（本运行从空侧栏开始）: {e}");
            Vec::new()
        });
        if loaded.is_empty() {
            let (id, pid, bubbles) = host.create_session_row(NEW_TITLE);
            host.adopt(id, pid, NEW_TITLE, bubbles);
        } else {
            for s in loaded {
                let bubbles = s.messages.into_iter().map(bubble_from).collect();
                host.adopt(s.id, s.persona_id.unwrap_or(-1), &s.title, bubbles);
            }
            // 重启后默认可见 = 最近活动会话（load_all 按活跃升序，即末条）
            let last = host.items.row_count() - 1;
            if let Some(row) = host.items.row_data(last) {
                host.items.set_row_data(
                    last,
                    SessionItem {
                        active: true,
                        ..row
                    },
                );
            }
            host.current.set(last);
        }
        host
    }

    /// 插入新会话行并绑定当前默认助手；开场白非空即持久化为第一条 assistant 消息
    /// （计入后续请求历史）。返回 `(会话 id, 助手 id, 初始气泡)`；失败记日志回 `-1` 占位。
    fn create_session_row(&self, title: &str) -> (i64, i64, Vec<ChatMessage>) {
        let ast = self.db.default_assistant().ok();
        let pid = ast.as_ref().map_or(-1, |a| a.id);
        let sid = self
            .db
            .insert_session(title, (pid > 0).then_some(pid))
            .unwrap_or_else(|e| {
                eprintln!("创建会话失败（不持久）: {e}");
                -1
            });
        let mut bubbles = Vec::new();
        if let Some(a) = ast.filter(|a| !a.opening.trim().is_empty()) {
            if sid >= 0
                && let Err(e) = self
                    .db
                    .insert_message(sid, "assistant", a.opening.trim(), "", None)
            {
                eprintln!("开场白落库失败: {e}");
            }
            bubbles.push(ChatMessage {
                role: "assistant".into(),
                text: a.opening.trim().into(),
                thinking: SharedString::default(),
                tstate: 0,
                tauto: true,
            });
        }
        (sid, pid, bubbles)
    }

    /// 当前会话绑定助手 → 聊天卡头栏四属性（未绑定/悬空/已删回落默认助手）。
    pub(super) fn sync_assistant_header(&self, window: &AppWindow) {
        let pid = self
            .persona_ids
            .borrow()
            .get(self.current.get())
            .copied()
            .unwrap_or(-1);
        let Some(a) = self
            .db
            .assistant(pid)
            .ok()
            .flatten()
            .filter(|a| !a.deleted)
            .or_else(|| self.db.default_assistant().ok())
        else {
            return;
        };
        window.set_chat_ast_name(a.name.as_str().into());
        window.set_chat_ast_initial(super::settings::initial_of(&a.name).as_str().into());
        window.set_chat_ast_color(super::settings::avatar_color(a.id));
        window.set_chat_ast_img(super::settings::load_avatar(&a.avatar));
    }

    /// 当前会话绑定助手若指定默认模型 → 切换激活模型下拉（仅新建会话时调用；
    /// 未绑定/未指定/已删助手不动，保持用户当前选择）。
    pub(super) fn apply_assistant_model(&self, models: &ModelsState, window: &AppWindow) {
        let pid = self
            .persona_ids
            .borrow()
            .get(self.current.get())
            .copied()
            .unwrap_or(-1);
        let Some(a) = self.db.assistant(pid).ok().flatten().filter(|a| !a.deleted) else {
            return;
        };
        if !a.model.is_empty() {
            models.pick(window, a.model);
        }
    }

    /// 一条会话行并列进五个簿记列表；仅首条自动选中（重启可见性在 new 里另定）。
    fn adopt(&self, db_id: i64, persona_id: i64, title: &str, bubbles: Vec<ChatMessage>) {
        let active = self.db_ids.borrow().is_empty();
        self.items.push(SessionItem {
            title: title.into(),
            active,
        });
        self.models
            .borrow_mut()
            .push(Rc::new(VecModel::from(bubbles)));
        self.runs.borrow_mut().push(RunState {
            gen_id: 0,
            cancel: None,
            model: String::new(),
        });
        self.db_ids.borrow_mut().push(db_id);
        self.persona_ids.borrow_mut().push(persona_id);
    }

    pub(super) fn active_model(&self) -> Rc<VecModel<ChatMessage>> {
        self.models.borrow()[self.current.get()].clone()
    }

    /// 当前可见会话是否生成中（决定发送键显示为停止键）。
    fn is_generating(&self) -> bool {
        self.runs.borrow()[self.current.get()].cancel.is_some()
    }

    /// 新建会话（DB 先行，内存跟随）并切为可见；进行中的流式增量按 sid 仍写回原会话。
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
        let (db_id, pid, bubbles) = self.create_session_row(NEW_TITLE);
        let model = Rc::new(VecModel::from(bubbles));
        self.models.borrow_mut().push(model.clone());
        self.runs.borrow_mut().push(RunState {
            gen_id: 0,
            cancel: None,
            model: String::new(),
        });
        self.db_ids.borrow_mut().push(db_id);
        self.persona_ids.borrow_mut().push(pid);
        self.items.push(SessionItem {
            title: NEW_TITLE.into(),
            active: true,
        });
        self.current.set(self.models.borrow().len() - 1);
        window.set_messages(model.into());
        window.set_generating(false);
        self.sync_assistant_header(window);
    }

    /// 删除可见会话（逻辑删 → 回收站）：取消其在途生成、整体 bump 删除点及之后的
    /// 轮次代次使旧事件作废（下方标量前移一位，事件若继续按旧 sid 写入会串会话）。
    /// 删空则补建一个「新对话」，侧栏恒 ≥1。
    pub(super) fn delete_session(&self, idx: usize, window: &AppWindow) {
        let len = self.models.borrow().len();
        if idx >= len {
            return;
        }
        if let Some(handle) = self.runs.borrow_mut()[idx].cancel.take() {
            handle.cancel();
        }
        {
            let mut runs = self.runs.borrow_mut();
            for run in runs.iter_mut().skip(idx) {
                run.gen_id += 1;
            }
        }
        if let Some(&db_id) = self.db_ids.borrow().get(idx)
            && db_id >= 0
            && let Err(e) = self.db.soft_delete_session(db_id)
        {
            eprintln!("会话删除落库失败: {e}");
        }
        self.items.remove(idx);
        self.models.borrow_mut().remove(idx);
        self.runs.borrow_mut().remove(idx);
        self.db_ids.borrow_mut().remove(idx);
        self.persona_ids.borrow_mut().remove(idx);
        if self.items.row_count() == 0 {
            let (id, pid, bubbles) = self.create_session_row(NEW_TITLE);
            self.adopt(id, pid, NEW_TITLE, bubbles);
            self.current.set(0);
        } else {
            let cur = self.current.get();
            let next = if idx < cur {
                cur - 1
            } else if idx == cur {
                cur.min(self.items.row_count() - 1)
            } else {
                cur
            };
            self.current.set(next);
            for i in 0..self.items.row_count() {
                if let Some(row) = self.items.row_data(i)
                    && row.active != (i == next)
                {
                    self.items.set_row_data(
                        i,
                        SessionItem {
                            active: i == next,
                            ..row
                        },
                    );
                }
            }
        }
        window.set_messages(self.active_model().into());
        window.set_generating(self.is_generating());
        self.sync_assistant_header(window);
    }

    /// 回收站恢复后把会话（含消息）推入侧栏末尾；不改变当前可见。
    pub(super) fn push_restored(&self, s: LoadedSession) {
        let bubbles = s.messages.into_iter().map(bubble_from).collect();
        self.adopt(s.id, s.persona_id.unwrap_or(-1), &s.title, bubbles);
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
        self.sync_assistant_header(window);
    }

    /// 会话首次发送后，把占位标题换成输入摘要（内存与 DB 同步）。
    pub(super) fn retitle_active(&self, text: &str) {
        let cur = self.current.get();
        if let Some(row) = self.items.row_data(cur)
            && row.title.as_str() == NEW_TITLE
        {
            let title = derive_title(text);
            self.items.set_row_data(
                cur,
                SessionItem {
                    title: title.clone(),
                    ..row
                },
            );
            if let Some(&db_id) = self.db_ids.borrow().get(cur)
                && let Err(e) = self.db.set_session_title(db_id, title.as_str())
            {
                eprintln!("会话标题落库失败: {e}");
            }
        }
    }

    /// 用户消息发送即落库（每轮写库第 1 次；model/payload 对 user 无意义）。
    pub(super) fn persist_user_message(&self, sid: usize, text: &str) {
        if let Some(&db_id) = self.db_ids.borrow().get(sid)
            && let Err(e) = self.db.insert_message(db_id, "user", text, "", None)
        {
            eprintln!("用户消息落库失败: {e}");
        }
    }

    /// 轮次收尾（Done/Error，含停止）把尾部 assistant 气泡整条写成终稿
    /// （每轮写库第 2 次；流式增量不逐 token 落库）。
    pub(super) fn persist_turn(&self, sid: usize) {
        let model = match self.models.borrow().get(sid) {
            Some(m) => m.clone(),
            None => return,
        };
        let Some(row) = model.row_data(model.row_count().saturating_sub(1)) else {
            return;
        };
        if row.role != "assistant" {
            return; // 空轮（无增量即收尾）：不写
        }
        let db_id = match self.db_ids.borrow().get(sid) {
            Some(id) => *id,
            None => return,
        };
        let model_id = match self.runs.borrow().get(sid) {
            Some(run) => run.model.clone(),
            None => String::new(),
        };
        let payload = MessagePayload {
            thinking: row.thinking.to_string(),
            tstate: row.tstate,
            tauto: row.tauto,
        };
        if let Err(e) = self.db.insert_message(
            db_id,
            "assistant",
            row.text.as_str(),
            &model_id,
            Some(&payload),
        ) {
            eprintln!("回复落库失败: {e}");
        }
    }
}

/// DB 消息行 → 启动回填的气泡（展示态原样恢复）。
fn bubble_from(m: LoadedMessage) -> ChatMessage {
    ChatMessage {
        role: m.role.into(),
        text: m.content.into(),
        thinking: m.payload.thinking.into(),
        tstate: m.payload.tstate,
        tauto: m.payload.tauto,
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

/// 回收站列表重注入（侧栏按钮计数与浮层同源）。
pub(super) fn refresh_trash(db: &Db, window: &AppWindow) {
    let rows: Vec<TrashItem> = db
        .list_deleted()
        .unwrap_or_default()
        .into_iter()
        .map(|(id, title)| TrashItem {
            id: id as i32,
            title: title.into(),
        })
        .collect();
    window.set_trash_list(Rc::new(VecModel::from(rows)).into());
}

/// 会话栏接线：新建/切换/删除/回收站（restore/purge）。删除只换「当前可见」模型与按钮态，
/// 流式增量按 sid 回原会话（路由在 ui.rs 的 Timer）。
pub(super) fn wire_sidebar(window: &AppWindow, ctx: &super::Ctx) {
    let db = ctx.db.clone();
    let host = ctx.host.clone();

    let window_weak = window.as_weak();
    window.on_new_session({
        let host = host.clone();
        let models = ctx.models.clone();
        move || {
            if let Some(w) = window_weak.upgrade() {
                host.new_session(&w);
                // 新会话绑定助手指定了默认模型 → 自动切模型下拉（回对话页即可见）
                host.apply_assistant_model(&models, &w);
            }
        }
    });
    let window_weak = window.as_weak();
    window.on_select_session({
        let host = host.clone();
        move |i| {
            if let Some(w) = window_weak.upgrade() {
                host.select(i.max(0) as usize, &w);
            }
        }
    });
    let window_weak = window.as_weak();
    window.on_session_delete({
        let host = host.clone();
        let db = db.clone();
        move |i| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            host.delete_session(i.max(0) as usize, &w);
            refresh_trash(&db, &w);
        }
    });
    let window_weak = window.as_weak();
    window.on_trash_requested({
        let db = db.clone();
        move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            refresh_trash(&db, &w);
            w.set_trash_open(true);
        }
    });
    let window_weak = window.as_weak();
    window.on_trash_closed(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_trash_open(false);
        }
    });
    let window_weak = window.as_weak();
    window.on_trash_restore({
        let host = host.clone();
        let db = db.clone();
        move |id| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let sid = id as i64;
            match db.restore_session(sid).and_then(|()| db.load_session(sid)) {
                Ok(Some(s)) => {
                    host.push_restored(s);
                    refresh_trash(&db, &w);
                }
                Ok(None) => {}
                Err(e) => eprintln!("恢复会话失败: {e}"),
            }
        }
    });
    let window_weak = window.as_weak();
    window.on_trash_purge({
        let db = db.clone();
        move |id| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            if let Err(e) = db.purge_session(id as i64) {
                eprintln!("彻底删除失败: {e}");
            }
            refresh_trash(&db, &w);
        }
    });
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
