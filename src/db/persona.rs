//! 角色设定（persona 表，DB-3）：一 kind 一活跃行的 system prompt / 温度存取。
//!
//! 表 DDL 与 `is_active` 约束见 [`crate::db`]（`PERSONA_DDL`）；本模块只负责活跃行的读与 upsert。

use std::error::Error;

use rusqlite::OptionalExtension;

use super::{Db, NOW};

/// 一行角色设定（persona 表活跃行投影；payload 列暂不露出）。
#[derive(Debug, Clone, PartialEq)]
pub struct Persona {
    /// 行 id
    pub id: i64,
    /// "user" | "assistant"
    pub kind: String,
    /// 显示名（当前恒"默认…"，多人格管理再启用）
    pub name: String,
    /// 系统提示词（assistant）/ 人设描述（user）；空 = 不注入
    pub system_prompt: String,
    /// 生成温度；`None` = 未设定，请求不发 temperature 字段
    pub temperature: Option<f64>,
}

impl Db {
    /// 某 kind 的活跃角色设定行；未设定过为 `None`。
    pub fn active_persona(&self, kind: &str) -> Result<Option<Persona>, Box<dyn Error>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, kind, name, system_prompt, temperature
                 FROM persona WHERE kind = ?1 AND is_active = 1 ORDER BY id LIMIT 1",
                [kind],
                |r| {
                    Ok(Persona {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        name: r.get(2)?,
                        system_prompt: r.get(3)?,
                        temperature: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// 保存角色设定：有活跃行即更新，无则建为活跃（一 kind 恒一行）。
    ///
    /// 往返与"未设定 = NULL"可离线验证：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// assert_eq!(db.active_persona("assistant")?, None);
    /// db.upsert_active_persona("assistant", "默认助手", "你言简意赅", Some(0.3))?;
    /// let p = db.active_persona("assistant")?.unwrap();
    /// assert_eq!((p.name.as_str(), p.system_prompt.as_str(), p.temperature),
    ///            ("默认助手", "你言简意赅", Some(0.3)));
    /// // 再存覆盖同一行（id 不变），温度可清空回未设定
    /// db.upsert_active_persona("assistant", "默认助手", "你罗嗦", None)?;
    /// let p2 = db.active_persona("assistant")?.unwrap();
    /// assert_eq!((p2.id, p2.system_prompt.as_str(), p2.temperature), (p.id, "你罗嗦", None));
    /// // user kind 独立成行，未存过仍是 None
    /// assert_eq!(db.active_persona("user")?, None);
    /// # Ok(()) }
    /// ```
    pub fn upsert_active_persona(
        &self,
        kind: &str,
        name: &str,
        system_prompt: &str,
        temperature: Option<f64>,
    ) -> Result<(), Box<dyn Error>> {
        let updated = self.conn.execute(
            &format!(
                "UPDATE persona SET name = ?1, system_prompt = ?2, temperature = ?3,
                 updated_at = {NOW} WHERE kind = ?4 AND is_active = 1"
            ),
            rusqlite::params![name, system_prompt, temperature, kind],
        )?;
        if updated == 0 {
            self.conn.execute(
                "INSERT INTO persona (kind, name, system_prompt, temperature, is_active)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![kind, name, system_prompt, temperature],
            )?;
        }
        Ok(())
    }
}
