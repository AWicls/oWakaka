//! 角色设定（persona 表，DB-3）：用户人设单活跃行 + 多助手（默认助手恒一行）。
//!
//! 表 DDL 见 [`crate::db`]（`PERSONA_DDL`）。混合式表约定：头像/开场白/默认模型/
//! 逻辑删除标记等不进 WHERE 的字段放 `payload` JSON 叶子（加键不是迁移）。
//! assistant kind 允许多行，`is_active = 1` 恒恰一行 = **默认助手**（不可删除，
//! 缺失时 [`Db::default_assistant`] 懒播种/自愈）；user kind 仍一活跃行。

use std::error::Error;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

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

/// 助手行的 JSON 叶子：只放不进 WHERE/索引的字段；缺键容错（读旧行回缺省）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct AssistantPayload {
    /// 头像本地图片路径；空 = UI 画纯色圆底 + 名称首字
    avatar: String,
    /// 开场白：用该助手建会话时持久化为第一条 assistant 消息；空 = 不插
    opening: String,
    /// 默认模型 id（取自当前激活提供商清单）；空 = 建会话不切模型
    model: String,
    /// 逻辑删除标记（删除宪法：灰显可恢复；彻底删除走物理 DELETE）
    deleted: bool,
}

/// 一个 AI 助手（kind='assistant' 行 + payload 叶子展开）。
#[derive(Debug, Clone, PartialEq)]
pub struct Assistant {
    /// 行 id（`session.persona_id` 引用；助手被彻底删除后旧会话读侧回落默认助手）
    pub id: i64,
    /// 显示名
    pub name: String,
    /// 系统提示词（每次发送前置注入）；空 = 不注入
    pub system_prompt: String,
    /// 生成温度；`None` = 未设定，请求不发 temperature 字段
    pub temperature: Option<f64>,
    /// 头像本地图片路径；空 = 圆底 + 首字
    pub avatar: String,
    /// 开场白；空 = 建会话不插
    pub opening: String,
    /// 默认模型 id；空 = 建会话不切模型
    pub model: String,
    /// 是否全局默认助手（is_active=1；恒恰一行，不可删）
    pub is_default: bool,
    /// 逻辑删除标记
    pub deleted: bool,
}

/// 助手投影统一 SELECT 列序：id, name, system_prompt, temperature, payload, is_active。
const ASSISTANT_COLS: &str = "id, name, system_prompt, temperature, payload, is_active";

/// SQL 谓词：行未被逻辑删除（payload 缺省/坏 JSON 视为未删）。
const NOT_DELETED: &str = "COALESCE(json_extract(payload, '$.deleted'), 0) = 0";

fn row_to_assistant(r: &rusqlite::Row<'_>) -> rusqlite::Result<Assistant> {
    let raw: Option<String> = r.get(4)?;
    // 坏 payload 回退缺省值：读助手永不炸
    let p: AssistantPayload = raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Ok(Assistant {
        id: r.get(0)?,
        name: r.get(1)?,
        system_prompt: r.get(2)?,
        temperature: r.get(3)?,
        avatar: p.avatar,
        opening: p.opening,
        model: p.model,
        is_default: r.get::<_, i64>(5)? != 0,
        deleted: p.deleted,
    })
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

    /// 保存角色设定：有活跃行即更新，无则建为活跃（user kind 恒一行；
    /// assistant 的活跃行 = 默认助手，多助手管理走 [`Db::save_assistant`] 一族）。
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

    /// 全部助手行（含逻辑删除项，设置页列表用），按 id 升序。
    pub fn assistants(&self) -> Result<Vec<Assistant>, Box<dyn Error>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ASSISTANT_COLS} FROM persona WHERE kind = 'assistant' ORDER BY id"
        ))?;
        Ok(stmt
            .query_map([], row_to_assistant)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// 按 id 读单个助手（含逻辑删除项）；不存在回 `None`。
    pub fn assistant(&self, id: i64) -> Result<Option<Assistant>, Box<dyn Error>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ASSISTANT_COLS} FROM persona WHERE kind = 'assistant' AND id = ?1"
                ),
                [id],
                row_to_assistant,
            )
            .optional()?)
    }

    /// 全局默认助手；恒有值（懒播种/自愈，可离线验证）：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// // 空库即播种
    /// let d = db.default_assistant()?;
    /// assert_eq!(d.name, "默认助手");
    /// assert!(d.is_default && !d.deleted);
    /// // 幂等：再读同一行
    /// assert_eq!(db.default_assistant()?.id, d.id);
    /// # Ok(()) }
    /// ```
    ///
    /// 自愈两级：有未删助手但无活跃行 → 最早一行升为默认；全被逻辑删除 → 播种新行。
    pub fn default_assistant(&self) -> Result<Assistant, Box<dyn Error>> {
        if let Some(a) = self
            .conn
            .query_row(
                &format!(
                    "SELECT {ASSISTANT_COLS} FROM persona
                     WHERE kind = 'assistant' AND is_active = 1 AND {NOT_DELETED}
                     ORDER BY id LIMIT 1"
                ),
                [],
                row_to_assistant,
            )
            .optional()?
        {
            return Ok(a);
        }
        // MIN(id) 恒回一行（空集 = NULL 行），故取 Option 而非 .optional()
        let oldest: Option<i64> = self.conn.query_row(
            &format!("SELECT MIN(id) FROM persona WHERE kind = 'assistant' AND {NOT_DELETED}"),
            [],
            |r| r.get::<_, Option<i64>>(0),
        )?;
        if let Some(id) = oldest {
            self.conn.execute(
                "UPDATE persona SET is_active = 0 WHERE kind = 'assistant'",
                [],
            )?;
            self.conn
                .execute("UPDATE persona SET is_active = 1 WHERE id = ?1", [id])?;
            if let Some(a) = self.assistant(id)? {
                return Ok(a);
            }
        }
        self.conn.execute(
            "INSERT INTO persona (kind, name, is_active) VALUES ('assistant', '默认助手', 1)",
            [],
        )?;
        self.assistant(self.conn.last_insert_rowid())?
            .ok_or_else(|| "默认助手播种失败".into())
    }

    /// 新建自定义助手（非默认，仅名称，其余字段空），返回行 id。
    pub fn insert_assistant(&self, name: &str) -> Result<i64, Box<dyn Error>> {
        self.conn.execute(
            "INSERT INTO persona (kind, name) VALUES ('assistant', ?1)",
            [name],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 按 id 整行保存助手（名称/提示词/温度 + payload 叶子）；id 不存在回 `Err`。
    /// 删除态转换请走 [`soft_delete_assistant`](Self::soft_delete_assistant) 一族，
    /// 本方法照写 `deleted` 字段但不校验默认助手保护。
    pub fn save_assistant(&self, a: &Assistant) -> Result<(), Box<dyn Error>> {
        let payload = serde_json::to_string(&AssistantPayload {
            avatar: a.avatar.clone(),
            opening: a.opening.clone(),
            model: a.model.clone(),
            deleted: a.deleted,
        })?;
        let n = self.conn.execute(
            &format!(
                "UPDATE persona SET name = ?1, system_prompt = ?2, temperature = ?3,
                 payload = ?4, updated_at = {NOW} WHERE kind = 'assistant' AND id = ?5"
            ),
            rusqlite::params![a.name, a.system_prompt, a.temperature, payload, a.id],
        )?;
        if n == 0 {
            return Err("助手不存在".into());
        }
        Ok(())
    }

    /// 切换默认助手（事务内先全清再置一，恒恰一行）；目标不存在或已逻辑删除回 `Err`。
    pub fn set_default_assistant(&self, id: i64) -> Result<(), Box<dyn Error>> {
        let n: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM persona
                 WHERE kind = 'assistant' AND id = ?1 AND {NOT_DELETED}"
            ),
            [id],
            |r| r.get(0),
        )?;
        if n == 0 {
            return Err("目标助手不存在或已删除".into());
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE persona SET is_active = 0 WHERE kind = 'assistant'",
            [],
        )?;
        tx.execute(
            &format!("UPDATE persona SET is_active = 1, updated_at = {NOW} WHERE id = ?1"),
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 逻辑删除助手（payload 叶子置 deleted）；默认助手不可删。
    pub fn soft_delete_assistant(&self, id: i64) -> Result<(), Box<dyn Error>> {
        let Some(a) = self.assistant(id)? else {
            return Err("助手不存在".into());
        };
        if a.is_default {
            return Err("默认助手不可删除".into());
        }
        if a.deleted {
            return Ok(());
        }
        self.save_assistant(&Assistant { deleted: true, ..a })
    }

    /// 恢复逻辑删除的助手。
    pub fn restore_assistant(&self, id: i64) -> Result<(), Box<dyn Error>> {
        let Some(a) = self.assistant(id)? else {
            return Err("助手不存在".into());
        };
        if !a.deleted {
            return Ok(());
        }
        self.save_assistant(&Assistant {
            deleted: false,
            ..a
        })
    }

    /// 彻底删除助手（物理 DELETE，不可找回）；默认助手不可删。
    /// 引用它的旧会话 `persona_id` 悬空，读侧回落默认助手。
    pub fn purge_assistant(&self, id: i64) -> Result<(), Box<dyn Error>> {
        let Some(a) = self.assistant(id)? else {
            return Ok(());
        };
        if a.is_default {
            return Err("默认助手不可删除".into());
        }
        self.conn.execute(
            "DELETE FROM persona WHERE kind = 'assistant' AND id = ?1",
            [id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_crud_and_two_level_delete() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let d = db.default_assistant()?;
        let id = db.insert_assistant("翻译官")?;
        let mut a = db.assistant(id)?.unwrap();
        assert!(!a.is_default && !a.deleted);
        a.system_prompt = "你只输出译文".into();
        a.temperature = Some(0.2);
        a.avatar = "C:/pic.png".into();
        a.opening = "你好，请发要翻译的文本".into();
        a.model = "mimo-v2.5".into();
        db.save_assistant(&a)?;
        let a2 = db.assistant(id)?.unwrap();
        assert_eq!(
            (
                a2.name.as_str(),
                a2.system_prompt.as_str(),
                a2.opening.as_str(),
                a2.model.as_str(),
                a2.avatar.as_str(),
                a2.temperature
            ),
            (
                "翻译官",
                "你只输出译文",
                "你好，请发要翻译的文本",
                "mimo-v2.5",
                "C:/pic.png",
                Some(0.2)
            )
        );
        assert_eq!(db.assistants()?.len(), 2);
        assert!(db.assistant(999)?.is_none());
        assert!(
            db.save_assistant(&Assistant {
                id: 999,
                ..a2.clone()
            })
            .is_err()
        );

        // 默认切换恒恰一行；已删/不存在不可为默认
        db.set_default_assistant(id)?;
        assert!(db.assistant(id)?.unwrap().is_default);
        assert!(!db.assistant(d.id)?.unwrap().is_default);
        assert_eq!(db.default_assistant()?.id, id);
        assert!(db.set_default_assistant(999).is_err());

        // 默认不可删（两级都不可）；换回默认后自定义可 软删→恢复→彻底删
        assert!(db.soft_delete_assistant(id).is_err());
        assert!(db.purge_assistant(id).is_err());
        db.set_default_assistant(d.id)?;
        db.soft_delete_assistant(id)?;
        assert!(db.assistant(id)?.unwrap().deleted);
        assert_eq!(db.assistants()?.iter().filter(|x| !x.deleted).count(), 1);
        assert!(db.set_default_assistant(id).is_err(), "已删助手不可为默认");
        db.restore_assistant(id)?;
        assert!(!db.assistant(id)?.unwrap().deleted);
        db.purge_assistant(id)?;
        assert!(db.assistant(id)?.is_none());
        Ok(())
    }

    #[test]
    fn default_assistant_self_heals() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let d = db.default_assistant()?;
        let other = db.insert_assistant("b")?;

        // 无活跃行但有未删助手 → 最早一行升为默认
        db.conn.execute("UPDATE persona SET is_active = 0", [])?;
        let healed = db.default_assistant()?;
        assert_eq!(healed.id, d.id);
        assert!(healed.is_default && !db.assistant(other)?.unwrap().is_default);

        // 全部逻辑删除 → 播种新默认助手（旧行保留在列表）
        db.conn
            .execute("UPDATE persona SET payload = '{\"deleted\": true}'", [])?;
        let seeded = db.default_assistant()?;
        assert_ne!(seeded.id, d.id);
        assert_eq!(seeded.name, "默认助手");
        assert_eq!(db.assistants()?.len(), 3);
        Ok(())
    }
}
