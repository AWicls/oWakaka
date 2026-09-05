//! 设置页接线：AI 助手页（多助手列表+详情卡，Rust 整包为真相源回注）与
//! 用户设定页（全局用户人设保存）。提供商字段由提供商页即时落盘（见 [`super::prov`]），不经此。

use std::rc::Rc;

use crate::db::Db;
use slint::{Color, ComponentHandle, Image, SharedString, VecModel};

use super::{AppWindow, AstRow};

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

/// 头像圆底色板：按助手 id 取模选色（同 id 恒同色，跨重启稳定）。
const AVATAR_PALETTE: [(u8, u8, u8); 8] = [
    (0x5B, 0x8D, 0xEF), // 蓝
    (0x4C, 0xAF, 0x7D), // 绿
    (0xD9, 0x8E, 0x32), // 橙
    (0xB0, 0x6A, 0xD6), // 紫
    (0xD9, 0x5F, 0x5F), // 红
    (0x3F, 0xA8, 0xB8), // 青
    (0xC2, 0x64, 0x9A), // 粉
    (0x7D, 0x8F, 0x3C), // 橄榄
];

fn avatar_color(id: i64) -> Color {
    let (r, g, b) = AVATAR_PALETTE[id.rem_euclid(AVATAR_PALETTE.len() as i64) as usize];
    Color::from_rgb_u8(r, g, b)
}

/// 名称首字（Slint string 无 substring，Rust 侧算好注入）；空白名回退「?」。
fn initial_of(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |c| c.to_string())
}

/// 头像图片加载：空路径/加载失败一律回退空图（Slint 侧按 `width == 0` 判无图画圆底首字）。
fn load_avatar(path: &str) -> Image {
    if path.is_empty() {
        return Image::default();
    }
    match Image::load_from_path(std::path::Path::new(path)) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("头像图片加载失败（{path}）: {e}");
            Image::default()
        }
    }
}

/// 重注入助手列表与详情。select 为空 = 维持当前选中；选中失效回落默认助手。
pub(super) fn ast_inject(db: &Db, window: &AppWindow, select: &str) {
    let list = db.assistants().unwrap_or_default();
    let rows: Vec<AstRow> = list
        .iter()
        .map(|a| AstRow {
            id: a.id.to_string().into(),
            name: a.name.as_str().into(),
            sub: (if a.model.is_empty() {
                "未指定模型"
            } else {
                &a.model
            })
            .into(),
            active: a.is_default,
            deleted: a.deleted,
        })
        .collect();
    window.set_asts(Rc::new(VecModel::from(rows)).into());
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
        window.set_ast_avatar_img(Image::default());
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

/// 「完成」：表单写回选中助手（id/默认标记/删除态不信任 UI 传入）；
/// 名称空/温度非法直接回文案不落盘。
pub(super) fn ast_save(db: &Db, window: &AppWindow) -> SharedString {
    let Ok(id) = window.get_ast_sel().parse::<i64>() else {
        return "助手未选中".into();
    };
    let Some(mut a) = db.assistant(id).ok().flatten() else {
        return "助手不存在，请从列表重新选择".into();
    };
    if a.deleted {
        return "已删除助手：恢复后才能编辑".into();
    }
    a.name = window.get_ast_name().trim().to_string();
    if a.name.is_empty() {
        return "名称不能为空".into();
    }
    a.system_prompt = window.get_ast_prompt().trim().to_string();
    match parse_temperature(&window.get_ast_temp()) {
        Ok(v) => a.temperature = v,
        Err(msg) => return msg.into(),
    }
    a.opening = window.get_ast_opening().trim().to_string();
    a.model = window.get_ast_model().trim().to_string();
    // 头像路径：选图/清除时已即时校验可加载（on_ast_avatar_picked），此处照落盘
    a.avatar = window.get_ast_avatar().trim().to_string();
    match db.save_assistant(&a) {
        Ok(()) => {
            ast_inject(db, window, &id.to_string());
            "完成，已保存".into()
        }
        Err(e) => format!("保存失败: {e}").into(),
    }
}

/// 用户设定页「保存」= 全局用户人设（persona user 活跃行；助手字段已迁 AI 助手页）。
pub(super) fn save_user_persona(db: &Db, user_persona: &str) -> SharedString {
    match db.upsert_active_persona("user", "默认人设", user_persona.trim(), None) {
        Ok(()) => "已保存，下一次发送生效".into(),
        Err(e) => format!("保存失败: {e}").into(),
    }
}

/// 设置整页接线：进入注入提供商/助手档案 + 用户人设初值；返回重拉模型数据源；
/// 用户设定页保存 = 全局人设。AI 助手页操作全部在 [`wire_assist`] 接线。
pub(super) fn wire_settings(window: &AppWindow, ctx: &super::Ctx) {
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_settings_requested(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        super::prov::prov_inject(&db, &w, "");
        w.set_prov_status(SharedString::default());
        ast_inject(&db, &w, "");
        w.set_ast_status(SharedString::default());
        // 用户设定初值：persona user 活跃行（读不到/未设定 = 空白）
        if let Ok(Some(u)) = db.active_persona("user") {
            w.set_cfg_user_persona(u.system_prompt.into());
        }
        w.set_cfg_status(SharedString::default());
        w.set_settings_page(true);
    });
    let window_weak = window.as_weak();
    let models = ctx.models.clone();
    window.on_settings_back(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_settings_page(false);
            // 提供商/模型/能力可能在页内改过：回对话前重拉一次数据源
            models.sync_store(&w);
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_settings_saved(move |user_persona| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let status = save_user_persona(&db, &user_persona);
        w.set_cfg_status(status);
        // 整页设置不自动跳回，留在本页回显成功/失败；用户点「返回」退出
    });
    wire_assist(window, ctx);
}

/// AI 助手页接线：全部回调 = DB 操作 → [`ast_inject`] 回注 → ast-status 回显。
fn wire_assist(window: &AppWindow, ctx: &super::Ctx) {
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_created(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        match db.insert_assistant("新助手") {
            Ok(id) => {
                ast_inject(&db, &w, &id.to_string());
                w.set_ast_status("已创建，编辑后点「完成」保存".into());
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
    window.on_ast_saved(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let status = ast_save(&db, &w);
        w.set_ast_status(status);
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_deleted(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = w.get_ast_sel().parse::<i64>() else {
            return;
        };
        match db.soft_delete_assistant(id) {
            Ok(()) => ast_inject(&db, &w, &id.to_string()), // 留在原条目看「已删除」态
            Err(e) => w.set_ast_status(format!("删除失败: {e}").into()),
        }
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
    window.on_ast_purged(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = w.get_ast_sel().parse::<i64>() else {
            return;
        };
        match db.purge_assistant(id) {
            Ok(()) => {
                ast_inject(&db, &w, ""); // 条目已没了，回注入兜底选中默认助手
                w.set_ast_status("已彻底删除".into());
            }
            Err(e) => w.set_ast_status(format!("彻底删除失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_ast_defaulted(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let Ok(id) = w.get_ast_sel().parse::<i64>() else {
            return;
        };
        match db.set_default_assistant(id) {
            Ok(()) => {
                ast_inject(&db, &w, &id.to_string());
                w.set_ast_status("已设为默认，之后新建会话生效".into());
            }
            Err(e) => w.set_ast_status(format!("切换失败: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
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
        match Image::load_from_path(&path) {
            Ok(img) => {
                w.set_ast_avatar_img(img);
                w.set_ast_avatar(path.to_string_lossy().as_ref().into());
                w.set_ast_status("头像已载入，点「完成」保存".into());
            }
            // 默认解码仅 png/jpeg（slint std → image-decoders 实证），其余格式需先转换
            Err(e) => w.set_ast_status(format!("图片加载失败（支持 png/jpg）: {e}").into()),
        }
    });
    let window_weak = window.as_weak();
    window.on_ast_avatar_cleared(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        w.set_ast_avatar(SharedString::default());
        w.set_ast_avatar_img(Image::default());
        w.set_ast_status("头像已清除，点「完成」保存".into());
    });
}
