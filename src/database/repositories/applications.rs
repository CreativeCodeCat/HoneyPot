//! Staff application positions and the applications made to them.

use poise::serenity_prelude as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::connection;
use crate::shared::utils::now_iso;

pub const DB_FILE: &str = "database/applications.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub id: i64,
    pub name: String,
    /// Given to accepted applicants
    pub role_id: Option<u64>,
    pub questions: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// A decision on an application; new ones are pending until one is made
pub enum Status {
    Accepted,
    Denied,
}

impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Denied => "denied",
        }
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
        conn.execute_batch(include_str!("../../../migrations/applications.sql"))?;
        Ok(Self { conn })
    }

    pub fn channel(&self, guild_id: u64) -> rusqlite::Result<Option<u64>> {
        self.conn.query_row("SELECT channel_id FROM settings WHERE guild_id = ?", [guild_id], |row| row.get(0)).optional()
    }

    pub fn set_channel(&self, guild_id: u64, channel_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO settings (guild_id, channel_id) VALUES (?, ?)", [guild_id, channel_id])?;
        Ok(())
    }

    pub fn from_row(row: &rusqlite::Row) -> rusqlite::Result<Position> {
        let questions: String = row.get(3)?;
        Ok(Position { id: row.get(0)?, name: row.get(1)?, role_id: row.get(2)?, questions: serde_json::from_str(&questions).unwrap_or_default() })
    }

    pub fn positions(&self, guild_id: u64) -> rusqlite::Result<Vec<Position>> {
        let mut stmt = self.conn.prepare("SELECT id, name, role_id, questions FROM positions WHERE guild_id = ? ORDER BY name")?;
        let rows = stmt.query_map([guild_id], Self::from_row)?;
        rows.collect()
    }

    /// A position by name, ignoring case.
    pub fn position(&self, guild_id: u64, name: &str) -> rusqlite::Result<Option<Position>> {
        self.conn
            .query_row("SELECT id, name, role_id, questions FROM positions WHERE guild_id = ? AND name = ?", params![guild_id, name.trim()], Self::from_row)
            .optional()
    }

    pub fn position_by_id(&self, id: i64) -> rusqlite::Result<Option<Position>> {
        self.conn.query_row("SELECT id, name, role_id, questions FROM positions WHERE id = ?", [id], Self::from_row).optional()
    }

    /// Adds a position, or replaces the one with the same name.
    pub fn save_position(&self, guild_id: u64, name: &str, role_id: Option<u64>, questions: &[String]) -> rusqlite::Result<()> {
        let questions = serde_json::to_string(questions).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO positions (guild_id, name, role_id, questions) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, name) DO UPDATE SET role_id = ?3, questions = ?4",
            params![guild_id, name.trim(), role_id, questions],
        )?;
        Ok(())
    }

    /// Returns false if there was no such position.
    pub fn remove_position(&self, guild_id: u64, name: &str) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM positions WHERE guild_id = ? AND name = ?", params![guild_id, name.trim()])? > 0)
    }

    /// Whether the member already has an application for the position waiting for review.
    pub fn has_pending(&self, guild_id: u64, user_id: u64, position_id: i64) -> rusqlite::Result<bool> {
        self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM applications WHERE guild_id = ? AND user_id = ? AND position_id = ? AND status = 'pending'",
            params![guild_id, user_id, position_id],
            |row| row.get(0),
        )
    }

    /// Records a new application, returning its ID.
    pub fn add_application(&self, guild_id: u64, user_id: u64, position_id: i64) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO applications (guild_id, user_id, position_id, status, created) VALUES (?, ?, ?, 'pending', ?)",
            params![guild_id, user_id, position_id, now_iso()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Decides a pending application, returning its (guild, applicant, position), or `None`
    /// if it was already decided or doesn't exist.
    pub fn decide(&self, id: i64, status: Status) -> rusqlite::Result<Option<(u64, u64, i64)>> {
        self.conn
            .query_row(
                "UPDATE applications SET status = ? WHERE id = ? AND status = 'pending' RETURNING guild_id, user_id, position_id",
                params![status.key(), id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
    }

    pub fn add_panel(&self, guild_id: u64, channel_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO panels (message_id, guild_id, channel_id) VALUES (?, ?, ?)", [message_id, guild_id, channel_id])?;
        Ok(())
    }

    /// The guild's posted panels: (channel, message).
    pub fn panels(&self, guild_id: u64) -> rusqlite::Result<Vec<(u64, u64)>> {
        let mut stmt = self.conn.prepare("SELECT channel_id, message_id FROM panels WHERE guild_id = ?")?;
        let rows = stmt.query_map([guild_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    pub fn remove_panel(&self, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM panels WHERE message_id = ?", [message_id])?;
        Ok(())
    }

    /// Removes everything for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM settings WHERE guild_id NOT IN ({ids});
             DELETE FROM positions WHERE guild_id NOT IN ({ids});
             DELETE FROM applications WHERE guild_id NOT IN ({ids});
             DELETE FROM panels WHERE guild_id NOT IN ({ids});"
        ))
    }
}
