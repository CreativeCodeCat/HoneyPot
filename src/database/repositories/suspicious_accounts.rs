//! Suspicious account scoring: settings, and what each member has done that the score uses.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::database::connection;

pub const DB_FILE: &str = "database/suspicious_accounts.db";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub threshold: u32,
    pub alert_channel: Option<u64>,
    pub auto_quarantine: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { threshold: 60, alert_channel: None, auto_quarantine: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    pub messages: u32,
    pub first_message_at: Option<i64>,
    pub joined_during_raid: bool,
    pub invites: u32,
    pub mass_mentions: u32,
    pub links: u32,
    pub noisy: u32,
    pub bursts: u32,
    pub duplicates: u32,
    pub alerted: bool,
}

/// What one message adds to a member's activity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageFlags {
    pub invite: bool,
    pub mass_mention: bool,
    pub link: bool,
    pub noisy: bool,
    pub burst: bool,
    pub duplicate: bool,
}

const ACTIVITY_COLUMNS: &str = "messages, first_message_at, joined_during_raid, invites, mass_mentions, links, noisy, bursts, duplicates, alerted";

fn activity(row: &Row) -> rusqlite::Result<Activity> {
    Ok(Activity {
        messages: row.get(0)?,
        first_message_at: row.get(1)?,
        joined_during_raid: row.get(2)?,
        invites: row.get(3)?,
        mass_mentions: row.get(4)?,
        links: row.get(5)?,
        noisy: row.get(6)?,
        bursts: row.get(7)?,
        duplicates: row.get(8)?,
        alerted: row.get(9)?,
    })
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
        conn.execute_batch(include_str!("../../../migrations/suspicious_accounts.sql"))?;
        Ok(Self { conn })
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        let saved = self
            .conn
            .query_row("SELECT threshold, alert_channel, auto_quarantine FROM settings WHERE guild_id = ?", [guild_id], |row| {
                Ok(Settings { threshold: row.get(0)?, alert_channel: row.get(1)?, auto_quarantine: row.get(2)? })
            })
            .optional()?;
        Ok(saved.unwrap_or_default())
    }

    pub fn set_settings(&self, guild_id: u64, s: &Settings) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, threshold, alert_channel, auto_quarantine) VALUES (?, ?, ?, ?)",
            params![guild_id, s.threshold, s.alert_channel, s.auto_quarantine],
        )?;
        Ok(())
    }

    /// The member's activity; all zeros if they've done nothing yet.
    pub fn activity(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Activity> {
        let saved = self
            .conn
            .query_row(&format!("SELECT {ACTIVITY_COLUMNS} FROM activity WHERE guild_id = ? AND user_id = ?"), [guild_id, user_id], activity)
            .optional()?;
        Ok(saved.unwrap_or_default())
    }

    /// Members with any activity recorded.
    pub fn members(&self, guild_id: u64) -> rusqlite::Result<Vec<u64>> {
        let mut stmt = self.conn.prepare("SELECT user_id FROM activity WHERE guild_id = ?")?;
        let rows = stmt.query_map([guild_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Starts a member's record when they join, noting whether a raid was going on.
    pub fn joined(&self, guild_id: u64, user_id: u64, during_raid: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO activity (guild_id, user_id, joined_during_raid) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET joined_during_raid = joined_during_raid OR ?3",
            params![guild_id, user_id, during_raid],
        )?;
        Ok(())
    }

    /// Counts a message at Unix time `now`.
    pub fn message(&self, guild_id: u64, user_id: u64, now: i64, flags: MessageFlags) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO activity (guild_id, user_id, messages, first_message_at, invites, mass_mentions, links, noisy, bursts, duplicates)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET
                messages = messages + 1,
                first_message_at = coalesce(first_message_at, ?3),
                invites = invites + ?4,
                mass_mentions = mass_mentions + ?5,
                links = links + ?6,
                noisy = noisy + ?7,
                bursts = bursts + ?8,
                duplicates = duplicates + ?9",
            params![
                guild_id,
                user_id,
                now,
                flags.invite as u32,
                flags.mass_mention as u32,
                flags.link as u32,
                flags.noisy as u32,
                flags.burst as u32,
                flags.duplicate as u32
            ],
        )?;
        Ok(())
    }

    pub fn set_alerted(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO activity (guild_id, user_id, alerted) VALUES (?, ?, 1) ON CONFLICT (guild_id, user_id) DO UPDATE SET alerted = 1",
            [guild_id, user_id],
        )?;
        Ok(())
    }

    /// Removes everything for servers the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute(&format!("DELETE FROM settings WHERE guild_id NOT IN ({ids})"), [])?;
        self.conn.execute(&format!("DELETE FROM activity WHERE guild_id NOT IN ({ids})"), [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_messages_and_flags() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.activity(1, 10).unwrap(), Activity::default());
        s.joined(1, 10, true).unwrap();
        s.message(1, 10, 100, MessageFlags { invite: true, ..Default::default() }).unwrap();
        s.message(1, 10, 200, MessageFlags { link: true, burst: true, ..Default::default() }).unwrap();
        // Rejoining outside a raid doesn't clear the raid flag
        s.joined(1, 10, false).unwrap();
        let a = s.activity(1, 10).unwrap();
        assert_eq!(a, Activity { messages: 2, first_message_at: Some(100), joined_during_raid: true, invites: 1, links: 1, bursts: 1, ..Default::default() });
        assert_eq!(s.members(1).unwrap(), vec![10]);
    }

    #[test]
    fn alerts_once() {
        let s = Store::open_path(":memory:").unwrap();
        s.set_alerted(1, 10).unwrap();
        assert!(s.activity(1, 10).unwrap().alerted);
        s.message(1, 10, 5, MessageFlags::default()).unwrap();
        assert!(s.activity(1, 10).unwrap().alerted);
    }

    #[test]
    fn settings_default_until_saved() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let set = Settings { threshold: 40, alert_channel: Some(3), auto_quarantine: true };
        s.set_settings(1, &set).unwrap();
        assert_eq!(s.settings(1).unwrap(), set);
    }
}
