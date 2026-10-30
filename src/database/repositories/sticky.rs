//! Sticky messages, kept at the bottom of their channel.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

/// Default seconds a sticky waits once it's due before moving down, so it isn't reposted in
/// the middle of a conversation; `/sticky delay` changes it for every sticky in a server
pub const DEFAULT_DELAY_SECS: u32 = 5;

pub const DB_FILE: &str = "database/sticky.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sticky {
    pub channel_id: u64,
    pub text: String,
    /// Messages posted below it before it's moved down
    pub max_count: u32,
    pub message_id: u64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/sticky.sql"))?;
        Ok(Self { conn })
    }

    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Sticky> {
        Ok(Sticky { channel_id: row.get(0)?, text: row.get(1)?, max_count: row.get(2)?, message_id: row.get(3)? })
    }

    pub fn get(&self, channel_id: u64) -> rusqlite::Result<Option<Sticky>> {
        self.conn
            .query_row("SELECT channel_id, text, max_count, message_id FROM stickies WHERE channel_id = ?", [channel_id], Self::from_row)
            .optional()
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<Sticky>> {
        let mut stmt = self.conn.prepare("SELECT channel_id, text, max_count, message_id FROM stickies WHERE guild_id = ?")?;
        let rows = stmt.query_map([guild_id], Self::from_row)?;
        rows.collect()
    }

    pub fn add(&self, guild_id: u64, sticky: &Sticky) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO stickies (channel_id, guild_id, text, max_count, message_id) VALUES (?, ?, ?, ?, ?)",
            params![sticky.channel_id, guild_id, sticky.text, sticky.max_count, sticky.message_id],
        )?;
        Ok(())
    }

    /// Returns false if the channel had no sticky.
    pub fn remove(&self, channel_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM stickies WHERE channel_id = ?", [channel_id])? > 0)
    }

    /// Counts a message posted in the channel. Returns the sticky if it's due to be
    /// moved down, starting the count again.
    pub fn bump(&self, channel_id: u64) -> rusqlite::Result<Option<Sticky>> {
        let due = self
            .conn
            .query_row(
                "UPDATE stickies SET count = CASE WHEN count + 1 >= max_count THEN 0 ELSE count + 1 END
                 WHERE channel_id = ? RETURNING count = 0",
                [channel_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        match due {
            Some(true) => self.get(channel_id),
            _ => Ok(None),
        }
    }

    pub fn set_message(&self, channel_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE stickies SET message_id = ? WHERE channel_id = ?", [message_id, channel_id])?;
        Ok(())
    }

    /// Seconds every sticky in the guild waits once it's due before moving down.
    pub fn delay(&self, guild_id: u64) -> rusqlite::Result<u32> {
        Ok(self
            .conn
            .query_row("SELECT delay_secs FROM settings WHERE guild_id = ?", [guild_id], |row| row.get(0))
            .optional()?
            .unwrap_or(DEFAULT_DELAY_SECS))
    }

    pub fn set_delay(&self, guild_id: u64, seconds: u32) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO settings (guild_id, delay_secs) VALUES (?, ?)", params![guild_id, seconds])?;
        Ok(())
    }

    /// Removes stickies for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM stickies WHERE guild_id NOT IN ({ids}); DELETE FROM settings WHERE guild_id NOT IN ({ids})"
        ))
    }
}
