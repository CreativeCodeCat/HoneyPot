//! Reaction roles: an emoji on a message, and the role reacting with it gives.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;

pub const DB_FILE: &str = "database/reaction_roles.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReactionRole {
    pub message_id: u64,
    pub channel_id: u64,
    /// A custom emoji's ID, or the emoji itself; see `emoji_key`
    pub emoji: String,
    pub role_id: u64,
}

/// What an emoji is stored under. Custom emojis by ID, as their name can change; others by
/// the emoji, without the variation selector some keyboards add.
pub fn emoji_key(emoji: &serenity::ReactionType) -> String {
    match emoji {
        serenity::ReactionType::Custom { id, .. } => id.get().to_string(),
        serenity::ReactionType::Unicode(s) => s.replace('\u{fe0f}', ""),
        _ => String::new(),
    }
}

/// How to show a stored emoji in a message.
pub fn emoji_display(key: &str) -> String {
    if key.parse::<u64>().is_ok() {
        format!("<:e:{key}>")
    } else {
        key.to_string()
    }
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
        conn.execute_batch(include_str!("../../../migrations/reaction_roles.sql"))?;
        Ok(Self { conn })
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<ReactionRole> {
        Ok(ReactionRole { message_id: row.get(0)?, channel_id: row.get(1)?, emoji: row.get(2)?, role_id: row.get(3)? })
    }

    /// The role reacting to the message with the emoji gives, if any.
    pub fn role_for(&self, message_id: u64, emoji: &str) -> rusqlite::Result<Option<u64>> {
        self.conn
            .query_row("SELECT role_id FROM reaction_roles WHERE message_id = ? AND emoji = ?", params![message_id, emoji], |row| row.get(0))
            .optional()
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<ReactionRole>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, channel_id, emoji, role_id FROM reaction_roles WHERE guild_id = ? ORDER BY message_id, rowid",
        )?;
        let rows = stmt.query_map([guild_id], Self::from_row)?;
        rows.collect()
    }

    /// Adds the emoji to the message, or changes the role it gives.
    pub fn add(&self, guild_id: u64, reaction_role: &ReactionRole) -> rusqlite::Result<()> {
        let r = reaction_role;
        self.conn.execute(
            "INSERT OR REPLACE INTO reaction_roles (message_id, emoji, guild_id, channel_id, role_id) VALUES (?, ?, ?, ?, ?)",
            params![r.message_id, r.emoji, guild_id, r.channel_id, r.role_id],
        )?;
        Ok(())
    }

    /// Returns false if the emoji gave no role on the message.
    pub fn remove(&self, message_id: u64, emoji: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM reaction_roles WHERE message_id = ? AND emoji = ?", params![message_id, emoji])? > 0)
    }

    /// Removes every reaction role on the message, returning how many there were.
    pub fn remove_message(&self, message_id: u64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM reaction_roles WHERE message_id = ?", [message_id])
    }

    /// Removes reaction roles for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute_batch(&format!("DELETE FROM reaction_roles WHERE guild_id NOT IN ({})", ids.join(",")))
    }
}
