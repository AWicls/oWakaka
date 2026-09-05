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
//! 主线程  用户输入 ──▶ on_send ──▶ spawn_chat ──▶ tokio 后台任务
//!   ▲                                             │ generate（按配置分发接口族/流式）
//!   │ Timer 30ms 排空                             │ Reasoning/Content/Done/Error
//!   └── UiEvent ◀──────── mpsc::channel ◀─────────┘
//! ```
//!
//! 增量经 `std::sync::mpsc` 回传，Slint `Timer` 周期性在主线程排空并刷新
//! 气泡——UI 更新始终发生在主线程，无需跨线程句柄。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::{
    client::{Client, StreamEvent},
    config::Config,
    dto::openai_chat::request::{Message, Request},
};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

/// 后台任务 → UI 的事件流（与 `StreamEvent` 一一对应，外加收尾信号）。
enum UiEvent {
    /// 思考过程文本增量
    Reasoning(String),
    /// 回答正文文本增量
    Content(String),
    /// 本轮回复正常结束
    Done,
    /// 本轮回复失败，携带可展示的简述文本
    Error(String),
}

/// 懒构建的 (client, 默认模型名) 缓存。
///
/// 连接池应跨多次发送复用，故 `Client` 建一次存起来；`model` 与它同源（来自
/// 同一份 config），一并缓存避免二次读文件。
type ClientCache = Arc<Mutex<Option<(Arc<Client>, String)>>>;

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

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<UiEvent>();
    let cache: ClientCache = Arc::new(Mutex::new(None));

    // —— 会话栏：新建/切换只换"当前可见"模型；流式增量始终写 pending ——
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
            let (client, model) = match ensure_client(&cache) {
                Ok(pair) => pair,
                Err(msg) => {
                    append_part(&current, false, &format!("[配置错误] {msg}"));
                    return;
                }
            };
            // 登记接收增量的模型后即与"可见会话"解耦：流式中切走也写回原会话
            *host.pending.borrow_mut() = current;
            window_weak.upgrade().unwrap().set_busy(true);
            spawn_chat(&runtime, client, model, history, &tx);
        });
    }

    // —— 复制路径：Slint 无剪贴板 API，经 arboard 写系统剪贴板 ——
    window.on_copy(move |text| {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    });

    // —— 回流路径：Timer 排空事件队列，增量并入 pending 会话的助手气泡 ——
    {
        let window_weak = window.as_weak();
        let push_host = host.clone();
        let fold_host = host.clone();
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            for event in rx.try_iter() {
                match event {
                    UiEvent::Reasoning(t) => append_part(&push_host.pending.borrow(), true, &t),
                    UiEvent::Content(t) => append_part(&push_host.pending.borrow(), false, &t),
                    UiEvent::Done => {
                        fold_tail(&fold_host.pending.borrow()); // 兜底：纯思考回复也要收起
                        window.set_busy(false);
                    }
                    UiEvent::Error(e) => {
                        // 错误详情截断，防止超长网关响应体撑爆气泡
                        let brief: String = e.chars().take(300).collect();
                        append_part(
                            &push_host.pending.borrow(),
                            false,
                            &format!("\n[出错] {brief}"),
                        );
                        window.set_busy(false);
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
/// 会话切换只改「可见」（窗口的 messages 属性），而流式增量写「pending」——
/// 二者分离，才能实现"生成中允许切换会话、进度仍回原会话"。
#[derive(Clone)]
struct Host {
    /// 侧栏数据源（标题 + 选中态），直接交给 Slint 模型
    items: Rc<VecModel<SessionItem>>,
    /// 每会话一个消息模型，与 `items` 同下标
    models: Rc<RefCell<Vec<Rc<VecModel<ChatMessage>>>>>,
    /// 当前可见会话下标
    current: Rc<Cell<usize>>,
    /// 正在接收流式增量的消息模型（发送时登记，Done/Error 后即成历史）
    pending: Rc<RefCell<Rc<VecModel<ChatMessage>>>>,
}

impl Host {
    /// 初始即带一个空的「新对话」会话。
    fn new() -> Self {
        let first = Rc::new(VecModel::<ChatMessage>::default());
        Self {
            items: Rc::new(VecModel::from(vec![SessionItem {
                title: NEW_TITLE.into(),
                active: true,
            }])),
            models: Rc::new(RefCell::new(vec![first.clone()])),
            current: Rc::new(Cell::new(0)),
            pending: Rc::new(RefCell::new(first)),
        }
    }

    fn active_model(&self) -> Rc<VecModel<ChatMessage>> {
        self.models.borrow()[self.current.get()].clone()
    }

    /// 新建会话并切为可见；进行中的流式增量仍写回原会话（pending 在发送时已定）。
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
        self.items.push(SessionItem {
            title: NEW_TITLE.into(),
            active: true,
        });
        self.current.set(self.models.borrow().len() - 1);
        *self.pending.borrow_mut() = model.clone();
        window.set_messages(model.into());
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

/// 取（必要时懒建）客户端与其默认模型；`Err` 携带可直接展示的文案。
fn ensure_client(cache: &ClientCache) -> Result<(Arc<Client>, String), String> {
    let mut guard = cache.lock().unwrap();
    if guard.is_none() {
        let cfg = Config::load().map_err(|e| format!("读取 config.toml 失败: {e}"))?;
        let model = cfg.model.clone();
        *guard = Some((Arc::new(Client::from_config(&cfg)), model));
    }
    let (client, model) = guard.as_ref().unwrap();
    Ok((client.clone(), model.clone()))
}

/// 把一轮对话投给 tokio 后台任务：端点族与流式方式由 `client.generate`
/// 按配置分发，增量实时回传，收尾发 Done/Error。
///
/// 任务与 UI 之间只有 `tx` 单向通道，不持有任何 Slint 句柄。
fn spawn_chat(
    runtime: &Arc<tokio::runtime::Runtime>,
    client: Arc<Client>,
    model: String,
    history: Vec<Message>,
    tx: &mpsc::Sender<UiEvent>,
) {
    let tx_delta = tx.clone();
    let tx_final = tx.clone();
    runtime.spawn(async move {
        let req = Request {
            model,
            messages: history,
            extra: None,
        };
        let result = client
            .generate(&req, move |ev| {
                let ui = match ev {
                    StreamEvent::Reasoning(t) => UiEvent::Reasoning(t),
                    StreamEvent::Content(t) => UiEvent::Content(t),
                };
                let _ = tx_delta.send(ui);
            })
            .await;
        let _ = tx_final.send(match result {
            Ok(()) => UiEvent::Done,
            Err(e) => UiEvent::Error(e.to_string()),
        });
    });
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
