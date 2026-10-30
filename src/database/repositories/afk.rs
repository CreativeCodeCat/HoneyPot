//! AFK statuses: who is away, since when and why.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;
use crate::shared::utils::now_iso;

pub const DB_FILE: &str = "database/afk.db";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/afk.sql"))?;
        Ok(Self { conn })
    }

    pub fn set(&self, guild_id: u64, user_id: u64, reason: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO afk (guild_id, user_id, reason, since) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, reason, now_iso()],
        )?;
        Ok(())
    }

    /// The member's reason and when they went AFK, if they're AFK.
    pub fn get(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<(String, String)>> {
        self.conn
            .query_row("SELECT reason, since FROM afk WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
    }

    /// Clears the member's AFK, returning when they went AFK if they were.
    pub fn clear(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("DELETE FROM afk WHERE guild_id = ? AND user_id = ? RETURNING since", [guild_id, user_id], |row| row.get(0))
            .optional()
    }

    /// Removes AFKs for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM afk WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}
