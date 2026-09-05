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
//! - `ui.rs`（本文件）：共享句柄 [`Ctx`]、入口 [`run`]（构建窗口 + 按页接线 + 事件循环）、
//!   对话核心接线 `wire_chat`/`wire_timer` 与懒建客户端 `ensure_client`
//! - `ui/host.rs`：多会话簿记 + 会话栏接线 `wire_sidebar`（侧栏/回收站）+ `refresh_trash`
//! - `ui/frame.rs`：无边框窗框接线 `wire_frame`——标题栏三键与 winit 原生拖窗
//! - `ui/bubbles.rs`：气泡模型操作（增量合并、思考折叠、历史投影）
//! - `ui/models.rs`：模型下拉状态机 `ModelsState` + 接线 `wire_models`
//! - `ui/prov.rs`：提供商页装配（providers 整包 ↔ 窗口）+ 接线 `wire_prov`
//! - `ui/settings.rs`：设置页保存 + 接线 `wire_settings`

mod bubbles;
mod frame;
mod host;
mod models;
mod prov;
mod settings;

use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::{
    client::{Client, TurnEvent, TurnOptions, TurnPersona},
    providers::Providers,
};
use crate::db::Db;
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

use bubbles::{STATE_PARTIAL, append_part, fold_tail, snapshot_history};
use host::{Host, RunRef, StreamMsg};
use models::ModelsState;

slint::include_modules!();

/// 懒构建的客户端缓存。
///
/// 连接池应跨多次发送复用，故 `Client` 建一次存起来；默认模型名已折入
/// `Client`，不再单独缓存。
type ClientCache = Arc<Mutex<Option<Arc<Client>>>>;

/// 后台回传主线程的消息（Timer 主线程排空）：对话轮事件、模型清单、连通测试与远端模型拉取
enum UiMsg {
    Turn(StreamMsg),
    Models(Result<Vec<String>, String>),
    ProvTest(SharedString),
    ProvFetch(Result<Vec<String>, String>),
}

/// 各页接线共享的句柄束：run() 建一次，wire_* 从中按闭包再 clone。
/// 字段私有但整个 ui 模块树可读（子模块是本模块的后代）。
#[derive(Clone)]
struct Ctx {
    db: Rc<Db>,
    host: Host,
    models: ModelsState,
    runtime: Arc<tokio::runtime::Runtime>,
    tx: mpsc::Sender<UiMsg>,
    cache: ClientCache,
}

/// UI 主入口：构建窗口、组 Ctx、按页接线、运行 Slint 事件循环直到窗口关闭。
///
/// 这是 ui 模块唯一的公开接口，由 `main.rs` 转发调用。
pub fn run() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    // 打开失败不致命：回落内存库（本运行可用、重启即灭），错误打日志
    let db = Rc::new(Db::open("data/owakaka.db").unwrap_or_else(|e| {
        eprintln!("打开 data/owakaka.db 失败，本运行不持久化: {e}");
        Db::open_in_memory().expect("内存库也开不了就没法跑了")
    }));
    // 旧单配置一次性全量迁移（幂等）；失败仅记日志不阻断启动（下次再试）
    if let Err(e) = Providers::migrate_legacy(&db, "config.toml") {
        eprintln!("旧配置迁移失败: {e}");
    }
    let host = Host::new(db.clone());
    window.set_sessions(host.items.clone().into());
    window.set_messages(host.active_model().into());
    window.set_thinking_on(host.thinking_on.get());

    // 主题恢复：kv("ui") {"theme":0|1|2} → 双向链进 Theme 全局
    if let Ok(Some(json)) = db.kv_get("ui")
        && let Some(v) = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|j| j.get("theme").and_then(serde_json::Value::as_i64))
    {
        window.set_ui_theme(v as i32);
    }
    {
        let db = db.clone();
        window.on_theme_changed(move |mode| {
            if let Err(e) = db.kv_set("ui", &format!("{{\"theme\":{mode}}}")) {
                eprintln!("主题持久化失败: {e}");
            }
        });
    }
    host::refresh_trash(&db, &window);

    let models = ModelsState::from_config(db.clone());
    models.apply(&window);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<UiMsg>();
    let cache: ClientCache = Arc::new(Mutex::new(None));

    let ctx = Ctx {
        db,
        host,
        models,
        runtime,
        tx,
        cache,
    };

    // —— 接线：每页一块；会话核心（发送/停止/回流）留在本文件，页面归各子模块 ——
    frame::wire_frame(&window);
    host::wire_sidebar(&window, &ctx);
    wire_chat(&window, &ctx);

    models::wire_models(&window, &ctx);

    settings::wire_settings(&window, &ctx);

    prov::wire_prov(&window, &ctx);

    // —— 复制路径：Slint 无剪贴板 API，经 arboard 写系统剪贴板 ——
    window.on_copy(move |text| {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    });

    // —— 回流路径：Timer 排空事件队列，按 (sid, gen) 路由写回发起会话 ——
    wire_timer(&window, &ctx, rx);

    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}

/// 每次发送现读 DB 组装轮次人设（毫秒级，读失败按无角色处理不阻断发送）：
/// 助手段 = `persona_id` 绑定的会话助手（未绑定/已删/悬空回落默认助手），
/// 用户人设仍全局一份。两段提示词与温度全空 → `None`（请求与旧行为逐位一致）。
fn persona_for_turn(db: &Db, persona_id: i64) -> Option<TurnPersona> {
    let a = db
        .assistant(persona_id)
        .ok()
        .flatten()
        .filter(|a| !a.deleted)
        .or_else(|| db.default_assistant().ok());
    let u = db.active_persona("user").ok().flatten();
    let p = TurnPersona {
        system_prompt: a
            .as_ref()
            .map_or_else(String::new, |x| x.system_prompt.clone()),
        user_persona: u.map_or_else(String::new, |x| x.system_prompt),
        temperature: a.as_ref().and_then(|x| x.temperature),
    };
    (!p.is_empty()).then_some(p)
}

/// 取（必要时懒建）客户端；`Err` 携带可直接展示的文案。
/// 请求组装/取消编排已下沉至 [`Client::spawn_turn`](crate::ai::client::Client::spawn_turn)。
fn ensure_client(cache: &ClientCache, db: &Db) -> Result<Arc<Client>, String> {
    let mut guard = cache.lock().unwrap();
    if guard.is_none() {
        let client = Client::load(db).map_err(|e| format!("读取配置失败: {e}"))?;
        *guard = Some(Arc::new(client));
    }
    Ok(guard.as_ref().unwrap().clone())
}

/// 对话核心接线：发送（贴气泡 → 快照历史 → 后台轮）、停止、思考开关。
fn wire_chat(window: &AppWindow, ctx: &Ctx) {
    let db = ctx.db.clone();
    let host = ctx.host.clone();
    let models = ctx.models.clone();
    let runtime = ctx.runtime.clone();
    let tx = ctx.tx.clone();
    let cache = ctx.cache.clone();

    // —— 发送路径：贴用户气泡 → 快照历史 → 后台请求（方式按配置分发） ——
    {
        let window_weak = window.as_weak();
        let runtime = runtime.clone();
        let tx = tx.clone();
        let cache = cache.clone();
        let host = host.clone();
        let models = models.clone();
        let db = db.clone();
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
                text: text.clone().into(),
                thinking: SharedString::default(),
                tstate: STATE_PARTIAL,
                tauto: true,
            });
            host.persist_user_message(sid, &text);
            let history = snapshot_history(&current);

            // 配置缺失/损坏只影响本轮发送，以内联气泡反馈，不退出也不卡 UI
            let client = match ensure_client(&cache, &db) {
                Ok(c) => c,
                Err(msg) => {
                    append_part(&current, false, &format!("[配置错误] {msg}"));
                    return;
                }
            };
            // 登记轮次（gen_id 递增、记录本轮模型）后投任务；回流事件打 (sid, gen_id)
            // 标路由回原会话
            let active_model = models.active.borrow().clone();
            let gen_id = {
                let mut runs = host.runs.borrow_mut();
                let run = &mut runs[sid];
                run.gen_id += 1;
                run.model = active_model.clone();
                run.gen_id
            };
            window_weak.upgrade().unwrap().set_generating(true);
            let run = RunRef { sid, gen_id };
            let handle = client.spawn_turn(
                &runtime,
                history,
                TurnOptions {
                    // Step C：当前模型声明不支持思考 → 即使开关开着也强制关
                    thinking: host.thinking_on.get() && models.supports_thinking(&active_model),
                    // 模型下拉的激活项；config 缺失时为空串→回落客户端默认模型
                    model: Some(active_model),
                    // 会话绑定助手（-1/悬空回落默认）；user 人设全局注入
                    persona: persona_for_turn(
                        &db,
                        host.persona_ids.borrow().get(sid).copied().unwrap_or(-1),
                    ),
                },
                {
                    let tx = tx.clone();
                    move |event| {
                        let _ = tx.send(UiMsg::Turn(StreamMsg { run, event }));
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
}

/// 回流接线：Timer 排空事件队列，按 (sid, gen) 路由写回发起会话。
fn wire_timer(window: &AppWindow, ctx: &Ctx, rx: mpsc::Receiver<UiMsg>) {
    let window_weak = window.as_weak();
    let timer_host = ctx.host.clone();
    let timer_models = ctx.models.clone();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(30), move || {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        for msg in rx.try_iter() {
            let StreamMsg { run, event } = match msg {
                UiMsg::Turn(s) => s,
                UiMsg::Models(Ok(ids)) => {
                    // 远端清单并入下拉（去重保序），失败重试留待下回打开
                    timer_models.rebuild(Some(&ids));
                    timer_models.apply(&window);
                    continue;
                }
                UiMsg::Models(Err(e)) => {
                    eprintln!("拉取远端模型清单失败: {e}");
                    timer_models.fetched.set(false);
                    continue;
                }
                UiMsg::ProvTest(msg) => {
                    window.set_prov_status(msg);
                    continue;
                }
                UiMsg::ProvFetch(Ok(ids)) => {
                    let have: Vec<SharedString> = window
                        .get_prov_models()
                        .iter()
                        .map(|m| m.id.clone())
                        .collect();
                    let mut rows: Vec<RemoteRow> = ids
                        .into_iter()
                        .map(|id| RemoteRow {
                            added: have.iter().any(|h| h == &id),
                            id: id.into(),
                        })
                        .collect();
                    rows.sort_by(|a, b| a.added.cmp(&b.added).then(a.id.cmp(&b.id)));
                    window.set_remote_models(Rc::new(VecModel::from(rows)).into());
                    window.set_remote_loading(false);
                    continue;
                }
                UiMsg::ProvFetch(Err(e)) => {
                    window.set_remote_loading(false);
                    let brief: String = e.chars().take(160).collect();
                    window.set_prov_status(format!("拉取失败：{brief}").into());
                    continue;
                }
            };
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
                    timer_host.persist_turn(run.sid);
                    if run.sid == timer_host.current.get() {
                        window.set_generating(false);
                    }
                }
                TurnEvent::Error(e) => {
                    // 错误详情截断，防止超长网关响应体撑爆气泡
                    let brief: String = e.chars().take(300).collect();
                    append_part(&model, false, &format!("\n[出错] {brief}"));
                    timer_host.runs.borrow_mut()[run.sid].cancel = None;
                    timer_host.persist_turn(run.sid);
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
