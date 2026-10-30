//! Quarantine settings, and members in quarantine with the roles they had.

use ::serenity::all as serenity;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::database::connection;

pub const DB_FILE: &str = "database/quarantine.db";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub role_id: Option<u64>,
    pub channel_id: Option<u64>,
    pub review_channel_id: Option<u64>,
    pub new_account_days: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quarantined {
    pub guild_id: u64,
    pub user_id: u64,
    pub reason: String,
    pub source: String,
    pub saved_roles: Vec<u64>,
    pub quarantined_at: i64,
    pub review_message_id: Option<u64>,
}

const COLUMNS: &str = "guild_id, user_id, reason, source, saved_roles, quarantined_at, review_message_id";

fn quarantined(row: &Row) -> rusqlite::Result<Quarantined> {
    let roles: String = row.get(4)?;
    Ok(Quarantined {
        guild_id: row.get(0)?,
        user_id: row.get(1)?,
        reason: row.get(2)?,
        source: row.get(3)?,
        saved_roles: roles.split(',').filter_map(|id| id.parse().ok()).collect(),
        quarantined_at: row.get(5)?,
        review_message_id: row.get(6)?,
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
        conn.execute_batch(include_str!("../../../migrations/quarantine.sql"))?;
        Ok(Self { conn })
    }

    pub fn settings(&self, guild_id: u64) -> rusqlite::Result<Settings> {
        let saved = self
            .conn
            .query_row(
                "SELECT role_id, channel_id, review_channel_id, new_account_days FROM settings WHERE guild_id = ?",
                [guild_id],
                |row| Ok(Settings { role_id: row.get(0)?, channel_id: row.get(1)?, review_channel_id: row.get(2)?, new_account_days: row.get(3)? }),
            )
            .optional()?;
        Ok(saved.unwrap_or_default())
    }

    pub fn set_settings(&self, guild_id: u64, s: &Settings) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (guild_id, role_id, channel_id, review_channel_id, new_account_days) VALUES (?, ?, ?, ?, ?)",
            params![guild_id, s.role_id, s.channel_id, s.review_channel_id, s.new_account_days],
        )?;
        Ok(())
    }

    /// Adds a member to quarantine. Returns false if they already were.
    pub fn add(&self, q: &Quarantined) -> rusqlite::Result<bool> {
        let roles: Vec<String> = q.saved_roles.iter().map(u64::to_string).collect();
        Ok(self.conn.execute(
            &format!("INSERT OR IGNORE INTO quarantined ({COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?)"),
            params![q.guild_id, q.user_id, q.reason, q.source, roles.join(","), q.quarantined_at, q.review_message_id],
        )? > 0)
    }

    pub fn set_review_message(&self, guild_id: u64, user_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("UPDATE quarantined SET review_message_id = ? WHERE guild_id = ? AND user_id = ?", params![message_id, guild_id, user_id])?;
        Ok(())
    }

    pub fn get(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Quarantined>> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM quarantined WHERE guild_id = ? AND user_id = ?"), [guild_id, user_id], quarantined)
            .optional()
    }

    /// The server's quarantined members, longest first.
    pub fn list(&self, guild_id: u64) -> rusqlite::Result<Vec<Quarantined>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {COLUMNS} FROM quarantined WHERE guild_id = ? ORDER BY quarantined_at"))?;
        let rows = stmt.query_map([guild_id], quarantined)?;
        rows.collect()
    }

    /// Takes a member out of quarantine, returning their record.
    pub fn remove(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Quarantined>> {
        let found = self.get(guild_id, user_id)?;
        self.conn.execute("DELETE FROM quarantined WHERE guild_id = ? AND user_id = ?", [guild_id, user_id])?;
        Ok(found)
    }

    /// Removes everything for servers the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute(&format!("DELETE FROM settings WHERE guild_id NOT IN ({ids})"), [])?;
        self.conn.execute(&format!("DELETE FROM quarantined WHERE guild_id NOT IN ({ids})"), [])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(guild_id: u64, user_id: u64, at: i64) -> Quarantined {
        Quarantined { guild_id, user_id, reason: "honeypot".into(), source: "Honeypot".into(), saved_roles: vec![3, 4], quarantined_at: at, review_message_id: None }
    }

    #[test]
    fn settings_default_until_saved() {
        let s = Store::open_path(":memory:").unwrap();
        assert_eq!(s.settings(1).unwrap(), Settings::default());
        let set = Settings { role_id: Some(1), channel_id: Some(2), review_channel_id: Some(3), new_account_days: Some(7) };
        s.set_settings(1, &set).unwrap();
        assert_eq!(s.settings(1).unwrap(), set);
    }

    #[test]
    fn quarantines_once_and_releases() {
        let s = Store::open_path(":memory:").unwrap();
        assert!(s.add(&member(1, 10, 100)).unwrap());
        assert!(!s.add(&member(1, 10, 200)).unwrap());
        assert!(s.add(&member(1, 11, 50)).unwrap());
        s.set_review_message(1, 10, 99).unwrap();
        assert_eq!(s.get(1, 10).unwrap().unwrap().review_message_id, Some(99));
        assert_eq!(s.list(1).unwrap().iter().map(|q| q.user_id).collect::<Vec<_>>(), vec![11, 10]);
        let released = s.remove(1, 10).unwrap().unwrap();
        assert_eq!(released.saved_roles, vec![3, 4]);
        assert_eq!(s.get(1, 10).unwrap(), None);
        assert_eq!(s.remove(1, 10).unwrap(), None);
    }

    #[test]
    fn forgets_servers_left() {
        let s = Store::open_path(":memory:").unwrap();
        s.add(&member(1, 10, 100)).unwrap();
        s.add(&member(2, 10, 100)).unwrap();
        s.cleanup_stale(&[serenity::GuildId::new(1)]).unwrap();
        assert!(s.get(1, 10).unwrap().is_some());
        assert!(s.get(2, 10).unwrap().is_none());
    }
}
