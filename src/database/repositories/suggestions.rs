//! Suggestion channels and the suggestions posted in them.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const DB_FILE: &str = "database/suggestions.db";

/// Where suggestions go, and where accepted and declined ones are announced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setup {
    pub channel_id: u64,
    pub accepted_channel_id: u64,
    pub declined_channel_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub message_id: u64,
    pub member_id: u64,
    pub kind: String,
    pub text: String,
    /// DM the member when the suggestion is accepted, declined or reopened
    pub dm: bool,
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
        conn.execute_batch(include_str!("../../../migrations/suggestions.sql"))?;
        Ok(Self { conn })
    }

    pub fn setup(&self, guild_id: u64) -> rusqlite::Result<Option<Setup>> {
        self.conn
            .query_row(
                "SELECT channel_id, accepted_channel_id, declined_channel_id FROM setup WHERE guild_id = ?",
                [guild_id],
                |row| Ok(Setup { channel_id: row.get(0)?, accepted_channel_id: row.get(1)?, declined_channel_id: row.get(2)? }),
            )
            .optional()
    }

    pub fn save_setup(&self, guild_id: u64, setup: &Setup) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO setup (guild_id, channel_id, accepted_channel_id, declined_channel_id) VALUES (?, ?, ?, ?)",
            params![guild_id, setup.channel_id, setup.accepted_channel_id, setup.declined_channel_id],
        )?;
        Ok(())
    }

    pub fn get(&self, guild_id: u64, message_id: u64) -> rusqlite::Result<Option<Suggestion>> {
        self.conn
            .query_row(
                "SELECT message_id, member_id, kind, text, dm FROM suggestions WHERE guild_id = ? AND message_id = ?",
                [guild_id, message_id],
                |row| {
                    Ok(Suggestion {
                        message_id: row.get(0)?,
                        member_id: row.get(1)?,
                        kind: row.get(2)?,
                        text: row.get(3)?,
                        dm: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn add(&self, guild_id: u64, suggestion: &Suggestion) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO suggestions (guild_id, message_id, member_id, kind, text, dm) VALUES (?, ?, ?, ?, ?, ?)",
            params![guild_id, suggestion.message_id, suggestion.member_id, suggestion.kind, suggestion.text, suggestion.dm],
        )?;
        Ok(())
    }

    pub fn remove(&self, guild_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM suggestions WHERE guild_id = ? AND message_id = ?", [guild_id, message_id])?;
        Ok(())
    }

    /// Removes data for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM setup WHERE guild_id NOT IN ({ids});
             DELETE FROM suggestions WHERE guild_id NOT IN ({ids});"
        ))
    }
}
