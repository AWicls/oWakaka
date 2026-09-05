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
//! - **预埋**：`message.origin`（P2 A2A 外部来路）当前恒为默认值，占列免将来 ALTER。
//! - **kv 表**（DB-2）：小配置 JSON 一站一值（如 `providers` = 多提供商档案，密钥除外，
//!   见 [`crate::ai::config`]）；`CHECK(json_valid)` 挡住误写的裸字符串。
//! - **persona 表**（DB-3）：角色设定——user 人设一活跃行；assistant 多助手，
//!   `is_active=1` 恒一行 = 默认助手，`session.persona_id` 建会话时绑定。
//! - **删除宪法**：一切"删除"都是两级——先**逻辑删除**（`session.deleted_at` 置时间戳 /
//!   JSON 条目 `deleted` 标记，可恢复），再经显式**彻底删除**物理清除。列表读取恒过滤
//!   逻辑删除项。persona/kv 无删除场景；message 随所属会话级联清除。
//!
//! # 文件结构
//! - `db.rs`（本文件）：句柄与 schema——open / PRAGMA / 版本策略 / 建表
//! - `db/kv.rs`：kv 一站配置存取
//! - `db/persona.rs`：用户人设活跃行 + 多助手 CRUD（默认助手恒一行）
//! - `db/session.rs`：会话与消息、回收站两级删除

mod kv;
mod persona;
mod session;

use std::{error::Error, fs, path::Path};

use rusqlite::Connection;

// 领域类型仍从 db 命名空间露出：外部 use 路径不因拆分而变
pub use persona::{Assistant, Persona};
pub use session::{LoadedMessage, LoadedSession, MessagePayload};

/// 当前 schema 版本。改动表结构 = 此数 +1，旧库整库重建（开发期策略；
/// 纯加列/加表可像 v2→v3、v3→v4 这样留补结构例外，保住已有数据）。
pub const SCHEMA_VERSION: i32 = 4;

/// SQLite 生成的"现在"：RFC3339 UTC 带毫秒（strftime 的 %f 输出 SS.mmm）。
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// persona 表 DDL（v3 起，DB-3 角色设定）：user kind 一活跃行；assistant kind 多行，
/// `is_active=1` 恒一行 = 默认助手（`session.persona_id` 绑定，头像/开场白/默认模型/
/// 软删标记在 payload 叶子）。温度 NULL = 未设定（请求不发字段）。
const PERSONA_DDL: &str = "CREATE TABLE IF NOT EXISTS persona (
               id            INTEGER PRIMARY KEY,
               kind          TEXT    NOT NULL CHECK (kind IN ('user','assistant')),
               name          TEXT    NOT NULL DEFAULT '',
               system_prompt TEXT    NOT NULL DEFAULT '',
               temperature   REAL,
               payload       TEXT    CHECK (payload IS NULL OR json_valid(payload)),
               is_active     INTEGER NOT NULL DEFAULT 0,
               created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
               updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             );";

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
///     let sid = db.insert_session("新对话", None)?;
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

    /// 打开后的统一初始化：PRAGMA → 版本检查（不匹配重建；纯补表版本走例外）。
    fn setup(conn: Connection) -> Result<Self, Box<dyn Error>> {
        conn.busy_timeout(std::time::Duration::from_millis(3000))?;
        // 值即结果行（内存库回 "memory"），忽略即可
        let _ = conn.execute("PRAGMA journal_mode=WAL", []);
        conn.execute("PRAGMA foreign_keys=ON", [])?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            // 一次性例外：v2/v3 纯补结构（v2 补 persona 表、v2-v3 补 session.deleted_at
            // 列），保住已有数据；其余任何不匹配仍按开发期策略整库重建
            if version == 2 || version == 3 {
                if version == 2 {
                    conn.execute_batch(PERSONA_DDL)?;
                }
                if !Self::has_column(&conn, "session", "deleted_at")? {
                    conn.execute_batch("ALTER TABLE session ADD COLUMN deleted_at TEXT;")?;
                }
            } else {
                Self::create_schema(&conn)?;
            }
            conn.execute(&format!("PRAGMA user_version={SCHEMA_VERSION}"), [])?;
        }
        Ok(Self { conn })
    }

    /// 表是否已有某列（幂等 ALTER 用：升级对任意中间态安全）
    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, Box<dyn Error>> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;
        Ok(names.iter().any(|n| n == column))
    }

    /// 建表（先 DROP 后 CREATE：开发期重建 = 直接调它，无 backfill）。
    fn create_schema(conn: &Connection) -> Result<(), Box<dyn Error>> {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS message;
             DROP TABLE IF EXISTS session;
             DROP TABLE IF EXISTS kv;
             DROP TABLE IF EXISTS persona;
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
               updated_at   TEXT    NOT NULL DEFAULT ({NOW}),
               deleted_at   TEXT
             );
             CREATE INDEX session_activity ON session(updated_at, id);"
        ))?;
        conn.execute_batch(&format!(
            "CREATE TABLE message (
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
             );
             {PERSONA_DDL}"
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_schema_is_rebuilt_not_migrated() -> Result<(), Box<dyn Error>> {
        let db = Db::open_in_memory()?;
        let sid = db.insert_session("t", None)?;
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
        let sid = rebuilt.insert_session("fresh", None)?;
        rebuilt.insert_message(sid, "user", "ok", "", None)?;
        assert_eq!(rebuilt.load_all()?[0].messages.len(), 1);
        Ok(())
    }

    #[test]
    fn v2_v3_upgrades_add_missing_structure_only() -> Result<(), Box<dyn Error>> {
        // v2 库：补 persona 表 + session.deleted_at 列，数据保留、补的列即刻可用
        let db = Db::open_in_memory()?;
        let sid = db.insert_session("old", None)?;
        db.insert_message(sid, "user", "hi", "", None)?;
        let Db { conn } = db;
        conn.execute("PRAGMA user_version=2", [])?;
        let upgraded = Db::setup(conn)?;
        assert_eq!(upgraded.load_all()?[0].title, "old");
        upgraded.upsert_active_persona("assistant", "默认助手", " prompt ", Some(0.5))?;
        let p = upgraded.active_persona("assistant")?.unwrap();
        assert_eq!(
            (p.system_prompt.as_str(), p.temperature),
            (" prompt ", Some(0.5))
        );
        upgraded.soft_delete_session(sid)?;
        assert!(upgraded.load_all()?.is_empty());

        // v3 库：只补列
        let db = Db::open_in_memory()?;
        let sid = db.insert_session("keep", None)?;
        db.insert_message(sid, "user", "yo", "", None)?;
        let Db { conn } = db;
        conn.execute("PRAGMA user_version=3", [])?;
        let upgraded = Db::setup(conn)?;
        assert_eq!(upgraded.load_all()?[0].title, "keep");
        upgraded.soft_delete_session(sid)?;
        assert!(upgraded.load_all()?.is_empty());
        Ok(())
    }
}
