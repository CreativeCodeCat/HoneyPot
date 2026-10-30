//! Bots' messages in servers with a message log, so the log can tell a deleted message was a
//! bot's even once it's no longer in the cache.

use ::serenity::all as serenity;
use rusqlite::Connection;

use crate::database::connection;

pub const DB_FILE: &str = "database/bot_messages.db";

/// How long a bot message is remembered
pub const KEEP_DAYS: i64 = 90;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    pub fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = connection::open(path)?;
        conn.execute_batch(include_str!("../../../migrations/bot_messages.sql"))?;
        Ok(Self { conn })
    }

    pub fn add(&self, guild_id: u64, message_id: u64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO bot_messages (message_id, guild_id) VALUES (?, ?)", [message_id, guild_id])?;
        Ok(())
    }

    /// Forgets the message, returning whether it was a bot's.
    pub fn take(&self, message_id: u64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("DELETE FROM bot_messages WHERE message_id = ?", [message_id])? > 0)
    }

    /// Forgets messages sent more than `KEEP_DAYS` before Unix time `now`. Message IDs start
    /// with when they were sent, so the oldest have the smallest IDs.
    pub fn prune(&self, now: i64) -> rusqlite::Result<usize> {
        self.conn.execute("DELETE FROM bot_messages WHERE message_id < ?", [snowflake_at(now - KEEP_DAYS * 24 * 60 * 60)])
    }

    /// Removes messages from servers the bot is no longer in.
    pub fn cleanup_stale(&self, current_guilds: &[serenity::GuildId]) -> rusqlite::Result<usize> {
        let ids: Vec<String> = current_guilds.iter().map(|g| g.get().to_string()).collect();
        self.conn.execute(&format!("DELETE FROM bot_messages WHERE guild_id NOT IN ({})", ids.join(",")), [])
    }
}

/// The smallest Discord ID for something made at Unix time `secs`.
fn snowflake_at(secs: i64) -> u64 {
    // Discord's epoch, the start of 2015, in milliseconds
    const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
    ((secs * 1000 - DISCORD_EPOCH_MS).max(0) as u64) << 22
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_forgets() {
        let s = Store::open_path(":memory:").unwrap();
        s.add(1, 100).unwrap();
        assert!(s.take(100).unwrap());
        assert!(!s.take(100).unwrap());
        assert!(!s.take(200).unwrap());
    }

    #[test]
    fn prunes_old_messages() {
        let s = Store::open_path(":memory:").unwrap();
        let now = 1_800_000_000;
        let old = snowflake_at(now - (KEEP_DAYS + 1) * 24 * 60 * 60);
        let recent = snowflake_at(now - 24 * 60 * 60);
        s.add(1, old).unwrap();
        s.add(1, recent).unwrap();
        assert_eq!(s.prune(now).unwrap(), 1);
        assert!(s.take(recent).unwrap());
    }

    #[test]
    fn snowflakes_match_discords() {
        // A real message ID and when it was sent
        let id = serenity::MessageId::new(1_100_000_000_000_000_000);
        let secs = id.created_at().unix_timestamp();
        assert!(snowflake_at(secs) <= id.get() && id.get() < snowflake_at(secs + 1));
    }

    #[test]
    fn forgets_servers_left() {
        let s = Store::open_path(":memory:").unwrap();
        s.add(1, 100).unwrap();
        s.add(2, 200).unwrap();
        assert_eq!(s.cleanup_stale(&[serenity::GuildId::new(1)]).unwrap(), 1);
        assert!(s.take(100).unwrap());
    }
}
