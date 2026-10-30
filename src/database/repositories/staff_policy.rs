//! Staff policy settings: what happens to staff who moderate outside the bot.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension};

use crate::database::{connection, migrations};
use crate::shared::utils::now_iso;

/// Roles taken from a staff member: (user ID, role IDs, when).
pub type Removed = (u64, Vec<u64>, String);

pub const DB_FILE: &str = "database/staff_policy.db";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Given to staff in place of the roles taken
    pub demoted_role: Option<u64>,
    /// Where cases are posted; the Logs cog's member log channel if not set
    pub log_channel: Option<u64>,
    /// The roles to take; every role the bot can take if empty
    pub staff_roles: Vec<u64>,
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
        conn.execute_batch(include_str!("../../../migrations/staff_policy.sql"))?;
        // Added after the table was first made
        migrations::add_column(&conn, "settings", "log_channel", "INTEGER")?;
        migrations::add_column(&conn, "settings", "staff_roles", "TEXT NOT NULL DEFAULT ''")?;
        Ok(Self { conn })
    }

    pub fn parse(ids: &str) -> Vec<u64> {
        ids.split(',').filter_map(|id| id.parse().ok()).collect()
    }

    /// Saves roles taken from a member, adding to any taken before and not yet restored.
    pub fn add(&self, guild_id: u64, user_id: u64, roles: &[u64]) -> rusqlite::Result<()> {
        let mut all = self.get(guild_id, user_id)?.unwrap_or_default();
        for role in roles {
            if !all.contains(role) {
                all.push(*role);
            }
        }
        let ids: Vec<String> = all.iter().map(u64::to_string).collect();
        self.conn.execute(
            "INSERT OR REPLACE INTO removed_roles (guild_id, user_id, role_ids, removed_at) VALUES (?, ?, ?, ?)",
            params![guild_id, user_id, ids.join(","), now_iso()],
        )?;
        Ok(())
    }

    pub fn get(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Vec<u64>>> {
        self.conn
            .query_row("SELECT role_ids FROM removed_roles WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| {
                Ok(Self::parse(&row.get::<_, String>(0)?))
            })
            .optional()
    }

    pub fn remove(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM removed_roles WHERE guild_id = ? AND user_id = ?", [guild_id, user_id])?;
        Ok(())
    }

    /// Forgets every saved record for a guild, returning how many there were.
    pub fn clear(&self, guild_id: u64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM removed_roles WHERE guild_id = ?", [guild_id])
    }

    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<Removed>> {
        let mut stmt = self.conn.prepare("SELECT user_id, role_ids, removed_at FROM removed_roles WHERE guild_id = ? ORDER BY removed_at DESC")?;
        let rows = stmt.query_map([guild_id], |row| Ok((row.get(0)?, Self::parse(&row.get::<_, String>(1)?), row.get(2)?)))?;
        rows.collect()
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        Ok(self
            .conn
            .query_row("SELECT demoted_role, log_channel, staff_roles FROM settings WHERE guild_id = ?", [guild_id], |row| {
                Ok(Settings { demoted_role: row.get(0)?, log_channel: row.get(1)?, staff_roles: Self::parse(&row.get::<_, String>(2)?) })
            })
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, guild_id: u64, settings: &Settings) -> rusqlite::Result<()> {
        let roles: Vec<String> = settings.staff_roles.iter().map(u64::to_string).collect();
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, demoted_role, log_channel, staff_roles) VALUES (?, ?, ?, ?)",
            params![guild_id, settings.demoted_role, settings.log_channel, roles.join(",")],
        )?;
        Ok(())
    }

    /// Removes records for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!(
            "DELETE FROM removed_roles WHERE guild_id NOT IN ({ids}); DELETE FROM settings WHERE guild_id NOT IN ({ids})"
        ))
    }
}
