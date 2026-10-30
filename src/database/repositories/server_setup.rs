//! The channels a server picked with /server-setup.

use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const DB_FILE: &str = "database/server_setup.db";

/// Per-guild settings, keyed like "logs.message" or "channel.games", holding a channel ID.
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/server_setup.sql"))?;
        Ok(Self { conn })
    }

    pub fn get(&self, guild_id: u64, key: &str) -> rusqlite::Result<Option<u64>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE guild_id = ? AND key = ?", params![guild_id, key], |row| row.get(0))
            .optional()
    }

    pub fn set(&self, guild_id: u64, key: &str, value: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, key, value) VALUES (?, ?, ?)",
            params![guild_id, key, value],
        )?;
        Ok(())
    }

    /// Returns false if it wasn't set.
    pub fn remove(&self, guild_id: u64, key: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM settings WHERE guild_id = ? AND key = ?", params![guild_id, key])? > 0)
    }

    pub fn all(&self, guild_id: u64) -> rusqlite::Result<Vec<(String, u64)>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings WHERE guild_id = ? ORDER BY key")?;
        let rows = stmt.query_map([guild_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Every guild that has `key` set: (guild_id, value).
    pub fn everywhere(&self, key: &str) -> rusqlite::Result<Vec<(u64, u64)>> {
        let mut stmt = self.conn.prepare("SELECT guild_id, value FROM settings WHERE key = ?")?;
        let rows = stmt.query_map([key], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// Removes every setting for a guild, returning how many there were.
    pub fn reset(&self, guild_id: u64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM settings WHERE guild_id = ?", [guild_id])
    }
}
