//! UI 壳（Slint）：oWakaka 的对话窗口，UI 侧唯一对外接口为 [`run`]。
//!
//! # 职责边界
//! - 本模块只做三件事：渲染消息气泡、捕获用户发送动作、把后台事件落到 UI 上。
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
//!   ▲                                             │ chat_stream
//!   │ Timer 30ms 排空                             │ delta/Done/Error
//!   └── UiEvent ◀──────── mpsc::channel ◀─────────┘
//! ```
//!
//! 增量经 `std::sync::mpsc` 回传，Slint `Timer` 周期性在主线程排空并刷新
//! 气泡——UI 更新始终发生在主线程，无需跨线程句柄。

use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::{
    client::Client,
    config::Config,
    dto::openai_chat::request::{Message, Request},
};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

/// 后台任务 → UI 的事件流。
enum UiEvent {
    /// 一段流式文本增量
    Delta(String),
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

/// UI 主入口：构建窗口、接线交互、运行 Slint 事件循环直到窗口关闭。
///
/// 这是 ui 模块唯一的公开接口，由 `main.rs` 转发调用。
pub fn run() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let messages: Rc<VecModel<ChatMessage>> = Rc::new(VecModel::default());
    window.set_messages(messages.clone().into());

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<UiEvent>();
    let cache: ClientCache = Arc::new(Mutex::new(None));

    // —— 发送路径：贴用户气泡 → 快照历史 → 后台流式请求 ——
    {
        let window_weak = window.as_weak();
        let messages = messages.clone();
        let runtime = runtime.clone();
        let tx = tx.clone();
        let cache = cache.clone();
        window.on_send(move |text| {
            let text: String = text.to_string();
            if text.trim().is_empty() {
                return;
            }
            messages.push(ChatMessage {
                role: "user".into(),
                text: text.into(),
            });
            let history = snapshot_history(&messages);

            // 配置缺失/损坏只影响本轮发送，以内联气泡反馈，不退出也不卡 UI
            let (client, model) = match ensure_client(&cache) {
                Ok(pair) => pair,
                Err(msg) => {
                    append_assistant(&messages, &format!("[配置错误] {msg}"));
                    return;
                }
            };
            window_weak.upgrade().unwrap().set_busy(true);
            spawn_chat(&runtime, client, model, history, &tx);
        });
    }

    // —— 回流路径：Timer 排空事件队列，增量并入助手气泡 ——
    {
        let window_weak = window.as_weak();
        let messages = messages.clone();
        let append = {
            let messages = messages.clone();
            move |text: String| append_assistant(&messages, &text)
        };
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            for event in rx.try_iter() {
                match event {
                    UiEvent::Delta(t) => append(t),
                    UiEvent::Done => window.set_busy(false),
                    UiEvent::Error(e) => {
                        // 错误详情截断，防止超长网关响应体撑爆气泡
                        let brief: String = e.chars().take(300).collect();
                        append(format!("\n[出错] {brief}"));
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

/// 把一轮流式对话投给 tokio 后台任务：delta 实时回传，收尾发 Done/Error。
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
            .chat_stream(&req, move |t| {
                let _ = tx_delta.send(UiEvent::Delta(t.to_string()));
            })
            .await;
        let _ = tx_final.send(match result {
            Ok(()) => UiEvent::Done,
            Err(e) => UiEvent::Error(e.to_string()),
        });
    });
}

/// 把 `text` 并入尾部助手气泡；若尾气泡不是 assistant 则新开一条。
///
/// 流式增量、错误标注、配置提示共用此函数：发送刚结束时尾气泡必为 user，
/// 首个增量会自然新开 assistant 气泡，后续增量则原地追加。
fn append_assistant(messages: &Rc<VecModel<ChatMessage>>, text: &str) {
    let last = messages.row_count().saturating_sub(1);
    if let Some(mut row) = messages.row_data(last)
        && row.role == "assistant"
    {
        row.text = SharedString::from(format!("{}{text}", row.text));
        messages.set_row_data(last, row);
    } else {
        messages.push(ChatMessage {
            role: "assistant".into(),
            text: text.into(),
        });
    }
}
