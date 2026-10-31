//! Server activity over time, for the dashboard's charts: joins, leaves, messages, commands
//! and more, counted per day. Counts are kept in memory and written once a minute, so busy
//! servers don't write to the database on every message.

use std::collections::{HashMap, HashSet};

use ::serenity::all as serenity;
use rusqlite::{params, Connection};

use crate::database::connection;

pub const DB_FILE: &str = "database/analytics.db";
/// Days of history kept
pub const KEEP_DAYS: i64 = 90;

/// Metrics counted per day
pub const JOINS: &str = "joins";
pub const LEAVES: &str = "leaves";
pub const MESSAGES: &str = "messages";
pub const COMMANDS: &str = "commands";
pub const CATCHES: &str = "catches";
pub const INCIDENTS: &str = "incidents";
/// The member count, as last seen that day
pub const MEMBERS: &str = "members";
/// Each command's uses are kept as "cmd:<name>"
pub const COMMAND_PREFIX: &str = "cmd:";

pub fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

pub struct Store {
    conn: Connection,
    /// Counts not written yet, by guild, day and metric
    pending: HashMap<(u64, String, String), i64>,
    /// Values to set rather than add to, like the member count
    snapshots: HashMap<(u64, String, String), i64>,
    active: HashSet<(u64, String, u64)>,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/analytics.sql"))?;
        Ok(Self { conn, pending: HashMap::new(), snapshots: HashMap::new(), active: HashSet::new() })
    }

    /// Adds `by` to today's count of `metric`.
    pub fn bump(&mut self, guild_id: u64, metric: &str, by: i64) {
        *self.pending.entry((guild_id, today(), metric.to_string())).or_default() += by;
    }

    /// Sets today's value of `metric`, like the member count.
    pub fn snapshot(&mut self, guild_id: u64, metric: &str, value: i64) {
        self.snapshots.insert((guild_id, today(), metric.to_string()), value);
    }

    /// Counts a message, and its author as active today.
    pub fn message(&mut self, guild_id: u64, user_id: u64) {
        self.bump(guild_id, MESSAGES, 1);
        self.active.insert((guild_id, today(), user_id));
    }

    /// Writes what's been counted, and forgets history older than `KEEP_DAYS`.
    pub fn flush(&mut self) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        for ((guild, day, metric), by) in self.pending.drain() {
            tx.execute(
                "INSERT INTO daily (guild_id, day, metric, value) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (guild_id, day, metric) DO UPDATE SET value = value + ?4",
                params![guild, day, metric, by],
            )?;
        }
        for ((guild, day, metric), value) in self.snapshots.drain() {
            tx.execute("INSERT OR REPLACE INTO daily (guild_id, day, metric, value) VALUES (?, ?, ?, ?)", params![guild, day, metric, value])?;
        }
        for (guild, day, user) in self.active.drain() {
            tx.execute("INSERT OR IGNORE INTO active (guild_id, day, user_id) VALUES (?, ?, ?)", params![guild, day, user])?;
        }
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(KEEP_DAYS)).format("%Y-%m-%d").to_string();
        tx.execute("DELETE FROM daily WHERE day < ?", [&cutoff])?;
        tx.execute("DELETE FROM active WHERE day < ?", [&cutoff])?;
        tx.commit()
    }

    /// Every metric's value per day from `since` (YYYY-MM-DD) on, with active members as
    /// "active", including what hasn't been written yet.
    pub fn series(&self, guild_id: u64, since: &str) -> rusqlite::Result<HashMap<String, HashMap<String, i64>>> {
        let mut out: HashMap<String, HashMap<String, i64>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT metric, day, value FROM daily WHERE guild_id = ? AND day >= ?")?;
        let rows = stmt.query_map(params![guild_id, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)))?;
        for row in rows {
            let (metric, day, value) = row?;
            out.entry(metric).or_default().insert(day, value);
        }
        for ((guild, day, metric), by) in &self.pending {
            if *guild == guild_id && day.as_str() >= since {
                *out.entry(metric.clone()).or_default().entry(day.clone()).or_default() += by;
            }
        }
        for ((guild, day, metric), value) in &self.snapshots {
            if *guild == guild_id && day.as_str() >= since {
                out.entry(metric.clone()).or_default().insert(day.clone(), *value);
            }
        }
        let mut active: HashMap<String, HashSet<u64>> = HashMap::new();
        let mut stmt = self.conn.prepare("SELECT day, user_id FROM active WHERE guild_id = ? AND day >= ?")?;
        let rows = stmt.query_map(params![guild_id, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)))?;
        for row in rows {
            let (day, user) = row?;
            active.entry(day).or_default().insert(user);
        }
        for (guild, day, user) in &self.active {
            if *guild == guild_id && day.as_str() >= since {
                active.entry(day.clone()).or_default().insert(*user);
            }
        }
        out.insert("active".to_string(), active.into_iter().map(|(day, users)| (day, users.len() as i64)).collect());
        Ok(out)
    }

    /// Removes history for guilds the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<()> {
        // An empty cache means the guilds haven't loaded, not that the bot left them all
        if current_guilds.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        let ids = ids.join(",");
        self.conn.execute_batch(&format!("DELETE FROM daily WHERE guild_id NOT IN ({ids}); DELETE FROM active WHERE guild_id NOT IN ({ids});"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_add_up_before_and_after_writing() {
        let mut s = Store::open_path(":memory:").unwrap();
        s.bump(1, JOINS, 1);
        s.bump(1, JOINS, 2);
        s.message(1, 10);
        s.message(1, 10);
        s.message(1, 11);
        s.snapshot(1, MEMBERS, 50);
        let day = today();
        let before = s.series(1, &day).unwrap();
        assert_eq!(before[JOINS][&day], 3);
        assert_eq!(before[MESSAGES][&day], 3);
        assert_eq!(before["active"][&day], 2);
        s.flush().unwrap();
        s.bump(1, JOINS, 1);
        s.message(1, 10);
        s.snapshot(1, MEMBERS, 52);
        let after = s.series(1, &day).unwrap();
        assert_eq!(after[JOINS][&day], 4);
        assert_eq!(after["active"][&day], 2);
        assert_eq!(after[MEMBERS][&day], 52);
        // Other servers don't see it
        assert!(s.series(2, &day).unwrap().get(JOINS).is_none());
    }
}
