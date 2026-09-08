//! 提供商页装配（Step A：providers 整包即时读写，密钥恒不下 UI）。
//!
//! 注入与回写只做「DB 整包 ↔ 窗口属性」；连通测试/远端拉取在本模块接线
//! [`wire_prov`] 中取 [`prov_test_config`] 组装的临时视图后经 runtime 异步发起，
//! 结果走 ui.rs 的回流 Timer 落回窗口。

use std::rc::Rc;

use crate::ai::{
    config::{Api, Config, Secrets, load_toml, store_toml},
    provider::Provider,
    providers::{ModelInfo, Providers, join_url},
};
use crate::db::Db;
use slint::{ComponentHandle, SharedString, VecModel};

use crate::ui::{AppWindow, ModelRow, RailItem, RemoteRow};

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

/// 竖栏行数据（提供商列表卡的唯一构造点，整包注入与局部刷新共用）。
fn prov_rail_rows(ps: &Providers) -> Vec<RailItem> {
    ps.list
        .iter()
        .map(|e| RailItem {
            id: e.id.as_str().into(),
            name: e.name.as_str().into(),
            sub: e.kind.display_name().into(),
            active: e.id == ps.active,
            activeText: "● 使用中".into(),
            deleted: e.deleted,
        })
        .collect()
}

/// 只刷竖栏列表卡（增删改行后不动表单，保住正在输入的焦点）。
pub(super) fn prov_list_refresh(db: &Db, window: &AppWindow) {
    let ps = Providers::load(db).unwrap_or_default();
    window.set_provs(Rc::new(VecModel::from(prov_rail_rows(&ps))).into());
}

/// 重注入提供商列表与详情。select 为空 = 维持当前选中；选中失效回落 active。
/// 密钥框恒注空串（不回显明文；提交空 = 保持已存密钥不变）。
pub(in crate::ui) fn prov_inject(db: &Db, window: &AppWindow, select: &str) {
    let ps = Providers::load(db).unwrap_or_default();
    window.set_provs(Rc::new(VecModel::from(prov_rail_rows(&ps))).into());
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
pub(super) fn prov_models_refresh(ps: &Providers, window: &AppWindow) {
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
pub(super) fn remote_mark_added(window: &AppWindow, model_id: &str, added: bool) {
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

/// 远端清单异步回包 → 窗口：按「未添加优先 + id 升序」重排注入并复位加载态。
/// 入参与 [`UiMsg::ProvFetch`](crate::ui::UiMsg::ProvFetch) 一致，由 ui.rs 的回流 Timer 转调（页面逻辑归本页）。
pub(in crate::ui) fn on_fetch_result(window: &AppWindow, result: Result<Vec<String>, String>) {
    use slint::Model;
    match result {
        Ok(ids) => {
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
        }
        Err(e) => {
            window.set_remote_loading(false);
            let brief: String = e.chars().take(160).collect();
            window.set_prov_status(format!("拉取失败：{brief}").into());
        }
    }
}

/// 连通测试异步回包 → 窗口：文案直接写 prov-status。
pub(in crate::ui) fn on_test_result(window: &AppWindow, msg: SharedString) {
    window.set_prov_status(msg);
}

/// 行级模型改写的统一入口：读当前选中提供商 → 取/建该 model → 应用 f → upsert。
/// 提供商被删/不可用则写 prov-status 并静默返回。
/// `quiet` = 只落库不回刷 Slint 列表——逐字符的文本输入专用：
/// 回刷会重建 for-delegate，正在敲字的 TextInput 随销毁丢焦点、布局抖动。
pub(super) fn model_upsert(
    db: &Db,
    window: &AppWindow,
    model_id: &str,
    quiet: bool,
    f: impl FnOnce(&mut ModelInfo),
) {
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
    if !quiet {
        let ps = Providers::load(db).unwrap_or_default();
        prov_models_refresh(&ps, window);
        remote_mark_added(window, model_id, true);
    }
}

/// 自动保存：把表单当前值逐键落库到对应条目（kind/锁定族不信任 UI 传入）；
/// 密钥框非空才更新 `[keys]`。不回注入（防打字失焦/密钥框被清空），只刷竖栏列表；
/// 无条目/已删/custom 地址还没敲出来 → 静默跳过（半截态不值得入库报错）。
pub(super) fn prov_autosave(db: &Db, window: &AppWindow) {
    let id = window.get_prov_sel().to_string();
    if id.is_empty() {
        return;
    }
    let Ok(ps) = Providers::load(db) else {
        return;
    };
    let Some(cur) = ps.find(&id) else {
        return;
    };
    if cur.deleted {
        return;
    }
    let mut e = cur.clone();
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
        return;
    }
    // models 不在表单保存里动（Step B：行级 upsert/remove 即时落盘）
    if Providers::save_entry(db, &e).is_err() {
        return;
    }
    let key = window.get_prov_key().trim().to_string();
    if !key.is_empty() {
        let mut secrets: Secrets = load_toml("config.toml").unwrap_or_default();
        secrets.keys.insert(id.clone(), key);
        let _ = store_toml(&secrets, "config.toml");
    }
    prov_list_refresh(db, window);
    window.set_prov_status("已自动保存".into());
}

/// 用表单当前值（未保存也可测）组装临时视图；缺参则写 prov-status 回 `None`。
pub(super) fn prov_test_config(db: &Db, window: &AppWindow) -> Option<Config> {
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

/// 提供商页接线：列表/详情即时读写 providers 整包（Step A 骨架）＋
/// Step B 模型管理（拉远端 / ＋添加 / 自定义 / 别名 / 能力勾选 / 移除）。
/// 回连按钮均重建客户端缓存（下一次发送即用新配置）。
pub(in crate::ui) fn wire_prov(window: &AppWindow, ctx: &crate::ui::Ctx) {
    let db = ctx.db.clone();
    let cache = ctx.cache.clone();
    let runtime = ctx.runtime.clone();
    let tx = ctx.tx.clone();

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
    window.on_prov_autosave({
        let window_weak = window.as_weak();
        let cache = cache.clone();
        let db = db.clone();
        move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            prov_autosave(&db, &w);
            *cache.lock().unwrap() = None; // 编辑可能改了使用中的条目，重建客户端
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
        move |id| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            if id.is_empty() {
                return;
            }
            match Providers::purge(&db, "config.toml", &id) {
                Ok(()) => {
                    *cache.lock().unwrap() = None;
                    if w.get_prov_sel() == id.as_str() {
                        prov_inject(&db, &w, ""); // 条目已没了，回注入兜底选中
                        w.set_prov_status("已彻底删除".into());
                    } else {
                        prov_list_refresh(&db, &w); // 删的是非选中行：只刷列表，不劫持编辑目标
                    }
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
            let client = crate::ai::client::Client::from_config(&cfg);
            let tx = tx.clone();
            runtime.spawn(async move {
                let t = std::time::Instant::now();
                let msg = match client.list_models().await {
                    Ok(ids) => {
                        format!("连通：{} 个模型，{} ms", ids.len(), t.elapsed().as_millis())
                    }
                    Err(e) => {
                        let brief: String = e.to_string().chars().take(160).collect();
                        format!("连接失败：{brief}")
                    }
                };
                let _ = tx.send(crate::ui::UiMsg::ProvTest(msg.into()));
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
            let client = crate::ai::client::Client::from_config(&cfg);
            let tx = tx.clone();
            runtime.spawn(async move {
                let result = client.list_models().await.map_err(|e| e.to_string());
                let _ = tx.send(crate::ui::UiMsg::ProvFetch(result));
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
            model_upsert(&db, &w, &mid, false, |_| {});
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
            model_upsert(&db, &w, &mid, false, |m| m.alias = alias);
        }
    });
    window.on_prov_model_alias({
        let window_weak = window.as_weak();
        let db = db.clone();
        move |mid, alias| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            // 逐字符静默落库：不重建列表 = 保住 TextInput 焦点
            let alias = alias.trim_start().to_string();
            model_upsert(&db, &w, &mid, true, |m| m.alias = alias);
        }
    });
    window.on_prov_model_commit({
        let window_weak = window.as_weak();
        let db = db.clone();
        move |_mid| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            // 「完成」收起后统一回刷：名称行标签与模型下拉同步最新别名
            match Providers::load(&db) {
                Ok(ps) => prov_models_refresh(&ps, &w),
                Err(e) => w.set_prov_status(format!("回刷失败: {e}").into()),
            }
        }
    });
    window.on_prov_model_cap({
        let window_weak = window.as_weak();
        let db = db.clone();
        move |mid, cap, on| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            model_upsert(&db, &w, &mid, false, |m| match cap {
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
