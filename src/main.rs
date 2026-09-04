//! 可执行入口：Slint 对话窗口（3:2），默认走流式路径 `chat_stream`。
//!
//! 线程模型：UI 只在主线程刷新；网络任务 spawn 到 tokio Runtime，
//! 增量经 mpsc 通道回传，Slint Timer 以 30ms 轮询排空。
//! 业务逻辑全部位于库目标 `o_wakaka`（src/lib.rs）。

use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use o_wakaka::ai::{
    client::Client,
    config::Config,
    dto::openai_chat::request::{Message, Request},
};
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

/// 后台 → UI 的事件
enum UiEvent {
    Delta(String),
    Done,
    Error(String),
}

/// 懒构建并缓存 (client, 模型名)，跨发送复用连接池
type ClientCache = Arc<Mutex<Option<(Arc<Client>, String)>>>;

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let messages: Rc<VecModel<ChatMessage>> = Rc::new(VecModel::default());
    window.set_messages(messages.clone().into());

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<UiEvent>();
    let cache: ClientCache = Arc::new(Mutex::new(None));

    // 发送：贴用户气泡 → 快照历史 → 后台流式请求
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
            // 气泡列表即会话历史（user/assistant 交替），随请求快照携带
            let history: Vec<Message> = messages
                .iter()
                .map(|m| Message {
                    role: m.role.to_string(),
                    content: (!m.text.is_empty()).then(|| m.text.to_string()),
                    extra: None,
                })
                .collect();

            // 首次发送才读 config.toml 并建客户端；失败以气泡反馈，不阻塞 UI
            let (client, model) = {
                let mut guard = cache.lock().unwrap();
                if guard.is_none() {
                    let cfg = match Config::load() {
                        Ok(cfg) => cfg,
                        Err(e) => {
                            messages.push(ChatMessage {
                                role: "assistant".into(),
                                text: format!("[配置错误] 读取 config.toml 失败: {e}").into(),
                            });
                            return;
                        }
                    };
                    let model = cfg.model.clone();
                    *guard = Some((Arc::new(Client::from_config(&cfg)), model));
                }
                let (client, model) = guard.as_ref().unwrap();
                (client.clone(), model.clone())
            };
            window_weak.upgrade().unwrap().set_busy(true);

            runtime.spawn({
                let tx = tx.clone(); // FnMut 内不能移动捕获，克隆进任务
                async move {
                    let req = Request {
                        model,
                        messages: history,
                        extra: None,
                    };
                    let tx_chunk = tx.clone();
                    let result = client
                        .chat_stream(&req, move |t| {
                            let _ = tx_chunk.send(UiEvent::Delta(t.to_string()));
                        })
                        .await;
                    let _ = tx.send(match result {
                        Ok(()) => UiEvent::Done,
                        Err(e) => UiEvent::Error(e.to_string()),
                    });
                }
            });
        });
    }

    // UI 侧：排空事件队列，增量并入最后一个助手气泡
    {
        let window_weak = window.as_weak();
        let messages = messages.clone();
        // 尾气泡是 assistant 则追加，否则新开一条（Delta 与 Error 共用）
        let append = move |text: String| {
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
        // 定时器与应用同生命周期，故意泄漏
        std::mem::forget(timer);
    }

    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}
