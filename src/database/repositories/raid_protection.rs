//! Raid protection settings, and servers currently in raid mode.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::database::connection;

pub const DB_FILE: &str = "database/raid_protection.db";

/// What happens to accounts that join during a raid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinerAction {
    Nothing,
    Timeout,
    Kick,
    Ban,
    Quarantine,
}

impl JoinerAction {
    pub fn key(self) -> &'static str {
        match self {
            Self::Nothing => "none",
            Self::Timeout => "timeout",
            Self::Kick => "kick",
            Self::Ban => "ban",
            Self::Quarantine => "quarantine",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "timeout" => Self::Timeout,
            "kick" => Self::Kick,
            "ban" => Self::Ban,
            "quarantine" => Self::Quarantine,
            _ => Self::Nothing,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub joins: u32,
    pub seconds: u32,
    pub quiet_minutes: u32,
    pub lock_channels: bool,
    pub raise_verification: bool,
    pub joiner_action: JoinerAction,
    pub timeout_minutes: u32,
    pub alert_channel: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            joins: 10,
            seconds: 30,
            quiet_minutes: 10,
            lock_channels: true,
            raise_verification: true,
            joiner_action: JoinerAction::Kick,
            timeout_minutes: 60,
            alert_channel: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raid {
    pub guild_id: u64,
    pub started_at: i64,
    pub last_activity: i64,
    pub previous_verification: Option<u8>,
    pub locked_channels: Vec<u64>,
    pub joiners: u32,
}

fn raid(row: &Row) -> rusqlite::Result<Raid> {
    let locked: String = row.get(4)?;
    Ok(Raid {
        guild_id: row.get(0)?,
        started_at: row.get(1)?,
        last_activity: row.get(2)?,
        previous_verification: row.get(3)?,
        locked_channels: locked.split(',').filter_map(|id| id.parse().ok()).collect(),
        joiners: row.get(5)?,
    })
}

const RAID_COLUMNS: &str = "guild_id, started_at, last_activity, previous_verification, locked_channels, joiners";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/raid_protection.sql"))?;
        Ok(Self { conn })
    }

    /// The server's settings, or the defaults.
    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        let saved = self
            .conn
            .query_row(
                "SELECT joins, seconds, quiet_minutes, lock_channels, raise_verification, joiner_action, timeout_minutes, alert_channel
                 FROM settings WHERE guild_id = ?",
                [guild_id],
                |row| {
                    Ok(Settings {
                        joins: row.get(0)?,
                        seconds: row.get(1)?,
                        quiet_minutes: row.get(2)?,
                        lock_channels: row.get(3)?,
                        raise_verification: row.get(4)?,
                        joiner_action: JoinerAction::from_key(&row.get::<_, String>(5)?),
                        timeout_minutes: row.get(6)?,
                        alert_channel: row.get(7)?,
                    })
                },
            )
            .optional()?;
        Ok(saved.unwrap_or_default())
    }

    pub fn set_settings(&self, guild_id: u64, s: &Settings) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, joins, seconds, quiet_minutes, lock_channels, raise_verification, joiner_action, timeout_minutes, alert_channel)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![guild_id, s.joins, s.seconds, s.quiet_minutes, s.lock_channels, s.raise_verification, s.joiner_action.key(), s.timeout_minutes, s.alert_channel],
        )?;
        Ok(())
    }

    /// Puts the server in raid mode. Returns false if it already was.
    pub fn start(&self, guild_id: u64, now: i64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("INSERT OR IGNORE INTO raids (guild_id, started_at, last_activity) VALUES (?, ?, ?)", params![guild_id, now, now])? > 0)
    }

    pub fn raid(&self, guild_id: u64) -> rusqlite::Result<Option<Raid>> {
        self.conn.query_row(&format!("SELECT {RAID_COLUMNS} FROM raids WHERE guild_id = ?"), [guild_id], raid).optional()
    }

    pub fn raids(&self) -> rusqlite::Result<Vec<Raid>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {RAID_COLUMNS} FROM raids"))?;
        let rows = stmt.query_map([], raid)?;
        rows.collect()
    }

    /// Records what raid mode changed, so it can be put back.
    pub fn set_changes(&self, guild_id: u64, previous_verification: Option<u8>, locked_channels: &[u64]) -> rusqlite::Result<()> {
        let locked: Vec<String> = locked_channels.iter().map(u64::to_string).collect();
        self.conn.execute(
            "UPDATE raids SET previous_verification = ?, locked_channels = ? WHERE guild_id = ?",
            params![previous_verification, locked.join(","), guild_id],
        )?;
        Ok(())
    }

    /// Notes joins during the raid; `raid_speed` keeps raid mode going.
    pub fn joined(&self, guild_id: u64, count: u32, now: i64, raid_speed: bool) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE raids SET joiners = joiners + ? WHERE guild_id = ?", params![count, guild_id])?;
        if raid_speed {
            self.conn.execute("UPDATE raids SET last_activity = ? WHERE guild_id = ?", params![now, guild_id])?;
        }
        Ok(())
    }

    /// Takes the server out of raid mode, returning what to put back.
    pub fn end(&self, guild_id: u64) -> rusqlite::Result<Option<Raid>> {
        let raid = self.raid(guild_id)?;
        self.conn.execute("DELETE FROM raids WHERE guild_id = ?", [guild_id])?;
        Ok(raid)
    }

    /// Removes settings for servers the bot is no longer in. Raids are left for the raid
    /// loop to end, so nothing stays locked.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute(&format!("DELETE FROM settings WHERE guild_id NOT IN ({})", ids.join(",")), [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_until_saved() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let custom = Settings { joins: 5, joiner_action: JoinerAction::Ban, alert_channel: Some(9), ..Settings::default() };
        s.set_settings(1, &custom).unwrap();
        assert_eq!(s.settings(1).unwrap(), custom);
        assert_eq!(s.settings(2).unwrap(), Settings::default());
    }

    #[test]
    fn raid_mode_starts_once_and_remembers_changes() {
        let s = Store::open_path(":memory:").unwrap();
        assert!(s.start(1, 100).unwrap());
        assert!(!s.start(1, 101).unwrap());
        s.set_changes(1, Some(1), &[5, 6]).unwrap();
        s.joined(1, 3, 150, true).unwrap();
        s.joined(1, 1, 200, false).unwrap();
        let raid = s.raid(1).unwrap().unwrap();
        assert_eq!(raid, Raid { guild_id: 1, started_at: 100, last_activity: 150, previous_verification: Some(1), locked_channels: vec![5, 6], joiners: 4 });
        assert_eq!(s.raids().unwrap().len(), 1);
        assert_eq!(s.end(1).unwrap(), Some(raid));
        assert_eq!(s.raid(1).unwrap(), None);
        assert_eq!(s.end(1).unwrap(), None);
    }
}
