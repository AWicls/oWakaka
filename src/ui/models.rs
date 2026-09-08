//! 模型下拉状态机：清单数据源、激活模型、别名与能力表（提供商页改动的消费端）。
//!
//! 状态全在 [`ModelsState`]，对窗口只经由 `apply` 单向注入；远端 `/models` 拉取
//! 的异步回包经 ui.rs 的 Timer 排空后调 `rebuild(Some(&ids))` 并入。

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

use crate::ai::{config::Config, providers::Providers};
use crate::db::Db;
use slint::{ComponentHandle, SharedString, VecModel};

use super::{AppWindow, ModelItem};

/// 当前提供商的模型 id → 是否支持思考（Step B 能力位的第一个消费点）。
fn load_caps(db: &Db) -> BTreeMap<String, bool> {
    Providers::load(db)
        .ok()
        .and_then(|ps| {
            ps.active_entry().map(|e| {
                e.models
                    .iter()
                    .map(|m| (m.id.clone(), m.thinking))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// 模型切换下拉的 UI 状态：数据源、激活 ID、别名表（当前提供商模型）、思考能力表；
/// `fetched` 防抖——首次打开下拉才拉远端 `/models`，失败时下回打开可重试
#[derive(Clone)]
pub(super) struct ModelsState {
    db: Rc<Db>,
    items: Rc<VecModel<ModelItem>>,
    pub(super) active: Rc<RefCell<String>>,
    aliases: Rc<RefCell<BTreeMap<String, String>>>,
    caps: Rc<RefCell<BTreeMap<String, bool>>>,
    pub(super) fetched: Rc<Cell<bool>>,
}

impl ModelsState {
    /// 初始值取自运行配置（model + 提供商模型别名/能力）；读不到则空激活（发送回落客户端默认）
    pub(super) fn from_config(db: Rc<Db>) -> Self {
        let (active, aliases) = match Config::load(&db) {
            Ok(cfg) => (cfg.model, cfg.models),
            Err(_) => (String::new(), BTreeMap::new()),
        };
        let caps = load_caps(&db);
        let state = Self {
            db,
            items: Rc::new(VecModel::default()),
            active: Rc::new(RefCell::new(active)),
            aliases: Rc::new(RefCell::new(aliases)),
            caps: Rc::new(RefCell::new(caps)),
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
    pub(super) fn rebuild(&self, remote: Option<&[String]>) {
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

    /// 把状态同步到窗口（下拉数据源 + 模型按钮文案 + 思考能力位）
    pub(super) fn apply(&self, window: &AppWindow) {
        window.set_models(self.items.clone().into());
        let active = self.active.borrow().clone();
        let label = if active.is_empty() {
            "未选模型".into()
        } else {
            self.display(&active)
        };
        window.set_model_label(label);
        let capable = self.supports_thinking(&active);
        window.set_thinking_capable(capable);
    }

    /// 当前激活模型是否支持思考（未登记的模型视为支持，绝不误关）
    pub(super) fn supports_thinking(&self, id: &str) -> bool {
        self.caps.borrow().get(id).copied().unwrap_or(true)
    }

    /// 从设置页返回后重拉提供商数据源（模型增删/别名/能力可能已变）
    pub(super) fn sync_store(&self, window: &AppWindow) {
        if let Ok(cfg) = Config::load(&self.db) {
            *self.aliases.borrow_mut() = cfg.models;
        }
        *self.caps.borrow_mut() = load_caps(&self.db);
        self.rebuild(None);
        self.apply(window);
    }

    /// 切换激活模型：本会话即时生效（后续发送携带）并回写设置（DB kv），
    /// 回写失败仅影响重启后持久，打日志不阻断
    pub(super) fn pick(&self, window: &AppWindow, id: String) {
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

/// 模型下拉接线：选择即时生效并回写 config；打开下拉首次异步拉远端 /models。
pub(super) fn wire_models(window: &AppWindow, ctx: &super::Ctx) {
    let window_weak = window.as_weak();
    let models = ctx.models.clone();
    window.on_model_picked(move |id| {
        if let Some(w) = window_weak.upgrade() {
            models.pick(&w, id.to_string());
        }
    });
    let models = ctx.models.clone();
    let runtime = ctx.runtime.clone();
    let cache = ctx.cache.clone();
    let tx = ctx.tx.clone();
    let db = ctx.db.clone();
    window.on_models_requested(move || {
        if models.fetched.get() {
            return; // 已成功拉取过，不重复请求
        }
        let Ok(client) = super::chat::ensure_client(&cache, &db) else {
            return; // 配置缺失：下拉仍可用 config 清单，不打扰
        };
        models.fetched.set(true);
        let tx = tx.clone();
        runtime.spawn(async move {
            let result = client.list_models().await.map_err(|e| e.to_string());
            let _ = tx.send(super::UiMsg::Models(result));
        });
    });
}
