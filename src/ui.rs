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
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::ai::{
    client::{Client, TurnEvent, TurnOptions, TurnPersona},
    config::Config,
    provider::Provider,
};
use crate::db::Db;
use slint::{SharedString, Timer, TimerMode, VecModel};

use bubbles::{STATE_PARTIAL, append_part, fold_tail, snapshot_history};
use host::{Host, RunRef, StreamMsg};

slint::include_modules!();

/// 懒构建的客户端缓存。
///
/// 连接池应跨多次发送复用，故 `Client` 建一次存起来；默认模型名已折入
/// `Client`，不再单独缓存。
type ClientCache = Arc<Mutex<Option<Arc<Client>>>>;

/// 后台回传主线程的消息（Timer 主线程排空）：对话轮事件与模型清单异步结果
enum UiMsg {
    Turn(StreamMsg),
    Models(Result<Vec<String>, String>),
}

/// 模型切换下拉的 UI 状态：数据源、激活 ID、别名表（config `[models]`）；
/// `fetched` 防抖——首次打开下拉才拉远端 `/models`，失败时下回打开可重试
#[derive(Clone)]
struct ModelsState {
    db: Rc<Db>,
    items: Rc<VecModel<ModelItem>>,
    active: Rc<RefCell<String>>,
    aliases: Rc<RefCell<BTreeMap<String, String>>>,
    fetched: Rc<Cell<bool>>,
}

impl ModelsState {
    /// 初始值取自运行配置（model + [models]）；读不到则空激活（发送回落客户端默认）
    fn from_config(db: Rc<Db>) -> Self {
        let (active, aliases) = match Config::load(&db) {
            Ok(cfg) => (cfg.model, cfg.models),
            Err(_) => (String::new(), BTreeMap::new()),
        };
        let state = Self {
            db,
            items: Rc::new(VecModel::default()),
            active: Rc::new(RefCell::new(active)),
            aliases: Rc::new(RefCell::new(aliases)),
            fetched: Rc::new(Cell::new(false)),
        };
        state.rebuild(None);
        state
    }

    /// 模型 ID 的显示名：config 别名优先，未配别名原样显示
    fn display(&self, id: &str) -> SharedString {
        self.aliases
            .borrow()
            .get(id)
            .map_or_else(|| id.into(), |a| a.as_str().into())
    }

    /// 重建下拉清单：{激活} ∪ config 别名键 ∪ 远端结果，去重保序（激活恒首位）
    fn rebuild(&self, remote: Option<&[String]>) {
        let active = self.active.borrow().clone();
        let mut ids: Vec<String> = Vec::new();
        {
            let mut push = |id: &str| {
                if !id.is_empty() && !ids.iter().any(|x| x == id) {
                    ids.push(id.to_string());
                }
            };
            push(&active);
            for id in self.aliases.borrow().keys() {
                push(id);
            }
            if let Some(remote) = remote {
                for id in remote {
                    push(id);
                }
            }
        }
        let rows = ids
            .into_iter()
            .map(|id| ModelItem {
                display: self.display(&id),
                id: id.into(),
                current: false,
            })
            .collect::<Vec<_>>();
        let active_s: SharedString = active.into();
        let rows: Vec<ModelItem> = rows
            .into_iter()
            .map(|mut m| {
                m.current = m.id == active_s;
                m
            })
            .collect();
        self.items.set_vec(rows);
    }

    /// 把状态同步到窗口（下拉数据源 + 模型按钮文案）
    fn apply(&self, window: &AppWindow) {
        window.set_models(self.items.clone().into());
        let active = self.active.borrow().clone();
        let label = if active.is_empty() {
            "未选模型".into()
        } else {
            self.display(&active)
        };
        window.set_model_label(label);
    }

    /// 切换激活模型：本会话即时生效（后续发送携带）并回写设置（DB kv），
    /// 回写失败仅影响重启后持久，打日志不阻断
    fn pick(&self, window: &AppWindow, id: String) {
        if id.is_empty() || id == *self.active.borrow() {
            return;
        }
        *self.active.borrow_mut() = id;
        self.rebuild(None);
        self.apply(window);
        let active = self.active.borrow().clone();
        if let Err(e) = self.persist_model(&active) {
            eprintln!("模型回写设置失败（仅本次会话生效）: {e}");
        }
    }

    /// 把激活模型写回设置（kv 配置 JSON）的 `model` 字段，其余字段原样保留
    fn persist_model(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = Config::load(&self.db)?;
        cfg.model = id.to_string();
        cfg.save(&self.db)
    }
}

/// 设置页保存：设置字段（stream/api/[models] 等原样保留）+ 角色设定
/// （助手提示词/温度、用户人设 → persona 活跃行）；校验不过不落盘；
/// 成功则失效客户端缓存并同步模型下拉（允许重拉远端清单）
// 参数 = Slint saved 回调直传 + 共享状态，拆包反而绕
#[allow(clippy::too_many_arguments)]
fn save_settings(
    p: i32,
    url: &str,
    key: &str,
    model: &str,
    sys_prompt: &str,
    user_persona: &str,
    temp: &str,
    db: &Db,
    cache: &ClientCache,
    models: &ModelsState,
    window: &AppWindow,
) -> SharedString {
    let temperature = match parse_temperature(temp) {
        Ok(v) => v,
        Err(msg) => return msg.into(),
    };
    let mut cfg = match Config::load(db) {
        Ok(cfg) => cfg,
        Err(e) => return format!("读取配置失败: {e}").into(),
    };
    cfg.provider = if p == 1 {
        Provider::XiaomiMimo
    } else {
        Provider::Custom
    };
    cfg.base_url = url.trim().to_string();
    if !key.trim().is_empty() {
        cfg.api_key = key.trim().to_string(); // 空密钥 = 保持已存值
    }
    cfg.model = model.trim().to_string();
    if let Err(e) = cfg.validate() {
        return e.into();
    }
    if let Err(e) = cfg.save(db) {
        return format!("保存失败: {e}").into();
    }
    let personas = db
        .upsert_active_persona("assistant", "默认助手", sys_prompt.trim(), temperature)
        .and_then(|()| db.upsert_active_persona("user", "默认人设", user_persona.trim(), None));
    if let Err(e) = personas {
        return format!("设置已存，角色设定保存失败: {e}").into();
    }
    *cache.lock().unwrap() = None; // 下一次发送按新配置重建客户端
    *models.active.borrow_mut() = cfg.model.clone();
    models.fetched.set(false);
    models.rebuild(None);
    models.apply(window);
    "已保存，下一次发送生效".into()
}

/// UI 主入口：构建窗口、接线交互、运行 Slint 事件循环直到窗口关闭。
///
/// 这是 ui 模块唯一的公开接口，由 `main.rs` 转发调用。
pub fn run() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    // 打开失败不致命：回落内存库（本运行可用、重启即灭），错误打日志
    let db = Rc::new(Db::open("data/owakaka.db").unwrap_or_else(|e| {
        eprintln!("打开 data/owakaka.db 失败，本运行不持久化: {e}");
        Db::open_in_memory().expect("内存库也开不了就没法跑了")
    }));
    let host = Host::new(db.clone());
    window.set_sessions(host.items.clone().into());
    window.set_messages(host.active_model().into());
    window.set_thinking_on(host.thinking_on.get());

    let models = ModelsState::from_config(db.clone());
    models.apply(&window);

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("启动 tokio Runtime 失败"));
    let (tx, rx) = mpsc::channel::<UiMsg>();
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
                    thinking: host.thinking_on.get(),
                    // 模型下拉的激活项；config 缺失时为空串→回落客户端默认模型
                    model: Some(active_model),
                    // 每次发送现读 DB 活跃角色行（毫秒级）；未设定 = None 零变化
                    persona: persona_for_turn(&db),
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

    // —— 模型切换：选择即时生效并回写 config；打开下拉首次异步拉远端 /models ——
    {
        let window_weak = window.as_weak();
        window.on_model_picked({
            let models = models.clone();
            move |id| {
                if let Some(w) = window_weak.upgrade() {
                    models.pick(&w, id.to_string());
                }
            }
        });
        window.on_models_requested({
            let runtime = runtime.clone();
            let cache = cache.clone();
            let tx = tx.clone();
            let models = models.clone();
            let db = db.clone();
            move || {
                if models.fetched.get() {
                    return; // 已成功拉取过，不重复请求
                }
                let Ok(client) = ensure_client(&cache, &db) else {
                    return; // 配置缺失：下拉仍可用 config 清单，不打扰
                };
                models.fetched.set(true);
                let tx = tx.clone();
                runtime.spawn(async move {
                    let result = client.list_models().await.map_err(|e| e.to_string());
                    let _ = tx.send(UiMsg::Models(result));
                });
            }
        });
    }

    // —— 设置整页：进入注入 config 初值；保存=校验落盘 + 失效客户端缓存 + 同步模型态 ——
    {
        window.on_settings_requested({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                // 读不到配置也照常进设置页（字段空白），保存时报错引导
                if let Ok(cfg) = Config::load(&db) {
                    w.set_cfg_provider(if cfg.provider == Provider::XiaomiMimo {
                        1
                    } else {
                        0
                    });
                    w.set_cfg_base_url(cfg.base_url.into());
                    w.set_cfg_api_key(cfg.api_key.into());
                    w.set_cfg_model(cfg.model.into());
                } else {
                    *cache.lock().unwrap() = None;
                }
                // 角色设定初值：persona 活跃行（读不到/未设定 = 空白 + 温度空串）
                if let Ok(Some(a)) = db.active_persona("assistant") {
                    w.set_cfg_system_prompt(a.system_prompt.into());
                    w.set_cfg_temperature(
                        a.temperature
                            .map_or_else(String::new, |t| format!("{t}"))
                            .into(),
                    );
                }
                if let Ok(Some(u)) = db.active_persona("user") {
                    w.set_cfg_user_persona(u.system_prompt.into());
                }
                w.set_cfg_status(SharedString::default());
                w.set_settings_page(true);
            }
        });
        window.on_settings_back({
            let window_weak = window.as_weak();
            move || {
                if let Some(w) = window_weak.upgrade() {
                    w.set_settings_page(false);
                }
            }
        });
        window.on_settings_saved({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let models = models.clone();
            let db = db.clone();
            move |p, url, key, model, sys_prompt, user_persona, temp| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let status = save_settings(
                    p,
                    &url,
                    &key,
                    &model,
                    &sys_prompt,
                    &user_persona,
                    &temp,
                    &db,
                    &cache,
                    &models,
                    &w,
                );
                w.set_cfg_status(status.clone());
                // 整页设置不自动跳回，留在本页回显成功/失败；用户点「返回」退出
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
        let timer_models = models.clone();
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

    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}

/// 每次发送现读 DB 活跃角色行组装轮次人设（毫秒级，读失败按无角色处理不阻断发送）。
/// 助手/用户两段提示词与温度全空 → `None`（请求与旧行为逐位一致）。
fn persona_for_turn(db: &Db) -> Option<TurnPersona> {
    let a = db.active_persona("assistant").ok().flatten();
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

/// 温度文本 → `Option<f64>`：空 = 未设定（不发字段）；非数字或越界 0–2 回 `Err` 文案。
fn parse_temperature(s: &str) -> Result<Option<f64>, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let v: f64 = t.parse().map_err(|_| "温度需为数字")?;
    if !(0.0..=2.0).contains(&v) {
        return Err("温度需在 0–2 之间".into());
    }
    Ok(Some(v))
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
