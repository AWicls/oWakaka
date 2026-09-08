//! AI 助手设定页装配：助手列表/详情回注（DB persona 为真相源）、
//! 表单逐键自动保存与页内全部回调接线。头像小工具（色板/首字/加载）在父模块 `settings`。

use std::rc::Rc;

use slint::{ComponentHandle, SharedString, VecModel};

use crate::db::Db;
use crate::ui::{AppWindow, Ctx, RailItem};

use super::{avatar_color, initial_of, load_avatar};

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

/// 竖栏行数据（助手列表卡的唯一构造点，整包注入与局部刷新共用）。
fn ast_rail_rows(list: &[crate::db::Assistant]) -> Vec<RailItem> {
    list.iter()
        .map(|a| RailItem {
            id: a.id.to_string().into(),
            name: a.name.as_str().into(),
            sub: (if a.model.is_empty() {
                "未指定模型"
            } else {
                &a.model
            })
            .into(),
            active: a.is_default,
            activeText: "● 默认".into(),
            deleted: a.deleted,
            enabled: true, // 启停是提供商专属，助手行恒可用
        })
        .collect()
}

/// 只刷竖栏列表卡（自动保存后同步行名/摘要，不动表单保住输入焦点）。
fn ast_list_refresh(db: &Db, window: &AppWindow) {
    let list = db.assistants().unwrap_or_default();
    window.set_asts(Rc::new(VecModel::from(ast_rail_rows(&list))).into());
}

/// 重注入助手列表与详情。select 为空 = 维持当前选中；选中失效回落默认助手。
pub(super) fn ast_inject(db: &Db, window: &AppWindow, select: &str) {
    let list = db.assistants().unwrap_or_default();
    window.set_asts(Rc::new(VecModel::from(ast_rail_rows(&list))).into());
    let sel = if select.is_empty() {
        window.get_ast_sel().to_string()
    } else {
        select.to_string()
    };
    let sel_id = sel
        .parse::<i64>()
        .ok()
        .filter(|id| list.iter().any(|a| a.id == *id))
        .or_else(|| db.default_assistant().ok().map(|d| d.id))
        .unwrap_or(-1);
    let Some(a) = list.iter().find(|x| x.id == sel_id) else {
        // 零条目（播种都失败的异常态）：表单整体清空，仅剩新增入口
        window.set_ast_sel(SharedString::default());
        window.set_ast_name("".into());
        window.set_ast_prompt("".into());
        window.set_ast_temp("".into());
        window.set_ast_opening("".into());
        window.set_ast_model("".into());
        window.set_ast_avatar("".into());
        window.set_ast_avatar_img(slint::Image::default());
        window.set_sel_ast_default(false);
        window.set_sel_ast_deleted(false);
        return;
    };
    window.set_ast_sel(a.id.to_string().as_str().into());
    window.set_ast_name(a.name.as_str().into());
    window.set_ast_prompt(a.system_prompt.as_str().into());
    window.set_ast_temp(
        a.temperature
            .map_or_else(String::new, |t| format!("{t}"))
            .into(),
    );
    window.set_ast_opening(a.opening.as_str().into());
    window.set_ast_model(a.model.as_str().into());
    window.set_ast_avatar(a.avatar.as_str().into());
    window.set_ast_avatar_img(load_avatar(&a.avatar));
    window.set_ast_avatar_color(avatar_color(a.id));
    window.set_ast_initial(initial_of(&a.name).as_str().into());
    window.set_sel_ast_default(a.is_default);
    window.set_sel_ast_deleted(a.deleted);
}

/// 自动保存：表单当前值逐键落库到选中助手（id/默认标记/删除态不信任 UI 传入）。
/// 不回注入（保住输入焦点），只刷竖栏列表；无选中/已删/名称清空中 → 静默跳过；
/// 温度暂不可解析（如敲到一半的 "0."）→ 仅回显提示，不落盘。
fn ast_autosave(db: &Db, window: &AppWindow) {
    let Ok(id) = window.get_ast_sel().parse::<i64>() else {
        return;
    };
    let Some(mut a) = db.assistant(id).ok().flatten() else {
        return;
    };
    if a.deleted {
        return;
    }
    let name = window.get_ast_name().trim().to_string();
    if name.is_empty() {
        return; // 清空重打途中：不算非法，先不落盘
    }
    let Ok(temp) = parse_temperature(&window.get_ast_temp()) else {
        window.set_ast_status("温度需为数字（0–2）".into());
        return;
    };
    a.name = name;
    a.system_prompt = window.get_ast_prompt().trim().to_string();
    a.temperature = temp;
    a.opening = window.get_ast_opening().trim().to_string();
    a.model = window.get_ast_model().trim().to_string();
    // 头像路径：选图/清除时已即时校验可加载并直接落库（ast_set_avatar），此处照存
    a.avatar = window.get_ast_avatar().trim().to_string();
    if db.save_assistant(&a).is_err() {
        return;
    }
    ast_list_refresh(db, window);
    window.set_ast_status("已自动保存".into());
}

/// 头像即时落库：只改 avatar 字段，不经表单校验（避免温度等半截态挡住存图）。
fn ast_set_avatar(db: &Db, window: &AppWindow, path: &str) {
    let Ok(id) = window.get_ast_sel().parse::<i64>() else {
        return;
    };
    if let Ok(Some(mut a)) = db.assistant(id)
        && !a.deleted
    {
        a.avatar = path.to_string();
        let _ = db.save_assistant(&a);
    }
}

/// AI 助手页接线：全部回调 = DB 操作 → [`ast_inject`] 回注 → ast-status 回显。
pub(super) fn wire_assist(window: &AppWindow, ctx: &Ctx) {
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_created(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        match db.insert_assistant("新助手") {
            Ok(id) => {
                ast_inject(&db, &w, &id.to_string());
                w.set_ast_status("已创建，编辑内容自动保存".into());
            }
            Err(e) => w.set_ast_status(format!("新增失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_selected(move |id| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        ast_inject(&db, &w, &id);
        w.set_ast_status(SharedString::default());
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_autosave(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        ast_autosave(&db, &w);
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_restored(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = w.get_ast_sel().parse::<i64>() else {
            return;
        };
        match db.restore_assistant(id) {
            Ok(()) => ast_inject(&db, &w, &id.to_string()),
            Err(e) => w.set_ast_status(format!("恢复失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_purged(move |id| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = id.parse::<i64>() else {
            return;
        };
        match db.purge_assistant(id) {
            Ok(()) => {
                if w.get_ast_sel().is_empty() || w.get_ast_sel().parse::<i64>() != Ok(id) {
                    ast_list_refresh(&db, &w); // 删的是非选中行：只刷列表，不劫持编辑目标
                } else {
                    ast_inject(&db, &w, ""); // 条目已没了，回注入兜底选中默认助手
                    w.set_ast_status("已彻底删除".into());
                }
            }
            Err(e) => w.set_ast_status(format!("彻底删除失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_defaulted(move |id| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = id.parse::<i64>() else {
            return;
        };
        match db.set_default_assistant(id) {
            Ok(()) => {
                if w.get_ast_sel().parse::<i64>() != Ok(id) {
                    ast_list_refresh(&db, &w); // 设的非选中行：只刷列表（● 默认 标记随行移动）
                } else {
                    ast_inject(&db, &w, &id.to_string());
                }
                w.set_ast_status("已设为默认，之后新建会话生效".into());
            }
            Err(e) => w.set_ast_status(format!("切换失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_avatar_picked(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        // 同步原生对话框：主线程短暂阻塞（原生对话框自带消息循环），桌面应用可接受
        let Some(path) = rfd::FileDialog::new()
            .set_title("选择头像图片")
            .add_filter("图片", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return; // 取消：表单不动
        };
        match slint::Image::load_from_path(&path) {
            Ok(img) => {
                let path_str = path.to_string_lossy();
                w.set_ast_avatar_img(img);
                w.set_ast_avatar(path_str.as_ref().into());
                ast_set_avatar(&db, &w, &path_str);
                w.set_ast_status("头像已自动保存".into());
            }
            // 默认解码仅 png/jpeg（slint std → image-decoders 实证），其余格式需先转换
            Err(e) => w.set_ast_status(format!("图片加载失败（支持 png/jpg）: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_avatar_cleared(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_ast_avatar(SharedString::default());
        w.set_ast_avatar_img(slint::Image::default());
        ast_set_avatar(&db, &w, "");
        w.set_ast_status("头像已清除".into());
    });
}
