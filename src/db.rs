//! 数据库层（SQLite，经 rusqlite bundled）：对话会话/消息 + 配置存储。
//!
//! # 设计约定（2026-09-05 规划讨论定案）
//! - **混合式表**：要 WHERE/ORDER BY/建索引的字段成列，其余进 `payload` JSON 叶子；
//!   叶子加键不是迁移。列默认值与语义见建表 SQL。
//! - **开发期破坏策略**：`user_version` 与 [`SCHEMA_VERSION`] 不一致即整库重建
//!   （数据出现"要留"价值后废除本策略，改用逐级 backfill 迁移）。
//! - **时间戳**：由 SQLite 生成，RFC3339 UTC 带毫秒，字符串即可排序。
//! - **线程模型**：主线程持单连接同步读写（桌面单用户，操作毫秒级），
//!   故所有 `&self` 方法可被 UI 闭包自由共享（`Rc<Db>`）。
//! - **预埋**：`session.persona_id`（DB-3 角色设定）、`message.origin`（P2 A2A
//!   外部来路）当前恒为默认值，占列免将来 ALTER。
//! - **kv 表**（DB-2）：小配置 JSON 一站一值（如 `config` = 运行配置，密钥除外，
//!   见 [`crate::ai::config`]）；`CHECK(json_valid)` 挡住误写的裸字符串。

use std::{error::Error, fs, path::Path};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 当前 schema 版本。改动表结构 = 此数 +1，旧库整库重建（开发期策略）。
pub const SCHEMA_VERSION: i32 = 2;

/// SQLite 生成的"现在"：RFC3339 UTC 带毫秒（strftime 的 %f 输出 SS.mmm）。
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

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

/// 数据库句柄：打开即完成建目录、PRAGMA、版本检查与建表。
///
/// 文件库往返 = 模拟"重启不丢"：
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use o_wakaka::db::{Db, MessagePayload};
///
/// let path = std::env::temp_dir().join("o_wakaka_db_doctest.db");
/// std::fs::remove_file(&path).ok();
/// {
///     let db = Db::open(&path)?;
///     let sid = db.insert_session("新对话")?;
///     db.insert_message(sid, "user", "你好", "", None)?;
///     db.insert_message(
///         sid,
///         "assistant",
///         "嗨",
///         "gpt-4o-mini",
///         Some(&MessagePayload { thinking: "想了想".into(), tstate: 0, tauto: true }),
///     )?;
///     db.set_session_title(sid, "你好")?;
/// } // 连接关闭 = 应用重启
///
/// let db = Db::open(&path)?;
/// let all = db.load_all()?;
/// std::fs::remove_file(&path).ok();
///
/// assert_eq!(all.len(), 1);
/// assert_eq!(all[0].title, "你好");
/// let msgs = &all[0].messages;
/// assert_eq!(msgs.len(), 2);
/// assert_eq!((msgs[1].role.as_str(), msgs[1].model.as_str()), ("assistant", "gpt-4o-mini"));
/// assert_eq!(msgs[1].payload.thinking, "想了想");
/// # Ok(()) }
/// ```
pub struct Db {
    conn: Connection,
}

impl Db {
    /// 打开（必要时创建）`path` 的数据库；父目录不存在则建。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        if let Some(dir) = path.as_ref().parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        Self::setup(Connection::open(path)?)
    }

    /// 内存库：与文件库同 schema 同行为（doctest/单测用，进程退出即灭）。
    pub fn open_in_memory() -> Result<Self, Box<dyn Error>> {
        Self::setup(Connection::open_in_memory()?)
    }

    /// 打开后的统一初始化：PRAGMA → 版本检查（不匹配即重建）。
    fn setup(conn: Connection) -> Result<Self, Box<dyn Error>> {
        conn.busy_timeout(std::time::Duration::from_millis(3000))?;
        // 值即结果行（内存库回 "memory"），忽略即可
        let _ = conn.execute("PRAGMA journal_mode=WAL", []);
        conn.execute("PRAGMA foreign_keys=ON", [])?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            Self::create_schema(&conn)?;
            conn.execute(&format!("PRAGMA user_version={SCHEMA_VERSION}"), [])?;
        }
        Ok(Self { conn })
    }

    /// 建表（先 DROP 后 CREATE：开发期重建 = 直接调它，无 backfill）。
    fn create_schema(conn: &Connection) -> Result<(), Box<dyn Error>> {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS message;
             DROP TABLE IF EXISTS session;
             DROP TABLE IF EXISTS kv;
             CREATE TABLE kv (
               key        TEXT PRIMARY KEY,
               value      TEXT NOT NULL CHECK (json_valid(value)),
               updated_at TEXT NOT NULL DEFAULT ({NOW})
             );
             CREATE TABLE session (
               id           INTEGER PRIMARY KEY,
               title        TEXT    NOT NULL DEFAULT '',
               model        INTEGER NOT NULL DEFAULT -1,
               persona_id   INTEGER,
               created_at   TEXT    NOT NULL DEFAULT ({NOW}),
               updated_at   TEXT    NOT NULL DEFAULT ({NOW})
             );
             CREATE INDEX session_activity ON session(updated_at, id);
             CREATE TABLE message (
               id         INTEGER PRIMARY KEY,
               session_id INTEGER NOT NULL REFERENCES session(id),
               seq        INTEGER NOT NULL,
               role       TEXT    NOT NULL,
               content    TEXT    NOT NULL DEFAULT '',
               model      TEXT    NOT NULL DEFAULT '',
               origin     TEXT    NOT NULL DEFAULT 'local',
               payload    TEXT    CHECK (payload IS NULL OR json_valid(payload)),
               created_at TEXT    NOT NULL DEFAULT ({NOW}),
               updated_at TEXT    NOT NULL DEFAULT ({NOW}),
               UNIQUE (session_id, seq)
             );"
        ))?;
        Ok(())
    }

    /// 读 kv 一站：未存过为 `None`；值是 JSON 文本（写入侧有 CHECK 保证）。
    ///
    /// 存取自成一体，覆盖即 upsert，非 JSON 值被 CHECK 拒绝：
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use o_wakaka::db::Db;
    ///
    /// let db = Db::open_in_memory()?;
    /// assert_eq!(db.kv_get("config")?, None);
    /// db.kv_set("config", r#"{"model":"m1"}"#)?;
    /// assert_eq!(db.kv_get("config")?.as_deref(), Some(r#"{"model":"m1"}"#));
    /// db.kv_set("config", r#"{"model":"m2"}"#)?;
    /// assert_eq!(db.kv_get("config")?.as_deref(), Some(r#"{"model":"m2"}"#));
    /// assert!(db.kv_set("config", "not json").is_err());
    /// # Ok(()) }
    /// ```
    pub fn kv_get(&self, key: &str) -> Result<Option<String>, Box<dyn Error>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    /// 写 kv 一站（value 必须是合法 JSON 文本；upsert 同时刷新 updated_at）。
    pub fn kv_set(&self, key: &str, value: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute(
            &format!(
                "INSERT INTO kv (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = {NOW}"
            ),
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

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

    /// 全量读取：会话按活跃时间升序（重启后"最近用过的在最下"，与新建顺序一致），
    /// 消息按会话内序号升序。DB-1 不分页。
    pub fn load_all(&self) -> Result<Vec<LoadedSession>, Box<dyn Error>> {
        let heads: Vec<(i64, String)> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id, title FROM session ORDER BY updated_at, id")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut sessions = Vec::with_capacity(heads.len());
        for (id, title) in heads {
            let mut stmt = self.conn.prepare(
                "SELECT role, content, model, payload FROM message
                 WHERE session_id = ?1 ORDER BY seq",
            )?;
            let messages = stmt
                .query_map([id], |r| {
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
                .collect::<Result<Vec<_>, _>>()?;
            sessions.push(LoadedSession {
                id,
                title,
                messages,
            });
        }
        Ok(sessions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_schema_is_rebuilt_not_migrated() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let sid = db.insert_session("t")?;
        db.insert_message(sid, "user", "hi", "", None)?;
        assert_eq!(db.load_all()?.len(), 1);

        // 模拟"版本不匹配的库"：改 user_version 后交回 setup → 清表重建
        let Db { conn } = db;
        conn.execute("PRAGMA user_version=999", [])?;
        let rebuilt = Db::setup(conn)?;
        assert!(rebuilt.load_all()?.is_empty());
        let version: i32 = rebuilt
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        assert_eq!(version, SCHEMA_VERSION);
        // 重建后 schema 完整可用
        let sid = rebuilt.insert_session("fresh")?;
        rebuilt.insert_message(sid, "user", "ok", "", None)?;
        assert_eq!(rebuilt.load_all()?[0].messages.len(), 1);
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
