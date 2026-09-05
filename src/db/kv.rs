//! kv 一站：小配置 JSON 的键值存取（表设计与 CHECK 约束见 [`crate::db`] 模块头）。

use std::error::Error;

use rusqlite::OptionalExtension;

use super::{Db, NOW};

impl Db {
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

    /// 删 kv 一站（不存在静默无操作）；旧配置迁移后清理用。
    pub fn kv_delete(&self, key: &str) -> Result<(), Box<dyn Error>> {
        self.conn.execute("DELETE FROM kv WHERE key = ?1", [key])?;
        Ok(())
    }
}
