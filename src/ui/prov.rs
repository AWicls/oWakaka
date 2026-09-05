//! 提供商页装配（Step A：providers 整包即时读写，密钥恒不下 UI）。
//!
//! 本模块只做「DB 整包 ↔ 窗口属性」的注入与回写，不发网络请求：
//! 连通测试/远端拉取由 run() 接线取 [`prov_test_config`] 组装的临时视图后异步执行。

use std::rc::Rc;

use crate::ai::{
    config::{Api, Config, Secrets, load_toml, store_toml},
    provider::Provider,
    providers::{ModelInfo, Providers, join_url},
};
use crate::db::Db;
use slint::{SharedString, VecModel};

use super::{AppWindow, ModelRow, ProvRow, RemoteRow};

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
pub(super) fn prov_inject(db: &Db, window: &AppWindow, select: &str) {
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

/// 「完成」：表单写回对应条目（kind/锁定族不信任 UI 传入）；密钥框非空才更新 `[keys]`。
pub(super) fn prov_save(db: &Db, window: &AppWindow) -> SharedString {
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
