//! Channels that are off-limits to the bot, per server.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection};

use crate::database::connection;

pub const DB_FILE: &str = "database/offlimit.db";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/offlimit.sql"))?;
        Ok(Self { conn })
    }

    /// Makes the channel off-limits. Returns false if it already was.
    pub fn add(&self, guild_id: u64, channel_id: u64) -> rusqlite::Result<bool> {
        let added = self.conn.execute("INSERT OR IGNORE INTO offlimit_channels (channel_id, guild_id) VALUES (?, ?)", params![channel_id, guild_id])?;
        Ok(added > 0)
    }

    /// Lifts the channel. Returns false if it wasn't off-limits.
    pub fn remove(&self, channel_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM offlimit_channels WHERE channel_id = ?", [channel_id])? > 0)
    }

    pub fn is_offlimit(&self, channel_id: u64) -> rusqlite::Result<bool> {
        self.conn.query_row("SELECT COUNT(*) > 0 FROM offlimit_channels WHERE channel_id = ?", [channel_id], |row| row.get(0))
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<u64>> {
        let mut stmt = self.conn.prepare("SELECT channel_id FROM offlimit_channels WHERE guild_id = ? ORDER BY channel_id")?;
        let rows = stmt.query_map([guild_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Removes off-limits channels in guilds the bot left, or that were deleted.
    pub fn cleanup_stale(&self, current: &HashMap<u64, HashSet<u64>>) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare("SELECT channel_id, guild_id FROM offlimit_channels")?;
        let rows: Vec<(u64, u64)> = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<Result<_, _>>()?;
        for (channel_id, guild_id) in rows {
            if !current.get(&guild_id).is_some_and(|channels| channels.contains(&channel_id)) {
                self.remove(channel_id)?;
            }
        }
        Ok(())
    }
}
