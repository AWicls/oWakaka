//! 会话栏与回收站接线：新建/切换/删除/回收站（restore/purge）回调 → Host 簿记，
//! 兼 [`refresh_trash`] 回收站列表重注入（侧栏计数与浮层同源）。

use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

use crate::db::Db;
use crate::ui::{AppWindow, Ctx, TrashItem};

/// 回收站列表重注入（侧栏按钮计数与浮层同源）。
pub(in crate::ui) fn refresh_trash(db: &Db, window: &AppWindow) {
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

/// 会话栏接线：新建/切换/删除/回收站（restore/purge）。删除只换「当前可见」模型与按钮态，
/// 流式增量按 sid 回原会话（路由在 ui.rs 的 Timer）。
pub(in crate::ui) fn wire_sidebar(window: &AppWindow, ctx: &Ctx) {
    let db = ctx.db.clone();
    let host = ctx.host.clone();

    let window_weak = window.as_weak();
    window.on_new_session({
        let host = host.clone();
        let models = ctx.models.clone();
        move || {
            if let Some(w) = window_weak.upgrade() {
                host.new_session(&w);
                // 新会话绑定助手指定了默认模型 → 自动切模型下拉（回对话页即可见）
                host.apply_assistant_model(&models, &w);
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
