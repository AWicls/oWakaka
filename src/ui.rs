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
//! - `ui.rs`（本文件）：共享句柄 [`Ctx`]、回传类型 [`UiMsg`]/[`ClientCache`] 与入口 [`run`]
//!   （构建窗口 + 按页接线 + 事件循环）
//! - `ui/chat.rs`：对话核心接线 `wire_chat`/`wire_timer`、懒建客户端 `ensure_client`、轮次人设组装
//! - `ui/host.rs`：多会话簿记 + 会话栏接线 `wire_sidebar`（侧栏/回收站）+ `refresh_trash`
//! - `ui/frame.rs`：无边框窗框接线 `wire_frame`——标题栏三键与 winit 原生拖窗
//! - `ui/bubbles.rs`：气泡模型操作（增量合并、思考折叠、历史投影）
//!
//! # 文件结构（按三个界面域分级）
//! - `frame.rs`：无边框窗框——标题栏三键与 winit 原生拖窗（边框域）
//! - `chat.rs` + `chat/`：对话域入口接线（发送/停止/回流）与
//!   `chat/{host,session,bubbles,models}.rs`（簿记/侧栏/气泡/模型下拉）
//! - `settings.rs` + `settings/`：设置域装配（进出/用户人设/头像工具）与
//!   `settings/{prov,ast}.rs`（LLM 提供商页 / AI 助手页）
mod chat;
mod frame;
mod settings;

use std::{
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
};

use crate::ai::{client::Client, providers::Providers};
use crate::db::Db;
use slint::SharedString;

use chat::{Host, ModelsState, StreamMsg};

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
    chat::session::refresh_trash(&db, &window);

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
    ctx.host.sync_assistant_header(&window); // 启动首屏：当前会话助手进头栏
    frame::wire_frame(&window);
    chat::session::wire_sidebar(&window, &ctx);
    chat::wire_chat(&window, &ctx);

    chat::models::wire_models(&window, &ctx);

    settings::wire_settings(&window, &ctx);

    settings::prov::wire_prov(&window, &ctx);

    // —— 复制路径：Slint 无剪贴板 API，经 arboard 写系统剪贴板 ——
    window.on_copy(move |text| {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    });

    // —— 回流路径：Timer 排空事件队列，按 (sid, gen) 路由写回发起会话 ——
    chat::wire_timer(&window, &ctx, rx);

    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}
