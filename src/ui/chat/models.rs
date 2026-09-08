//! 模型下拉状态机：跨「全部启用提供商」聚合模型清单 + 路由对（提供商, 模型）。
//!
//! 提供商不再有显式「使用此提供商」入口：下拉列出所有可用（未删未禁）提供商的模型，
//! 选中哪家模型的哪一行，即隐式把路由切到该提供商（active 指针 + active_model 落库），
//! 单活运行时（`Providers::view`/`Client`）零改动复用。
//! 状态全在 [`ModelsState`]，对窗口只经由 `apply` 单向注入；远端 `/models` 拉取
//! 的异步回包经 ui.rs 的 Timer 排空后调 `rebuild(Some(&ids))` 并入（挂当前路由家）。

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

use crate::ai::providers::{ModelInfo, Providers};
use crate::db::Db;
use slint::{ComponentHandle, Model, VecModel};

use crate::ui::{AppWindow, ModelItem};

/// 模型切换下拉的 UI 状态：数据源、路由对 (提供商 id, 模型 id)、思考能力表
/// （仅登记模型有条目，未登记默认支持，绝不误关）；
/// `fetched` 防抖——首次打开下拉才拉远端 `/models`，失败时下回打开可重试
#[derive(Clone)]
pub(in crate::ui) struct ModelsState {
    db: Rc<Db>,
    items: Rc<VecModel<ModelItem>>,
    pub(in crate::ui) active: Rc<RefCell<(String, String)>>,
    caps: Rc<RefCell<BTreeMap<(String, String), bool>>>,
    pub(in crate::ui) fetched: Rc<Cell<bool>>,
}

impl ModelsState {
    /// 初始路由对取自 providers 整包；读不到则空（发送回落客户端默认）
    pub(in crate::ui) fn from_config(db: Rc<Db>) -> Self {
        let pair = Providers::load(&db)
            .ok()
            .map(|ps| (ps.active.clone(), ps.active_model.clone()))
            .unwrap_or_default();
        let state = Self {
            db,
            items: Rc::new(VecModel::default()),
            active: Rc::new(RefCell::new(pair)),
            caps: Rc::new(RefCell::new(BTreeMap::new())),
            fetched: Rc::new(Cell::new(false)),
        };
        state.rebuild(None);
        state
    }

    /// 重建下拉清单：全部可用提供商的模型聚合（(提供商,模型) 去重）∪ 远端结果
    /// （并入当前路由家）。跨家同模型 id 撞车 → 显示加「 · 提供商名」后缀区分。
    pub(in crate::ui) fn rebuild(&self, remote: Option<&[String]>) {
        let ps = Providers::load(&self.db).unwrap_or_default();
        let (ap, am) = self.active.borrow().clone();
        let mut rows: Vec<(String, ModelInfo)> = Vec::new();
        let mut caps: BTreeMap<(String, String), bool> = BTreeMap::new();
        {
            let mut push = |prov: &str, m: ModelInfo| {
                if !m.id.is_empty() && !rows.iter().any(|(p, x)| p == prov && x.id == m.id) {
                    rows.push((prov.to_string(), m));
                }
            };
            for e in ps.usable() {
                for m in &e.models {
                    caps.insert((e.id.clone(), m.id.clone()), m.thinking);
                    push(&e.id, m.clone());
                }
            }
            if let Some(ids) = remote {
                for id in ids {
                    push(
                        &ap,
                        ModelInfo {
                            id: id.clone(),
                            ..Default::default()
                        },
                    );
                }
            }
            // 激活对不在清单（提供商被禁/模型未登记）→ 补进首位，恒可见可回切
            if !am.is_empty() && !rows.iter().any(|(p, x)| *p == ap && x.id == am) {
                rows.insert(
                    0,
                    (
                        ap.clone(),
                        ModelInfo {
                            id: am.clone(),
                            ..Default::default()
                        },
                    ),
                );
            }
        }
        let mut dup: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, m) in &rows {
            *dup.entry(m.id.as_str()).or_default() += 1;
        }
        let items = rows
            .iter()
            .map(|(p, m)| {
                let mut display = if m.alias.is_empty() {
                    m.id.clone()
                } else {
                    m.alias.clone()
                };
                if dup[m.id.as_str()] > 1
                    && let Some(e) = ps.find(p)
                {
                    display.push_str(" · ");
                    display.push_str(&e.name);
                }
                ModelItem {
                    prov: p.as_str().into(),
                    id: m.id.as_str().into(),
                    display: display.into(),
                    current: *p == ap && m.id == am,
                }
            })
            .collect::<Vec<_>>();
        self.items.set_vec(items);
        *self.caps.borrow_mut() = caps;
    }

    /// 把状态同步到窗口（下拉数据源 + 模型按钮文案 + 思考能力位）
    pub(in crate::ui) fn apply(&self, window: &AppWindow) {
        window.set_models(self.items.clone().into());
        let (ap, am) = self.active.borrow().clone();
        let label = if am.is_empty() {
            "未选模型".into()
        } else {
            self.items
                .iter()
                .find(|m| m.prov == ap.as_str() && m.id == am.as_str())
                .map(|m| m.display.clone())
                .unwrap_or_else(|| am.as_str().into())
        };
        window.set_model_label(label);
        let capable = self.supports_thinking(&ap, &am);
        window.set_thinking_capable(capable);
    }

    /// 当前激活模型是否支持思考（未登记的模型视为支持，绝不误关）
    pub(in crate::ui) fn supports_thinking(&self, prov: &str, id: &str) -> bool {
        self.caps
            .borrow()
            .get(&(prov.to_string(), id.to_string()))
            .copied()
            .unwrap_or(true)
    }

    /// 从设置页返回后重拉提供商数据源（模型增删/启停/别名可能已变），路由对以 DB 为准
    pub(in crate::ui) fn sync_store(&self, window: &AppWindow) {
        if let Ok(ps) = Providers::load(&self.db) {
            let cur = self.active.borrow().clone();
            // DB 路由对仍有效（pick 即时回写，正常一致）才保留本会话未落库的切换
            let valid = ps
                .find(&cur.0)
                .is_some_and(|e| !e.deleted && e.enabled && ps.active == cur.0);
            if !valid {
                *self.active.borrow_mut() = (ps.active.clone(), ps.active_model.clone());
            }
        }
        self.rebuild(None);
        self.apply(window);
    }

    /// 切换路由：本会话即时生效（后续发送携带）并回写 providers（active+active_model），
    /// 回写失败仅影响重启后持久，打日志不阻断
    pub(in crate::ui) fn pick(&self, window: &AppWindow, prov: String, model: String) {
        if model.is_empty() || *self.active.borrow() == (prov.clone(), model.clone()) {
            return;
        }
        *self.active.borrow_mut() = (prov, model.clone());
        self.rebuild(None);
        self.apply(window);
        let (p, _) = self.active.borrow().clone();
        if !p.is_empty()
            && let Err(e) = Providers::set_active(&self.db, &p)
        {
            eprintln!("提供商回写失败（仅本次会话生效）: {e}");
        }
        if let Err(e) = Providers::set_active_model(&self.db, &model) {
            eprintln!("模型回写设置失败（仅本次会话生效）: {e}");
        }
    }

    /// 按模型 id 选模型（助手绑默认模型等无提供商上下文场景）：
    /// 当前路由家含该模型优先，否则首个含该模型的可用提供商；都没有 → 维持当前家
    pub(in crate::ui) fn pick_by_model(&self, window: &AppWindow, model: &str) {
        if model.is_empty() {
            return;
        }
        let cur = self.active.borrow().clone();
        let ps = Providers::load(&self.db).unwrap_or_default();
        let has = |e: &crate::ai::providers::ProviderEntry| e.models.iter().any(|m| m.id == model);
        let prov = ps
            .find(&cur.0)
            .filter(|e| !e.deleted && e.enabled && has(e))
            .or_else(|| ps.usable().find(|e| has(e)))
            .map_or_else(|| cur.0.clone(), |e| e.id.clone());
        self.pick(window, prov, model.to_string());
    }
}

/// 模型下拉接线：选择即时生效（隐式切提供商路由）并回写 providers，
/// 并清客户端缓存让本轮发送走新家；打开下拉首次异步拉远端 /models（仅当前路由家）。
pub(in crate::ui) fn wire_models(window: &AppWindow, ctx: &crate::ui::Ctx) {
    let window_weak = window.as_weak();
    let models = ctx.models.clone();
    let cache = ctx.cache.clone();
    window.on_model_picked(move |prov, id| {
        if let Some(w) = window_weak.upgrade() {
            models.pick(&w, prov.to_string(), id.to_string());
            *cache.lock().unwrap() = None; // 下次发送经新路由提供商建客户端
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
        let Ok(client) = super::ensure_client(&cache, &db) else {
            return; // 配置缺失：下拉仍可用 config 清单，不打扰
        };
        models.fetched.set(true);
        let tx = tx.clone();
        runtime.spawn(async move {
            let result = client.list_models().await.map_err(|e| e.to_string());
            let _ = tx.send(crate::ui::UiMsg::Models(result));
        });
    });
}
