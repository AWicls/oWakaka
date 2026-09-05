//! 设置页「保存」：角色设定两页（助手提示词/温度、用户人设）→ persona 活跃行。
//! 提供商字段由提供商页即时落盘（见 [`super::prov`]），不经此。

use crate::db::Db;
use slint::SharedString;

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
