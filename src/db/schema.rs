//! schema 定义与版本策略：建表 DDL、时间戳表达式、开发期破坏式重建与补结构例外。
//!
//! 由 [`crate::db`] 装配层在打开连接后调用 [`apply_version`] 完成建表/升级；
//! 表结构的列默认值与语义直接看 [`create_schema`] 里的建表 SQL。

use std::error::Error;

use rusqlite::Connection;

/// 当前 schema 版本。改动表结构 = 此数 +1，旧库整库重建（开发期策略；
/// 纯加列/加表可像 v2→v3、v3→v4 这样留补结构例外，保住已有数据）。
pub const SCHEMA_VERSION: i32 = 4;

/// SQLite 生成的"现在"：RFC3339 UTC 带毫秒（strftime 的 %f 输出 SS.mmm）。
pub const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

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

/// 版本检查与处置：`user_version` 与 [`SCHEMA_VERSION`] 不符时，
/// v2/v3 走补结构例外（保住已有数据），其余整库重建；末了把版本对齐。
pub fn apply_version(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        // 一次性例外：v2/v3 纯补结构（v2 补 persona 表、v2-v3 补 session.deleted_at
        // 列），保住已有数据；其余任何不匹配仍按开发期策略整库重建
        if version == 2 || version == 3 {
            if version == 2 {
                conn.execute_batch(PERSONA_DDL)?;
            }
            if !has_column(conn, "session", "deleted_at")? {
                conn.execute_batch("ALTER TABLE session ADD COLUMN deleted_at TEXT;")?;
            }
        } else {
            create_schema(conn)?;
        }
        conn.execute(&format!("PRAGMA user_version={SCHEMA_VERSION}"), [])?;
    }
    Ok(())
}
