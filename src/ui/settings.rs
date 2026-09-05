//! 设置页「保存」：角色设定两页（助手提示词/温度、用户人设）→ persona 活跃行。
//! 提供商字段由提供商页即时落盘（见 [`super::prov`]），不经此。

use crate::db::Db;
use slint::{ComponentHandle, SharedString};

use super::AppWindow;

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

/// 设置页「保存」= 仅角色设定两页（助手提示词/温度、用户人设 → persona 活跃行）。
/// 提供商字段由提供商页即时落盘，不经此；温度非法直接回文案不落盘。
pub(super) fn save_personas(
    db: &Db,
    sys_prompt: &str,
    user_persona: &str,
    temp: &str,
) -> SharedString {
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

/// 设置整页接线：进入注入提供商档案 + persona 初值；返回重拉模型数据源；保存 = persona 两页。
pub(super) fn wire_settings(window: &AppWindow, ctx: &super::Ctx) {
    let window_weak = window.as_weak();
    let db = ctx.db.clone();
    window.on_settings_requested(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        super::prov::prov_inject(&db, &w, "");
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
    window.on_settings_saved(move |sys_prompt, user_persona, temp| {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let status = save_personas(&db, &sys_prompt, &user_persona, &temp);
        w.set_cfg_status(status);
        // 整页设置不自动跳回，留在本页回显成功/失败；用户点「返回」退出
    });
}
