//! 会话与消息：混合式表的读写面——会话头、消息续号、历史装载与回收站两级删除。
//!
//! 删除宪法（见 [`crate::db`] 模块头）在本模块落地：`soft_delete_session`（逻辑删，
//! 可 `restore_session`）→ `purge_session`（物理删，连带消息级联清除）。

use std::error::Error;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::{Db, NOW};

/// 消息的 JSON 叶子：只放不查询的展示态，全部字段可缺省（读旧行容错）。
///
/// 缺省语义 = "无思考、收起、未被手动干预"：
///
/// ```
/// use o_wakaka::db::MessagePayload;
///
/// let p: MessagePayload = serde_json::from_str(r#"{"tstate": 2}"#).unwrap();
/// assert_eq!((p.thinking.as_str(), p.tstate, p.tauto), ("", 2, true));
/// // 完全缺省走 Default（tauto=true 而非 bool 零值 false）
/// let d: MessagePayload = serde_json::from_str("{}").unwrap();
/// assert!(d.tauto && d.tstate == 0 && d.thinking.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessagePayload {
    /// 思考过程全文（仅 assistant；user 消息 payload 整体为 NULL）
    pub thinking: String,
    /// 思考展示态，与 `ui` 侧 `ChatMessage.tstate` 同义：0 收起 | 1 部分 | 2 全开
    pub tstate: i32,
    /// 未被手动干预：思考完成时"部分"态自动转收起
    pub tauto: bool,
}

impl Default for MessagePayload {
    fn default() -> Self {
        Self {
            thinking: String::new(),
            tstate: 0,
            tauto: true,
        }
    }
}

/// 读到的一条消息（启动回填气泡用；身份/序号列不外露，UI 不需要）。
#[derive(Debug, Clone)]
pub struct LoadedMessage {
    /// "user" | "assistant"
    pub role: String,
    /// 气泡正文
    pub content: String,
    /// 产出本条回答的模型 ID（user 消息为空串）
    pub model: String,
    /// JSON 叶子（思考全文与展示态）
    pub payload: MessagePayload,
}

/// 读到的一个会话（含按序消息）。
#[derive(Debug, Clone)]
pub struct LoadedSession {
    /// 行 id：UI 侧簿记与 [`Db::insert_message`] 的定位键
    pub id: i64,
    /// 侧栏标题
    pub title: String,
    /// 按会话内序号升序
    pub messages: Vec<LoadedMessage>,
}

impl Db {
    /// 新建会话，返回行 id。
    pub fn insert_session(&self, title: &str) -> Result<i64, Box<dyn Error>> {
        self.conn
            .execute("INSERT INTO session (title) VALUES (?1)", [title])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 重命名会话（首条消息摘要替换占位标题），同时刷新活跃时间。
    pub fn set_session_title(&self, id: i64, title: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            &format!("UPDATE session SET title = ?1, updated_at = {NOW} WHERE id = ?2"),
            rusqlite::params![title, id],
        )?;
        Ok(())
    }

    /// 写入一条消息：`seq` 会话内自动续号（乱序/重放保险丝），并 bump 会话活跃时间。
    ///
    /// `model` 仅 assistant 携带（产出一条回答所用）；user 消息传空串、`payload` 传 `None`。
    pub fn insert_message(
        &self,
        session_id: i64,
        role: &str,
        content: &str,
        model: &str,
        payload: Option<&MessagePayload>,
    ) -> Result<i64, Box<dyn Error>> {
        let payload_json = payload.map(serde_json::to_string).transpose()?;
        let seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM message WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO message (session_id, seq, role, content, model, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![session_id, seq, role, content, model, payload_json],
        )?;
        self.conn.execute(
            &format!("UPDATE session SET updated_at = {NOW} WHERE id = ?1"),
            [session_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 全量读取**未删除**会话：按活跃时间升序（重启后"最近用过的在最下"，与新建顺序一致），
    /// 消息按会话内序号升序。DB-1 不分页。
    pub fn load_all(&self) -> Result<Vec<LoadedSession>, Box<dyn Error>> {
        let heads: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, title FROM session WHERE deleted_at IS NULL ORDER BY updated_at, id",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut sessions = Vec::with_capacity(heads.len());
        for (id, title) in heads {
            sessions.push(LoadedSession {
                id,
                title,
                messages: self.messages_of(id)?,
            });
        }
        Ok(sessions)
    }

    /// 单会话消息按 seq 升序。
    fn messages_of(&self, session_id: i64) -> Result<Vec<LoadedMessage>, Box<dyn Error>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, model, payload FROM message WHERE session_id = ?1 ORDER BY seq",
        )?;
        stmt.query_map([session_id], |r| {
            let raw: Option<String> = r.get(3)?;
            // 坏 payload 回退缺省值：读历史永不炸
            let payload = raw
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            Ok(LoadedMessage {
                role: r.get(0)?,
                content: r.get(1)?,
                model: r.get(2)?,
                payload,
            })
        })?
        .collect::<Result<_, _>>()
        .map_err(Into::into)
    }

    /// 回收站列表：已逻辑删除的会话 (id, title)，按 id 升序。
    pub fn list_deleted(&self) -> Result<Vec<(i64, String)>, Box<dyn Error>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title FROM session WHERE deleted_at IS NOT NULL ORDER BY id")?;
        Ok(stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    /// 装载单个会话（含消息）；不存在回 `None`。回收站恢复推入侧栏用。
    pub fn load_session(&self, id: i64) -> Result<Option<LoadedSession>, Box<dyn Error>> {
        let Some(title) = self
            .conn
            .query_row("SELECT title FROM session WHERE id = ?1", [id], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
        else {
            return Ok(None);
        };
        Ok(Some(LoadedSession {
            id,
            title,
            messages: self.messages_of(id)?,
        }))
    }

    /// 逻辑删除会话：置 `deleted_at`，随时可 [`restore_session`](Self::restore_session) 找回。
    pub fn soft_delete_session(&self, id: i64) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            &format!("UPDATE session SET deleted_at = {NOW} WHERE id = ?1"),
            [id],
        )?;
        Ok(())
    }

    /// 恢复逻辑删除的会话。
    pub fn restore_session(&self, id: i64) -> Result<(), Box<dyn Error>> {
        self.conn
            .execute("UPDATE session SET deleted_at = NULL WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 彻底删除：物理清除会话与其全部消息（不可找回）。
    ///
    /// 两级删除闭环可离线验证：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// let sid = db.insert_session("要删的")?;
    /// db.insert_message(sid, "user", "hi", "", None)?;
    ///
    /// db.soft_delete_session(sid)?;
    /// assert!(db.load_all()?.is_empty(), "逻辑删除后列表不可见");
    ///
    /// db.restore_session(sid)?;
    /// assert_eq!(db.load_all()?[0].messages.len(), 1, "恢复后消息还在");
    ///
    /// db.soft_delete_session(sid)?;
    /// db.purge_session(sid)?;
    /// assert!(db.load_all()?.is_empty());
    /// // 行与消息都物理清除（无 FK 残留可插新消息验证会话确实没了）
    /// assert!(db.insert_message(sid, "user", "x", "", None).is_err());
    /// # Ok(()) }
    /// ```
    pub fn purge_session(&self, id: i64) -> Result<(), Box<dyn Error>> {
        self.conn
            .execute("DELETE FROM message WHERE session_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM session WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_deleted_and_load_session_for_trash() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let keep = db.insert_session("留")?;
        let gone = db.insert_session("删")?;
        db.insert_message(gone, "user", "hi", "", None)?;
        assert!(db.list_deleted()?.is_empty());

        db.soft_delete_session(gone)?;
        assert_eq!(db.list_deleted()?, vec![(gone, "删".to_string())]);
        // 未删的 load_session 也可读（供恢复推入）；已删同样能读出（恢复语义）
        let s = db.load_session(gone)?.unwrap();
        assert_eq!((s.id, s.messages.len()), (gone, 1));
        assert_eq!(db.load_session(keep)?.unwrap().messages.len(), 0);
        assert!(db.load_session(999)?.is_none());

        db.purge_session(gone)?;
        assert!(db.list_deleted()?.is_empty());
        Ok(())
    }

    #[test]
    fn seq_is_per_session_and_order_preserved() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let a = db.insert_session("a")?;
        let b = db.insert_session("b")?;
        db.insert_message(a, "user", "x", "", None)?;
        db.insert_message(a, "assistant", "y", "m1", None)?;
        db.insert_message(b, "user", "z", "", None)?;
        let all = db.load_all()?;
        // 同毫秒新建时按 id 稳定排序：a 在前
        assert_eq!((all[0].title.as_str(), all[1].title.as_str()), ("a", "b"));
        let contents: Vec<&str> = all[0].messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["x", "y"]);
        assert_eq!(all[0].messages[1].model, "m1");
        assert_eq!(all[1].messages.len(), 1);
        Ok(())
    }
}
