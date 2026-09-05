//! UI 壳（Slint）：oWakaka 的对话窗口，UI 侧唯一对外接口为 [`run`]。
//!
//! # 职责边界
//! - 本模块只做：渲染消息气泡与会话栏、捕获用户发送/切换会话动作、把后台事件落到 UI 上。
//!   通信与配置逻辑一律走库目标 `ai` 模块，UI 不碰网络与文件细节。
//! - `src/main.rs` 仅转发调用 [`run`]，不含任何逻辑（bin 薄入口约定）。
//! - Slint 组件定义在 `ui/app.slint`，经 build.rs 编译后由
//!   [`slint::include_modules!`] 生成同名 Rust 类型（`AppWindow`、`ChatMessage`）。
//!
//! # 线程模型
//! Slint 要求组件只在主线程触碰，而网络是异步的，因此：
//!
//! ```text
//! 主线程  用户输入 ──▶ on_send ──▶ Client::spawn_turn ─▶ tokio 后台任务（每轮一个，可并行）
//!   ▲                                          │ generate（端点族/流式分发）
//!   │ Timer 30ms 排空                          │ TurnEvent 全序，收尾恰一次
//!   └── StreamMsg{run, TurnEvent} ◀─ mpsc ◀─────┘  （TurnHandle::cancel 取消）
//! ```
//!
//! 一轮对话的请求组装与取消编排都归 `ai::client::turn`；UI 只把 [`TurnEvent`]
//! 打上会话标（sid, gen_id）经 `std::sync::mpsc` 回传，Slint `Timer` 周期性在
//! 主线程排空、按 sid 路由写回原会话气泡——UI 更新始终发生在主线程。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::{
    client::{Client, TurnEvent, TurnHandle, TurnOptions},
    dto::openai_chat::request::Message,
};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

/// 「哪个会话的哪一轮生成」定位对：回流事件据此写回原会话、并丢弃过期轮次。
#[derive(Clone, Copy)]
struct RunRef {
    sid: usize,
    gen_id: u64,
}

/// 后台任务回流 UI 的单条事件：`run` 定位轮次，`event` 为透传的领域事件。
/// Timer 仅当 `run.gen_id` 与该会话当前代次一致才应用，从而丢弃"停止后重发"
/// 的旧轮迟到事件。收尾（Done/Error）恰一次。
struct StreamMsg {
    run: RunRef,
    event: TurnEvent,
}

/// 懒构建的客户端缓存。
///
/// 连接池应跨多次发送复用，故 `Client` 建一次存起来；默认模型名已折入
/// `Client`，不再单独缓存。
type ClientCache = Arc<Mutex<Option<Arc<Client>>>>;

/// 思考展示态取值，与 `ui/app.slint` 中 `ChatMessage.tstate` 的注释约定一致
const STATE_COLLAPSED: i32 = 0;
/// 默认态：最多 5 行、按内容自适应高度，思考完成后会被自动收起
const STATE_PARTIAL: i32 = 1;

/// UI 主入口：构建窗口、接线交互、运行 Slint 事件循环直到窗口关闭。
///
/// 这是 ui 模块唯一的公开接口，由 `main.rs` 转发调用。
pub fn run() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let host = Host::new();
    window.set_sessions(host.items.clone().into());
    window.set_messages(host.active_model().into());
    window.set_thinking_on(host.thinking_on.get());

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<StreamMsg>();
    let cache: ClientCache = Arc::new(Mutex::new(None));

    // —— 会话栏：新建/切换只换"当前可见"模型与按钮态；流式增量按 sid 回原会话 ——
    {
        let window_weak = window.as_weak();
        window.on_new_session({
            let host = host.clone();
            move || {
                if let Some(w) = window_weak.upgrade() {
                    host.new_session(&w);
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
    }

    // —— 发送路径：贴用户气泡 → 快照历史 → 后台请求（方式按配置分发） ——
    {
        let window_weak = window.as_weak();
        let runtime = runtime.clone();
        let tx = tx.clone();
        let cache = cache.clone();
        let host = host.clone();
        window.on_send(move |text| {
            let text: String = text.to_string();
            if text.trim().is_empty() {
                return;
            }
            let sid = host.current.get();
            let current = host.active_model();
            host.retitle_active(&text);
            current.push(ChatMessage {
                role: "user".into(),
                text: text.into(),
                thinking: SharedString::default(),
                tstate: STATE_PARTIAL,
                tauto: true,
            });
            let history = snapshot_history(&current);

            // 配置缺失/损坏只影响本轮发送，以内联气泡反馈，不退出也不卡 UI
            let client = match ensure_client(&cache) {
                Ok(c) => c,
                Err(msg) => {
                    append_part(&current, false, &format!("[配置错误] {msg}"));
                    return;
                }
            };
            // 登记轮次（gen_id 递增）后投任务；回流事件打 (sid, gen_id) 标路由回原会话
            let gen_id = {
                let mut runs = host.runs.borrow_mut();
                let run = &mut runs[sid];
                run.gen_id += 1;
                run.gen_id
            };
            window_weak.upgrade().unwrap().set_generating(true);
            let run = RunRef { sid, gen_id };
            let handle = client.spawn_turn(
                &runtime,
                history,
                TurnOptions {
                    thinking: host.thinking_on.get(),
                },
                {
                    let tx = tx.clone();
                    move |event| {
                        let _ = tx.send(StreamMsg { run, event });
                    }
                },
            );
            host.runs.borrow_mut()[sid].cancel = Some(handle);
        });
    }

    // —— 停止当前会话 / 思考模式开关 ——
    {
        let window_weak = window.as_weak();
        window.on_stop({
            let host = host.clone();
            move || {
                let sid = host.current.get();
                if let Some(handle) = host.runs.borrow_mut()[sid].cancel.take() {
                    handle.cancel(); // 任务内请求 future 被丢弃 → 连接即断
                }
                if let Some(w) = window_weak.upgrade() {
                    w.set_generating(false);
                }
            }
        });
        let window_weak = window.as_weak();
        window.on_toggle_thinking({
            let host = host.clone();
            move || {
                host.thinking_on.set(!host.thinking_on.get());
                if let Some(w) = window_weak.upgrade() {
                    let on = host.thinking_on.get();
                    w.set_thinking_on(on);
                }
            }
        });
    }

    // —— 复制路径：Slint 无剪贴板 API，经 arboard 写系统剪贴板 ——
    window.on_copy(move |text| {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    });

    // —— 回流路径：Timer 排空事件队列，按 (sid, gen) 路由写回发起会话 ——
    {
        let window_weak = window.as_weak();
        let timer_host = host.clone();
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            for msg in rx.try_iter() {
                let StreamMsg { run, event } = msg;
                // 该会话已停止/重发（gen_id 前进过）：旧轮迟到事件一律丢弃
                {
                    let runs = timer_host.runs.borrow();
                    let Some(state) = runs.get(run.sid) else {
                        continue;
                    };
                    if state.gen_id != run.gen_id {
                        continue;
                    }
                }
                let model = timer_host.models.borrow()[run.sid].clone();
                match event {
                    TurnEvent::Reasoning(t) => append_part(&model, true, &t),
                    TurnEvent::Content(t) => append_part(&model, false, &t),
                    TurnEvent::Done => {
                        fold_tail(&model); // 兜底：纯思考回复也要收起
                        timer_host.runs.borrow_mut()[run.sid].cancel = None;
                        if run.sid == timer_host.current.get() {
                            window.set_generating(false);
                        }
                    }
                    TurnEvent::Error(e) => {
                        // 错误详情截断，防止超长网关响应体撑爆气泡
                        let brief: String = e.chars().take(300).collect();
                        append_part(&model, false, &format!("\n[出错] {brief}"));
                        timer_host.runs.borrow_mut()[run.sid].cancel = None;
                        if run.sid == timer_host.current.get() {
                            window.set_generating(false);
                        }
                    }
                }
            }
        });
        // 定时器与应用同生命周期，故意泄漏（进程退出即回收）
        std::mem::forget(timer);
    }

    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}

/// 新会话在侧栏的占位标题；该会话首条用户消息发出后被输入摘要替换。
const NEW_TITLE: &str = "新对话";

/// UI 侧多会话状态：各回调共享，全部只在主线程触碰（Rc/RefCell 即够）。
///
/// 「可见」（窗口的 messages 属性）与「生成中轮次」（runs）彻底解耦，流式事件
/// 自带 (sid, gen) 路由：生成中切换/新建不串会话，多会话可并行生成。
#[derive(Clone)]
struct Host {
    /// 侧栏数据源（标题 + 选中态），直接交给 Slint 模型
    items: Rc<VecModel<SessionItem>>,
    /// 每会话一个消息模型，与 `items` 同下标
    models: Rc<RefCell<Vec<Rc<VecModel<ChatMessage>>>>>,
    /// 每会话一个轮次簿记，与 `items` 同下标
    runs: Rc<RefCell<Vec<RunState>>>,
    /// 当前可见会话下标
    current: Rc<Cell<usize>>,
    /// 思考模式开关（发送时读取；进行中的生成不受影响）
    thinking_on: Rc<Cell<bool>>,
}

/// 一次会话的生成簿记：`gen_id` 每次发送递增（旧轮迟到事件据此丢弃）；
/// `cancel` 仅在该会话生成中为 `Some`（取出即 [`TurnHandle::cancel`]）。
struct RunState {
    gen_id: u64,
    cancel: Option<TurnHandle>,
}

impl Host {
    /// 初始即带一个空的「新对话」会话。
    fn new() -> Self {
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

    fn active_model(&self) -> Rc<VecModel<ChatMessage>> {
        self.models.borrow()[self.current.get()].clone()
    }

    /// 当前可见会话是否生成中（决定发送键显示为停止键）。
    fn is_generating(&self) -> bool {
        self.runs.borrow()[self.current.get()].cancel.is_some()
    }

    /// 新建会话并切为可见；进行中的流式增量按 sid 仍写回原会话。
    fn new_session(&self, window: &AppWindow) {
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
    fn select(&self, index: usize, window: &AppWindow) {
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
    fn retitle_active(&self, text: &str) {
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

/// 气泡列表即会话历史：转成请求消息列表（刚输入的 user 消息已含在内）。
///
/// 空文本转 `None`，与响应侧 `content: null` 的线格式约定对齐。
fn snapshot_history(messages: &Rc<VecModel<ChatMessage>>) -> Vec<Message> {
    messages
        .iter()
        .map(|m| Message {
            role: m.role.to_string(),
            content: (!m.text.is_empty()).then(|| m.text.to_string()),
            extra: None,
        })
        .collect()
}

/// 取（必要时懒建）客户端；`Err` 携带可直接展示的文案。
/// 请求组装/取消编排已下沉至 [`Client::spawn_turn`](crate::ai::client::Client::spawn_turn)。
fn ensure_client(cache: &ClientCache) -> Result<Arc<Client>, String> {
    let mut guard = cache.lock().unwrap();
    if guard.is_none() {
        let client = Client::load().map_err(|e| format!("读取 config.toml 失败: {e}"))?;
        *guard = Some(Arc::new(client));
    }
    Ok(guard.as_ref().unwrap().clone())
}

/// 把一段文本增量并入"当前回答"：尾部是 assistant 气泡则合并，否则新开一条。
///
/// `thinking=true` 追加到思考过程，`false` 追加到回答正文；追加正文即视为
/// "思考已完成"，同步触发自动收起（见 [`fold_thinking`]）。
fn append_part(messages: &Rc<VecModel<ChatMessage>>, thinking: bool, text: &str) {
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
fn fold_tail(messages: &Rc<VecModel<ChatMessage>>) {
    let last = messages.row_count().saturating_sub(1);
    if let Some(mut row) = messages.row_data(last)
        && row.role == "assistant"
    {
        fold_thinking(&mut row);
        messages.set_row_data(last, row);
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
