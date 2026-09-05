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
    config::{Api, Config, Secrets, load_toml, store_toml},
    provider::Provider,
    providers::{ModelInfo, Providers, join_url},
};
use crate::db::Db;
use slint::{Model, SharedString, Timer, TimerMode, VecModel};

use bubbles::{STATE_PARTIAL, append_part, fold_tail, snapshot_history};
use host::{Host, RunRef, StreamMsg};

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

    /// 模型 ID 的显示名：config 别名优先，未配/空别名原样显示
    fn display(&self, id: &str) -> SharedString {
        self.aliases
            .borrow()
            .get(id)
            .filter(|a| !a.is_empty())
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

    /// 把激活模型写回 providers 整包的 active_model 字段
    fn persist_model(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        Providers::set_active_model(&self.db, id)
    }
}

/// 设置页「保存」= 仅角色设定两页（助手提示词/温度、用户人设 → persona 活跃行）。
/// 提供商字段由提供商页即时落盘，不经此；温度非法直接回文案不落盘。
fn save_personas(db: &Db, sys_prompt: &str, user_persona: &str, temp: &str) -> SharedString {
    let temperature = match parse_temperature(temp) {
        Ok(v) => v,
        Err(msg) => return msg.into(),
    };
    let r = db
        .upsert_active_persona("assistant", "默认助手", sys_prompt.trim(), temperature)
        .and_then(|()| db.upsert_active_persona("user", "默认人设", user_persona.trim(), None));
    match r {
        Ok(()) => "已保存，下一次发送生效".into(),
        Err(e) => format!("保存失败: {e}").into(),
    }
}

// —— 提供商页装配（Step A：providers 整包即时读写，密钥恒不下 UI） ——

/// 表单 api 序号 ↔ 枚举（0 chat | 1 responses）
fn api_of(i: i32) -> Api {
    if i == 1 { Api::Responses } else { Api::Chat }
}

fn api_to_i(api: Api) -> i32 {
    match api {
        Api::Chat => 0,
        Api::Responses => 1,
    }
}

/// 重注入提供商列表与详情。select 为空 = 维持当前选中；选中失效回落 active。
/// 密钥框恒注空串（不回显明文；提交空 = 保持已存密钥不变）。
fn prov_inject(db: &Db, window: &AppWindow, select: &str) {
    let ps = Providers::load(db).unwrap_or_default();
    let rows: Vec<ProvRow> = ps
        .list
        .iter()
        .map(|e| ProvRow {
            id: e.id.as_str().into(),
            name: e.name.as_str().into(),
            kind: e.kind.display_name().into(),
            active: e.id == ps.active,
            deleted: e.deleted,
        })
        .collect();
    window.set_provs(Rc::new(VecModel::from(rows)).into());
    window.set_remote_models(slint::ModelRc::default()); // 远端清单不跨提供商缓存（端点已变）
    let mut sel = select.to_string();
    if sel.is_empty() {
        sel = window.get_prov_sel().to_string();
    }
    if ps.find(&sel).is_none() {
        sel = ps.active.clone();
    }
    window.set_prov_sel(sel.as_str().into());
    let Some(e) = ps.find(&sel) else {
        // 零可用条目：表单整体清空，仅剩新增入口
        window.set_prov_name("".into());
        window.set_prov_base("".into());
        window.set_prov_suffix("".into());
        window.set_prov_key("".into());
        window.set_prov_api(0);
        window.set_sel_kind("".into());
        window.set_sel_locked(false);
        window.set_sel_active(false);
        window.set_sel_deleted(false);
        window.set_prov_models(Rc::new(VecModel::<ModelRow>::default()).into());
        return;
    };
    window.set_prov_name(e.name.as_str().into());
    window.set_prov_base(e.base_url.as_str().into());
    window.set_prov_suffix(e.url_suffix.as_str().into());
    window.set_prov_key("".into());
    window.set_prov_api(api_to_i(e.kind.locked_api().or(e.api).unwrap_or_default()));
    window.set_sel_kind(e.kind.display_name().into());
    window.set_sel_locked(e.kind.locked_api().is_some());
    window.set_sel_active(sel == ps.active);
    window.set_sel_deleted(e.deleted);
    prov_models_refresh(&ps, window);
}

/// 当前选中提供商的模型列表 → 窗口（Step B 行级 CRUD 的展示面）。
fn prov_models_refresh(ps: &Providers, window: &AppWindow) {
    let rows: Vec<ModelRow> = ps
        .find(&window.get_prov_sel())
        .filter(|e| !e.deleted)
        .map(|e| {
            e.models
                .iter()
                .map(|m| ModelRow {
                    id: m.id.as_str().into(),
                    alias: m.alias.as_str().into(),
                    thinking: m.thinking,
                    vision: m.vision,
                    audio: m.audio,
                    video: m.video,
                    tools: m.tools,
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_prov_models(Rc::new(VecModel::from(rows)).into());
}

/// 远端清单 added 标记同步（当前提供商已含该 id）。
fn remote_mark_added(window: &AppWindow, model_id: &str, added: bool) {
    use slint::Model;
    let list = window.get_remote_models();
    let mut rows: Vec<RemoteRow> = (0..list.row_count())
        .filter_map(|i| list.row_data(i))
        .map(|r| RemoteRow {
            added: if r.id == model_id { added } else { r.added },
            ..r
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    rows.sort_by(|a, b| {
        (a.added, &a.id).cmp(&(b.added, &b.id)) // 未添加的排前，方便连续添加
    });
    window.set_remote_models(Rc::new(VecModel::from(rows)).into());
}

/// 行级模型改写的统一入口：读当前选中提供商 → 取/建该 model → 应用 f → upsert → 回刷。
/// 提供商被删/不可用则写 prov-status 并静默返回。
fn model_upsert(db: &Db, window: &AppWindow, model_id: &str, f: impl FnOnce(&mut ModelInfo)) {
    let sel = window.get_prov_sel().to_string();
    let ps = match Providers::load(db) {
        Ok(ps) => ps,
        Err(e) => {
            window.set_prov_status(format!("读取失败: {e}").into());
            return;
        }
    };
    let Some(entry) = ps.find(&sel).filter(|e| !e.deleted) else {
        window.set_prov_status("提供商不可用，无法编辑模型".into());
        return;
    };
    let mut m = entry
        .models
        .iter()
        .find(|x| x.id == model_id)
        .cloned()
        .unwrap_or_else(|| ModelInfo {
            id: model_id.to_string(),
            ..Default::default()
        });
    f(&mut m);
    if let Err(e) = Providers::upsert_model(db, &sel, m) {
        window.set_prov_status(format!("模型保存失败: {e}").into());
        return;
    }
    let ps = Providers::load(db).unwrap_or_default();
    prov_models_refresh(&ps, window);
    remote_mark_added(window, model_id, true);
}

/// 「完成」：表单写回对应条目（kind/锁定族不信任 UI 传入）；密钥框非空才更新 `[keys]`。
fn prov_save(db: &Db, window: &AppWindow) -> SharedString {
    let id = window.get_prov_sel().to_string();
    let ps = match Providers::load(db) {
        Ok(ps) => ps,
        Err(e) => return format!("读取失败: {e}").into(),
    };
    let Some(cur) = ps.find(&id).cloned() else {
        return "条目不存在，请从列表重新选择".into();
    };
    if cur.deleted {
        return "已删除条目：恢复后才能编辑".into();
    }
    let mut e = cur;
    e.name = window.get_prov_name().trim().to_string();
    if e.name.is_empty() {
        e.name = e.kind.display_name().to_string();
    }
    e.base_url = window.get_prov_base().trim().to_string();
    e.url_suffix = window.get_prov_suffix().trim().to_string();
    e.api = match e.kind.locked_api() {
        Some(locked) => Some(locked),
        None => Some(api_of(window.get_prov_api())),
    };
    if e.kind == Provider::Custom && join_url(&e.base_url, &e.url_suffix).is_empty() {
        return "自定义提供商必须填基础地址".into();
    }
    // models 不在表单保存里动（Step B：行级 upsert/remove 即时落盘）
    if let Err(err) = Providers::save_entry(db, &e) {
        return format!("保存失败: {err}").into();
    }
    let key = window.get_prov_key().trim().to_string();
    if !key.is_empty() {
        let mut secrets: Secrets = load_toml("config.toml").unwrap_or_default();
        secrets.keys.insert(id.clone(), key);
        if let Err(err) = store_toml(&secrets, "config.toml") {
            return format!("设置已存，密钥写入失败: {err}").into();
        }
    }
    prov_inject(db, window, &id);
    "完成，已保存".into()
}

/// 用表单当前值（未保存也可测）组装临时视图；缺参则写 prov-status 回 `None`。
fn prov_test_config(db: &Db, window: &AppWindow) -> Option<Config> {
    let id = window.get_prov_sel().to_string();
    let ps = Providers::load(db).ok()?;
    let entry = ps.find(&id)?;
    let kind = entry.kind;
    let base_in = window.get_prov_base().trim().to_string();
    let base = if base_in.is_empty() {
        kind.default_base_url().unwrap_or("").to_string()
    } else {
        base_in
    };
    let full = join_url(&base, window.get_prov_suffix().trim());
    if full.is_empty() {
        window.set_prov_status("请先填写基础地址".into());
        return None;
    }
    let key_in = window.get_prov_key().trim().to_string();
    let key = if key_in.is_empty() {
        load_toml::<Secrets>("config.toml")
            .ok()
            .and_then(|s| s.keys.get(&id).cloned())
            .unwrap_or_default()
    } else {
        key_in
    };
    Some(Config {
        provider: kind,
        base_url: full,
        api_key: key,
        model: String::new(),
        api: Some(
            kind.locked_api()
                .unwrap_or_else(|| api_of(window.get_prov_api())),
        ),
        stream: true,
        models: Default::default(),
    })
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
    refresh_trash(&db, &window);

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

    // —— 设置整页：进入注入提供商档案 + persona 初值；保存 = persona 两页 ——
    {
        window.on_settings_requested({
            let window_weak = window.as_weak();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                prov_inject(&db, &w, "");
                w.set_prov_status(SharedString::default());
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
            let db = db.clone();
            move |sys_prompt, user_persona, temp| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let status = save_personas(&db, &sys_prompt, &user_persona, &temp);
                w.set_cfg_status(status);
                // 整页设置不自动跳回，留在本页回显成功/失败；用户点「返回」退出
            }
        });
    }

    // —— 提供商页：列表/详情即时读写 providers 整包（Step A 骨架） ——
    {
        window.on_prov_created({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |kind_i| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let kind = if kind_i == 1 {
                    Provider::XiaomiMimo
                } else {
                    Provider::Custom
                };
                match Providers::create(&db, kind) {
                    Ok(id) => prov_inject(&db, &w, &id),
                    Err(e) => w.set_prov_status(format!("新增失败: {e}").into()),
                }
            }
        });
        window.on_prov_selected({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |id| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                prov_inject(&db, &w, &id);
                w.set_prov_status(SharedString::default());
            }
        });
        window.on_prov_used({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let id = w.get_prov_sel().to_string();
                match Providers::set_active(&db, &id) {
                    Ok(()) => {
                        *cache.lock().unwrap() = None; // 下一次发送即用新提供商
                        prov_inject(&db, &w, &id);
                        w.set_prov_status("已切换，下一次发送生效".into());
                    }
                    Err(e) => w.set_prov_status(format!("切换失败: {e}").into()),
                }
            }
        });
        window.on_prov_saved({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let status = prov_save(&db, &w);
                *cache.lock().unwrap() = None; // 编辑可能改了使用中的条目，重建客户端
                w.set_prov_status(status);
            }
        });
        window.on_prov_deleted({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let id = w.get_prov_sel().to_string();
                if id.is_empty() {
                    return;
                }
                match Providers::soft_delete(&db, &id) {
                    Ok(()) => {
                        *cache.lock().unwrap() = None;
                        prov_inject(&db, &w, &id); // 留在原条目看"已删除"态
                    }
                    Err(e) => w.set_prov_status(format!("删除失败: {e}").into()),
                }
            }
        });
        window.on_prov_restored({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let id = w.get_prov_sel().to_string();
                if id.is_empty() {
                    return;
                }
                match Providers::restore(&db, &id) {
                    Ok(()) => {
                        *cache.lock().unwrap() = None;
                        prov_inject(&db, &w, &id);
                    }
                    Err(e) => w.set_prov_status(format!("恢复失败: {e}").into()),
                }
            }
        });
        window.on_prov_purged({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let id = w.get_prov_sel().to_string();
                if id.is_empty() {
                    return;
                }
                match Providers::purge(&db, "config.toml", &id) {
                    Ok(()) => {
                        *cache.lock().unwrap() = None;
                        prov_inject(&db, &w, ""); // 条目已没了，回注入兜底选中
                        w.set_prov_status("已彻底删除".into());
                    }
                    Err(e) => w.set_prov_status(format!("彻底删除失败: {e}").into()),
                }
            }
        });
        window.on_prov_tested({
            let window_weak = window.as_weak();
            let runtime = runtime.clone();
            let tx = tx.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let Some(cfg) = prov_test_config(&db, &w) else {
                    return; // 文案已写 prov-status
                };
                w.set_prov_status("测试连接中…".into());
                let client = Client::from_config(&cfg);
                let tx = tx.clone();
                runtime.spawn(async move {
                    let t = std::time::Instant::now();
                    let msg = match client.list_models().await {
                        Ok(ids) => format!(
                            "✓ 连通：{} 个模型，{} ms",
                            ids.len(),
                            t.elapsed().as_millis()
                        ),
                        Err(e) => {
                            let brief: String = e.to_string().chars().take(160).collect();
                            format!("✕ 连接失败: {brief}")
                        }
                    };
                    let _ = tx.send(UiMsg::ProvTest(msg.into()));
                });
            }
        });

        // —— Step B：模型管理（拉远端 + ＋添加 / 自定义 / 别名 / 能力勾选 / 移除） ——
        window.on_prov_fetch({
            let window_weak = window.as_weak();
            let runtime = runtime.clone();
            let tx = tx.clone();
            let db = db.clone();
            move || {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let Some(cfg) = prov_test_config(&db, &w) else {
                    return; // 缺 base 等已写 prov-status
                };
                w.set_remote_loading(true);
                let client = Client::from_config(&cfg);
                let tx = tx.clone();
                runtime.spawn(async move {
                    let result = client.list_models().await.map_err(|e| e.to_string());
                    let _ = tx.send(UiMsg::ProvFetch(result));
                });
            }
        });
        window.on_prov_model_added({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |mid| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                model_upsert(&db, &w, &mid, |_| {});
            }
        });
        window.on_prov_model_custom({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |mid, alias| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let mid = mid.trim().to_string();
                if mid.is_empty() {
                    w.set_prov_status("自定义模型必须填请求名".into());
                    return;
                }
                let alias = alias.trim().to_string();
                model_upsert(&db, &w, &mid, |m| m.alias = alias);
            }
        });
        window.on_prov_model_alias({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |mid, alias| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let alias = alias.trim().to_string();
                model_upsert(&db, &w, &mid, |m| m.alias = alias);
            }
        });
        window.on_prov_model_cap({
            let window_weak = window.as_weak();
            let db = db.clone();
            move |mid, cap, on| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                model_upsert(&db, &w, &mid, |m| match cap {
                    0 => m.thinking = on,
                    1 => m.vision = on,
                    2 => m.audio = on,
                    3 => m.video = on,
                    _ => m.tools = on,
                });
            }
        });
        window.on_prov_model_removed({
            let window_weak = window.as_weak();
            let cache = cache.clone();
            let db = db.clone();
            move |mid| {
                let Some(w) = window_weak.upgrade() else {
                    return;
                };
                let sel = w.get_prov_sel().to_string();
                if let Err(e) = Providers::remove_model(&db, &sel, &mid) {
                    w.set_prov_status(format!("移除失败: {e}").into());
                    return;
                }
                *cache.lock().unwrap() = None;
                let ps = Providers::load(&db).unwrap_or_default();
                prov_models_refresh(&ps, &w);
                remote_mark_added(&w, &mid, false);
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
                        window.set_prov_status(format!("✕ 拉取失败: {brief}").into());
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

/// 回收站列表重注入（侧栏按钮计数与浮层同源）。
fn refresh_trash(db: &Db, window: &AppWindow) {
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
