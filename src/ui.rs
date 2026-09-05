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
//!
//! # 文件结构
//! - `ui.rs`（本文件）：入口 [`run`]——构建窗口、接线回调、Timer 事件路由
//! - `ui/host.rs`：多会话簿记（侧栏、可见会话、生成轮次与取消句柄）
//! - `ui/bubbles.rs`：气泡模型操作（增量合并、思考折叠、历史投影）

mod bubbles;
mod host;

use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::client::{Client, TurnEvent, TurnOptions};
use slint::{SharedString, Timer, TimerMode};

use bubbles::{STATE_PARTIAL, append_part, fold_tail, snapshot_history};
use host::{Host, RunRef, StreamMsg};

slint::include_modules!();

/// 懒构建的客户端缓存。
///
/// 连接池应跨多次发送复用，故 `Client` 建一次存起来；默认模型名已折入
/// `Client`，不再单独缓存。
type ClientCache = Arc<Mutex<Option<Arc<Client>>>>;

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
