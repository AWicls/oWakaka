//! 设置域装配：整页进出（注入提供商/助手档案与用户人设初值）、用户设定页自动保存，
//! 兼头像小工具（色板/首字/加载——聊天卡头栏与 AI 助手页共用）。
//! LLM 提供商页见 [`prov`]，AI 助手页见 [`ast`]。

pub(super) mod ast;
pub(super) mod prov;

pub(in crate::ui) use prov::{on_fetch_result, on_test_result};

use crate::db::Db;
use slint::{Color, ComponentHandle, Image, SharedString};

use super::AppWindow;

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

pub(super) fn avatar_color(id: i64) -> Color {
    let (r, g, b) = AVATAR_PALETTE[id.rem_euclid(AVATAR_PALETTE.len() as i64) as usize];
    Color::from_rgb_u8(r, g, b)
}

/// 名称首字（Slint string 无 substring，Rust 侧算好注入）；空白名回退「?」。
pub(super) fn initial_of(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |c| c.to_string())
}

/// 头像图片加载：空路径/加载失败一律回退空图（Slint 侧按 `width == 0` 判无图画圆底首字）。
pub(super) fn load_avatar(path: &str) -> Image {
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

/// 用户设定页自动保存 = 全局用户人设（persona user 活跃行；助手字段已迁 AI 助手页）。
pub(super) fn save_user_persona(db: &Db, user_persona: &str) -> SharedString {
    match db.upsert_active_persona("user", "默认人设", user_persona.trim(), None) {
        Ok(()) => "已自动保存，下一次发送生效".into(),
        Err(e) => format!("保存失败: {e}").into(),
    }
}

/// 设置整页接线：进入注入提供商/助手档案 + 用户人设初值；返回重拉模型数据源；
/// 用户设定页保存 = 全局人设。AI 助手页操作全部在 [`ast::wire_assist`] 接线。
pub(super) fn wire_settings(window: &AppWindow, ctx: &super::Ctx) {
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_settings_requested(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        prov::prov_inject(&db, &w, "");
        w.set_prov_status(SharedString::default());
        ast::ast_inject(&db, &w, "");
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
    let host = ctx.host.clone();
    window.on_settings_back(move || {
        if let Some(w) = window_weak.upgrade() {
            w.set_settings_page(false);
            // 提供商/模型/能力可能在页内改过：回对话前重拉一次数据源
            models.sync_store(&w);
            // 当前会话助手的名/头像可能改过：刷新聊天卡头栏
            host.sync_assistant_header(&w);
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
        // 用户人设为逐键自动保存触发：只回显状态，不跳页；用户点「返回」退出
    });
    ast::wire_assist(window, ctx);
}
